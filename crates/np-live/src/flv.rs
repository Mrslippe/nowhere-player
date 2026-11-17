//! 最小 HTTP-FLV 解复用：只产出 raw AAC 帧。
//!
//! 只处理 legacy FLV tag：type=8 音频里取 AAC raw frame（**不含 ADTS 头**），
//! type=9 视频只计数并丢弃，type=18 script 忽略。输出直接交给 `source` 里的
//! `symphonia` `AacDecoder`，不再经过 `MediaSourceStream` / `AdtsReader`，所以
//! 中间缓冲就是 `queue` 里的有界帧队列，大小可观测、可裁剪。

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read};
use std::time::Instant;

use crate::error::LiveError;
use crate::metrics::LiveMetrics;

/// 单个 FLV tag 的合理上限（与 B0/B1 一致）。
const MAX_TAG_BYTES: usize = 2 * 1024 * 1024;

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// FLV 里 AAC 的 AudioSpecificConfig；直接作为解码器 `extra_data`。
#[derive(Debug, Clone)]
pub(crate) struct AacConfig {
    sample_rate: u32,
    channel_config: u8,
    asc: Vec<u8>,
}

impl AacConfig {
    /// 解析 `AudioSpecificConfig`（ISO 14496-3）。
    fn parse(asc: &[u8]) -> Result<Self, LiveError> {
        let mut reader = BitReader { data: asc, bit: 0 };
        let aot = reader
            .read(5)
            .ok_or_else(|| LiveError::UnsupportedAudio("ASC 缺少 audioObjectType".to_owned()))?;
        let aot = if aot == 31 {
            32 + reader.read(6).ok_or_else(|| {
                LiveError::UnsupportedAudio("ASC 缺少扩展 audioObjectType".to_owned())
            })?
        } else {
            aot
        };
        if aot == 5 || aot == 29 {
            return Err(LiveError::UnsupportedAudio(format!(
                "AOT {aot}（HE-AAC/SBR）暂不支持"
            )));
        }
        if !(1..=4).contains(&aot) {
            return Err(LiveError::UnsupportedAudio(format!(
                "AOT {aot}，只支持 AAC-LC/Main 等 AOT 1–4"
            )));
        }

        let sf_index = reader
            .read(4)
            .ok_or_else(|| LiveError::UnsupportedAudio("ASC 缺少采样率索引".to_owned()))?
            as u8;
        let sample_rate = if sf_index == 15 {
            reader
                .read(24)
                .ok_or_else(|| LiveError::UnsupportedAudio("ASC 缺少显式采样率".to_owned()))?
        } else {
            *SAMPLE_RATES
                .get(usize::from(sf_index))
                .ok_or_else(|| LiveError::UnsupportedAudio(format!("非法采样率索引 {sf_index}")))?
        };
        if sample_rate == 0 {
            return Err(LiveError::UnsupportedAudio("采样率为 0".to_owned()));
        }

        let channel_config = reader.read(4).ok_or_else(|| {
            LiveError::UnsupportedAudio("ASC 缺少 channelConfiguration".to_owned())
        })? as u8;
        if channel_config == 0 {
            return Err(LiveError::UnsupportedAudio(
                "channelConfiguration=0（PCE）不支持".to_owned(),
            ));
        }
        if channel_config > 7 {
            return Err(LiveError::UnsupportedAudio(format!(
                "非法 channelConfiguration {channel_config}"
            )));
        }

        Ok(Self {
            sample_rate,
            channel_config,
            asc: asc.to_vec(),
        })
    }

    /// 解码后的采样率。
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 解码后的声道数；channelConfiguration 7 表示 7.1（8 声道）。
    pub(crate) fn channels(&self) -> u16 {
        match self.channel_config {
            7 => 8,
            count => u16::from(count),
        }
    }

    /// 交给 `AacDecoder` 的 `extra_data`。
    pub(crate) fn asc(&self) -> &[u8] {
        &self.asc
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn read(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.data.get(self.bit / 8)?;
            let bit = (byte >> (7 - (self.bit % 8))) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit += 1;
        }
        Some(value)
    }
}

/// 一帧原始 AAC（无 ADTS 头），`pts_ms` 来自 FLV tag 时间戳。
#[derive(Debug, Clone)]
pub(crate) struct AacFrame {
    pub(crate) pts_ms: u64,
    pub(crate) raw: Vec<u8>,
}

/// 从 HTTP 长连接按需拉 FLV tag，产出 raw AAC 帧。
///
/// 只被网络线程独占使用，所以不需要 `Mutex` / `MediaSource` 包装。
pub(crate) struct FlvFrameSource {
    reader: Box<dyn Read + Send>,
    config: Option<AacConfig>,
    header_done: bool,
    eof: bool,
    tag_buf: Vec<u8>,
    pending: VecDeque<AacFrame>,
    metrics: LiveMetrics,
}

impl FlvFrameSource {
    pub(crate) fn new(reader: Box<dyn Read + Send>, metrics: LiveMetrics) -> Self {
        Self {
            reader,
            config: None,
            header_done: false,
            eof: false,
            tag_buf: Vec::new(),
            pending: VecDeque::new(),
            metrics,
        }
    }

    /// 读 FLV 头并扫描到 AAC `AudioSpecificConfig`。
    ///
    /// 可能阻塞在网络读取上，只允许在准备线程调用。
    pub(crate) fn open(
        reader: Box<dyn Read + Send>,
        metrics: LiveMetrics,
    ) -> Result<(Self, AacConfig), LiveError> {
        let mut source = Self::new(reader, metrics);
        while source.config.is_none() && !source.eof {
            if !source.pull_tag()? {
                source.eof = true;
            }
        }
        match source.config.clone() {
            Some(config) => Ok((source, config)),
            None => Err(LiveError::UnsupportedAudio(
                "直播流里没有 AAC AudioSpecificConfig".to_owned(),
            )),
        }
    }

    /// 返回下一帧 raw AAC；`None` 表示干净 EOF。
    ///
    /// 同时记录这次调用被阻塞了多久；它包含 tag header / payload 的
    /// blocking HTTP read，是判断“CDN 停顿”最直接的数字。
    pub(crate) fn next_frame(&mut self) -> Result<Option<AacFrame>, LiveError> {
        let started = Instant::now();
        let result = self.next_frame_inner();
        self.metrics
            .note_read_stall(started.elapsed().as_millis() as u64);
        result
    }

    fn next_frame_inner(&mut self) -> Result<Option<AacFrame>, LiveError> {
        while self.pending.is_empty() && !self.eof {
            if !self.pull_tag()? {
                self.eof = true;
            }
        }
        Ok(self.pending.pop_front())
    }

    /// 读一个 FLV tag；`false` 表示干净 EOF。
    fn pull_tag(&mut self) -> Result<bool, LiveError> {
        if !self.header_done {
            self.read_header()?;
        }

        let mut tag_header = [0u8; 11];
        let read = read_upto(&mut *self.reader, &mut tag_header)?;
        if read == 0 {
            return Ok(false);
        }
        if read != tag_header.len() {
            return Err(LiveError::BadStream(format!(
                "FLV tag 头截断：{read}/11 字节"
            )));
        }

        let tag_type = tag_header[0];
        let data_size = u24(&tag_header[1..4]);
        let timestamp = (u24(&tag_header[4..7]) as u32) | (u32::from(tag_header[7]) << 24);
        if data_size > MAX_TAG_BYTES {
            return Err(LiveError::BadStream(format!(
                "FLV tag 过大：{data_size} 字节"
            )));
        }

        self.tag_buf.resize(data_size, 0);
        let read = read_upto(&mut *self.reader, &mut self.tag_buf)?;
        if read != data_size {
            return Err(LiveError::BadStream(format!(
                "FLV tag 内容截断：{read}/{data_size} 字节"
            )));
        }
        let mut previous = [0u8; 4];
        let read = read_upto(&mut *self.reader, &mut previous)?;
        if read != previous.len() {
            return Err(LiveError::BadStream("FLV PreviousTagSize 截断".to_owned()));
        }

        match tag_type {
            8 => self.push_audio(data_size, timestamp)?,
            9 => {
                // only_audio=1 被忽略时仍可能来视频：只计数，绝不进解码链路。
                self.metrics.note_video_tag();
            }
            18 => {}
            _ => {}
        }
        Ok(true)
    }

    fn read_header(&mut self) -> Result<(), LiveError> {
        let mut header = [0u8; 9];
        let read = read_upto(&mut *self.reader, &mut header)?;
        if read != header.len() {
            return Err(LiveError::BadStream(format!("FLV 头截断：{read}/9 字节")));
        }
        if &header[0..3] != b"FLV" {
            return Err(LiveError::BadStream(format!(
                "不是 FLV 流（magic {:02x?}）",
                &header[0..3]
            )));
        }
        let data_offset = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if !(9..=4096).contains(&data_offset) {
            return Err(LiveError::BadStream(format!(
                "非法 FLV data offset {data_offset}"
            )));
        }
        if data_offset > 9 {
            let mut extended = vec![0u8; data_offset - 9];
            let read = read_upto(&mut *self.reader, &mut extended)?;
            if read != extended.len() {
                return Err(LiveError::BadStream("FLV 扩展头截断".to_owned()));
            }
        }
        let mut previous = [0u8; 4];
        let read = read_upto(&mut *self.reader, &mut previous)?;
        if read != previous.len() {
            return Err(LiveError::BadStream("FLV PreviousTagSize0 截断".to_owned()));
        }
        self.header_done = true;
        Ok(())
    }

    fn push_audio(&mut self, data_size: usize, timestamp: u32) -> Result<(), LiveError> {
        if data_size < 2 {
            return Err(LiveError::BadStream("音频 tag 过短".to_owned()));
        }
        let data = &self.tag_buf[..data_size];
        let sound_format = data[0] >> 4;
        if sound_format != 10 {
            return Err(LiveError::UnsupportedAudio(format!(
                "FLV 音频编码 {sound_format}，只支持 AAC(10)"
            )));
        }
        match data[1] {
            0 => {
                // 重复的 ASC 只在首次解析；直播中改配置需要重建解码器，B1.2 不做。
                if self.config.is_none() {
                    self.config = Some(AacConfig::parse(&data[2..])?);
                }
            }
            1 => {
                if self.config.is_none() {
                    return Ok(());
                }
                self.pending.push_back(AacFrame {
                    pts_ms: u64::from(timestamp),
                    raw: data[2..].to_vec(),
                });
                self.metrics.note_frame();
            }
            // 其他 AAC 包类型（如 end-of-sequence）与播放无关。
            _ => {}
        }
        Ok(())
    }
}

fn read_upto(reader: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    while read < buf.len() {
        match reader.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(read)
}

fn u24(bytes: &[u8]) -> usize {
    (usize::from(bytes[0]) << 16) | (usize::from(bytes[1]) << 8) | usize::from(bytes[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const ASC_48K_STEREO: [u8; 4] = [0xAF, 0x00, 0x11, 0x90];

    fn tag_at(tag_type: u8, payload: &[u8], ts: u32) -> Vec<u8> {
        let size = payload.len();
        let mut bytes = vec![
            tag_type,
            (size >> 16) as u8,
            (size >> 8) as u8,
            size as u8,
            (ts >> 16) as u8,
            (ts >> 8) as u8,
            ts as u8,
            (ts >> 24) as u8,
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&(size as u32 + 11).to_be_bytes());
        bytes
    }

    fn tag(tag_type: u8, payload: &[u8]) -> Vec<u8> {
        tag_at(tag_type, payload, 0)
    }

    fn flv(flags: u8, tags: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"FLV\x01".to_vec();
        bytes.push(flags);
        bytes.extend_from_slice(&9u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        for tag in tags {
            bytes.extend_from_slice(tag);
        }
        bytes
    }

    fn audio_frame(payload: &[u8]) -> Vec<u8> {
        let mut tag_payload = vec![0xAF, 0x01];
        tag_payload.extend_from_slice(payload);
        tag_payload
    }

    fn synthetic_stream() -> Vec<u8> {
        flv(
            4,
            &[
                tag(18, b"\x00\x00\x00\x00\x00"),
                tag(8, &ASC_48K_STEREO),
                tag(9, b"\x17\x00\x00\x00\x00\xAA"),
                tag_at(8, &audio_frame(&[0x21, 0x10, 0x56, 0xE5, 0x00, 0x11]), 21),
            ],
        )
    }

    fn open(bytes: Vec<u8>) -> Result<(FlvFrameSource, AacConfig, LiveMetrics), LiveError> {
        let metrics = LiveMetrics::default();
        let (source, config) = FlvFrameSource::open(Box::new(Cursor::new(bytes)), metrics.clone())?;
        Ok((source, config, metrics))
    }

    #[test]
    fn parses_asc_and_emits_raw_frames_without_adts_headers() {
        let (mut source, config, metrics) = open(synthetic_stream()).expect("AAC config");
        assert_eq!(config.sample_rate(), 48_000);
        assert_eq!(config.channels(), 2);

        let frame = source.next_frame().unwrap().expect("one raw frame");
        assert_eq!(frame.pts_ms, 21);
        assert_eq!(frame.raw, [0x21, 0x10, 0x56, 0xE5, 0x00, 0x11]);
        // 没有 7 字节 ADTS 头：raw 就是 tag 里 ASC 之后的内容。
        assert_ne!(frame.raw[0], 0xFF);
        assert!(source.next_frame().unwrap().is_none());
        assert_eq!(metrics.frames_received(), 1);
        assert_eq!(metrics.video_tags(), 1);
    }

    #[test]
    fn video_tags_are_counted_but_never_emitted() {
        let (mut source, _config, metrics) = open(synthetic_stream()).unwrap();
        let mut frames = 0;
        while source.next_frame().unwrap().is_some() {
            frames += 1;
        }
        assert_eq!(frames, 1);
        assert_eq!(metrics.video_tags(), 1);
    }

    #[test]
    fn repeats_of_the_sequence_header_do_not_duplicate_frames() {
        let payload = audio_frame(&[0x21, 0x10]);
        let bytes = flv(
            4,
            &[
                tag(8, &ASC_48K_STEREO),
                tag(8, &ASC_48K_STEREO),
                tag_at(8, &payload, 10),
                tag_at(8, &payload, 31),
            ],
        );
        let (mut source, _config, metrics) = open(bytes).unwrap();
        let first = source.next_frame().unwrap().unwrap();
        let second = source.next_frame().unwrap().unwrap();
        assert_eq!((first.pts_ms, second.pts_ms), (10, 31));
        assert_eq!(metrics.frames_received(), 2);
    }

    #[test]
    fn frames_before_the_sequence_header_are_skipped() {
        let bytes = flv(
            4,
            &[
                tag(8, &audio_frame(&[0x21])),
                tag(8, &ASC_48K_STEREO),
                tag(8, &audio_frame(&[0x22])),
            ],
        );
        let (mut source, _config, metrics) = open(bytes).unwrap();
        assert_eq!(source.next_frame().unwrap().unwrap().raw, [0x22]);
        assert_eq!(metrics.frames_received(), 1);
    }

    #[test]
    fn he_aac_config_is_rejected_with_a_readable_error() {
        let bytes = flv(4, &[tag(8, &[0xAF, 0x00, 0x2B, 0x92, 0x08, 0x00])]);
        let error = open(bytes).err().expect("expected an error");
        assert!(matches!(error, LiveError::UnsupportedAudio(_)));
    }

    #[test]
    fn non_aac_audio_is_rejected() {
        let bytes = flv(4, &[tag(8, &[0x2F, 0x00, 0x00])]);
        let error = open(bytes).err().expect("expected an error");
        assert!(matches!(error, LiveError::UnsupportedAudio(_)));
    }

    #[test]
    fn non_flv_stream_is_rejected() {
        let error = open(vec![0u8; 9]).err().expect("expected an error");
        assert!(matches!(error, LiveError::BadStream(_)));
    }

    #[test]
    fn video_only_stream_has_no_audio() {
        let bytes = flv(1, &[tag(9, b"\x17\x00\x00\x00\x00\xAA")]);
        let error = open(bytes).err().expect("expected an error");
        assert!(matches!(error, LiveError::UnsupportedAudio(_)));
    }

    #[test]
    fn truncated_tag_payload_is_rejected() {
        let mut bytes = synthetic_stream();
        bytes.truncate(bytes.len() - 4);
        // 截断在最后一个 audio tag 上：open 已经拿到 ASC，读取帧时才撞上错误。
        let (mut source, _config, _metrics) = open(bytes).expect("config still parses");
        let error = source.next_frame().expect_err("expected an error");
        assert!(matches!(error, LiveError::BadStream(_)));
    }
}
