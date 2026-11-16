//! 最小 HTTP-FLV → ADTS 适配。
//!
//! 只处理 legacy FLV tag：type=8 音频转 ADTS，type=9 视频只计数并丢弃，
//! type=18 script 忽略。输出是顺序、不可 seek 的字节流，交给
//! `symphonia` 的 `AdtsReader`。

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;

use crate::error::LiveError;

/// 单个 FLV tag 的合理上限（与 B0/B1a 一致）。
const MAX_TAG_BYTES: usize = 2 * 1024 * 1024;
/// `prime` 最多消费的 tag 数，避免异常流把准备线程卡住。
const MAX_PRIME_TAGS: u32 = 64;

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// FLV 里 AAC 的 AudioSpecificConfig。
#[derive(Debug, Clone)]
pub(crate) struct AacConfig {
    aot: u8,
    sf_index: u8,
    sample_rate: u32,
    channel_config: u8,
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
                "AOT {aot}（HE-AAC/SBR）无法用 ADTS 表达"
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
        if sf_index == 15 {
            return Err(LiveError::UnsupportedAudio(
                "ASC 显式采样率无法用 ADTS 表达".to_owned(),
            ));
        }
        let sample_rate = *SAMPLE_RATES
            .get(sf_index as usize)
            .ok_or_else(|| LiveError::UnsupportedAudio(format!("非法采样率索引 {sf_index}")))?;

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
            aot: aot as u8,
            sf_index,
            sample_rate,
            channel_config,
        })
    }

    /// 解码后的采样率。
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 解码后的声道数；ADTS 的 channelConfiguration 7 表示 7.1（8 声道）。
    pub(crate) fn channels(&self) -> u16 {
        match self.channel_config {
            7 => 8,
            count => u16::from(count),
        }
    }

    /// 7 字节无 CRC 的 ADTS header。
    fn adts_header(&self, payload_len: usize) -> Result<[u8; 7], LiveError> {
        let frame_len = payload_len + 7;
        if frame_len > 0x1FFF {
            return Err(LiveError::BadStream(format!(
                "AAC 帧过大，无法写入 ADTS：{frame_len} 字节"
            )));
        }
        let profile = self.aot - 1;
        let channel = self.channel_config;
        let frame_len = frame_len as u32;
        let mut header = [0u8; 7];
        header[0] = 0xFF;
        header[1] = 0xF1; // MPEG-4, layer 0, no CRC
        header[2] = (profile << 6) | ((self.sf_index & 0x0F) << 2) | ((channel >> 2) & 0x01);
        header[3] = ((channel & 0x03) << 6) | ((frame_len >> 11) as u8 & 0x03);
        header[4] = ((frame_len >> 3) & 0xFF) as u8;
        header[5] = (((frame_len & 0x07) << 5) as u8) | 0x1F;
        header[6] = 0xFC;
        Ok(header)
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

/// 直播流的轻量诊断计数。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LiveStreamStats {
    pub(crate) flv_flags: u8,
    pub(crate) tags_total: u64,
    pub(crate) audio_tags: u64,
    pub(crate) video_tags: u64,
    pub(crate) script_tags: u64,
    pub(crate) other_tags: u64,
    pub(crate) audio_payload_bytes: u64,
    pub(crate) video_payload_bytes: u64,
    pub(crate) sequence_headers: u64,
    pub(crate) raw_audio_frames: u64,
    pub(crate) frames_before_asc: u64,
    pub(crate) adts_bytes: u64,
}

/// 从 HTTP 长连接按需拉 FLV tag，产出 ADTS 字节流。
///
/// `MediaSource` 要求 `Send + Sync`，所以底层 reader 放在 `Mutex` 里；
/// 实际只有解码线程顺序读取，不构成竞争。
pub(crate) struct FlvAdtsSource {
    reader: Mutex<Box<dyn Read + Send>>,
    pending: VecDeque<u8>,
    config: Option<AacConfig>,
    header_done: bool,
    eof: bool,
    tag_buf: Vec<u8>,
    stats: LiveStreamStats,
    first_audio_at: Option<Instant>,
    first_audio_ts: Option<u32>,
    last_audio_ts: Option<u32>,
    /// 初始突发期间只保留最新 AAC 帧，避免把 CDN 的 GOP/预缓冲全播一遍。
    keep_latest: bool,
}

impl FlvAdtsSource {
    pub(crate) fn new(reader: Box<dyn Read + Send>) -> Self {
        Self {
            reader: Mutex::new(reader),
            pending: VecDeque::new(),
            config: None,
            header_done: false,
            eof: false,
            tag_buf: Vec::new(),
            stats: LiveStreamStats::default(),
            first_audio_at: None,
            first_audio_ts: None,
            last_audio_ts: None,
            keep_latest: false,
        }
    }

    /// 读到首个 AAC 配置和首批 ADTS 字节。在网络线程里调用，主线程只拿成品。
    ///
    /// 连接建立时 B 站 CDN 常会先吐一段 GOP/预缓冲（实测 2.5s wall 收到约
    /// 3.8s 音频）。如果直接播放，会把这段缓冲全部播完，稳定延迟约 1–2s。
    /// 先探 300ms：媒体时间明显跑在墙钟前面时，丢弃旧帧，最多再读 2s，
    /// 只保留最新一帧，让播放从接近 live edge 的位置开始。
    pub(crate) fn prime(&mut self) -> Result<AacConfig, LiveError> {
        let mut consumed = 0;
        while (self.config.is_none() || self.pending.is_empty()) && consumed < MAX_PRIME_TAGS {
            if self.eof || !self.pull_tag()? {
                self.eof = true;
                break;
            }
            consumed += 1;
        }
        let Some(config) = self.config.clone() else {
            return Err(LiveError::UnsupportedAudio(
                "直播流里没有 AAC AudioSpecificConfig".to_owned(),
            ));
        };
        if self.pending.is_empty() {
            return Err(LiveError::BadStream("直播流在首个 AAC 帧前结束".to_owned()));
        }

        let Some(first_at) = self.first_audio_at else {
            return Ok(config);
        };
        let probe_deadline = first_at + Duration::from_millis(300);
        while Instant::now() < probe_deadline && !self.eof {
            if !self.pull_tag()? {
                self.eof = true;
                break;
            }
        }
        let wall_ms = first_at.elapsed().as_millis() as u32;
        let media_ms = self
            .last_audio_ts
            .zip(self.first_audio_ts)
            .map(|(last, first)| last.saturating_sub(first))
            .unwrap_or(0);
        if media_ms > wall_ms.saturating_add(100) {
            // 初始突发：丢掉探针读到的旧帧，只保留后续最新帧。
            self.pending.clear();
            self.keep_latest = true;
            let drain_deadline = first_at + Duration::from_millis(2000);
            while Instant::now() < drain_deadline && !self.eof {
                if !self.pull_tag()? {
                    self.eof = true;
                    break;
                }
            }
            self.keep_latest = false;
        }
        if self.pending.is_empty() {
            return Err(LiveError::BadStream("直播流在首个 AAC 帧前结束".to_owned()));
        }
        Ok(config)
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> LiveStreamStats {
        self.stats
    }

    /// 读一个 FLV tag；`false` 表示干净 EOF。
    fn pull_tag(&mut self) -> Result<bool, LiveError> {
        if !self.header_done {
            self.read_header()?;
        }

        let mut tag_header = [0u8; 11];
        let read = {
            let mut reader = self.reader();
            read_upto(&mut **reader, &mut tag_header)?
        };
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
        let ts = (u24(&tag_header[4..7]) as u32) | ((tag_header[7] as u32) << 24);
        if data_size > MAX_TAG_BYTES {
            return Err(LiveError::BadStream(format!(
                "FLV tag 过大：{data_size} 字节"
            )));
        }

        self.tag_buf.resize(data_size, 0);
        let read = {
            let mut reader = self
                .reader
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            read_upto(&mut **reader, &mut self.tag_buf)?
        };
        if read != data_size {
            return Err(LiveError::BadStream(format!(
                "FLV tag 内容截断：{read}/{data_size} 字节"
            )));
        }
        let mut previous = [0u8; 4];
        let read = {
            let mut reader = self.reader();
            read_upto(&mut **reader, &mut previous)?
        };
        if read != previous.len() {
            return Err(LiveError::BadStream("FLV PreviousTagSize 截断".to_owned()));
        }

        self.stats.tags_total += 1;
        match tag_type {
            8 => {
                self.stats.audio_tags += 1;
                self.stats.audio_payload_bytes += data_size as u64;
                self.push_audio(data_size, ts)?;
            }
            9 => {
                // only_audio=1 被忽略时仍可能来视频：只计数，绝不进解码链路。
                self.stats.video_tags += 1;
                self.stats.video_payload_bytes += data_size as u64;
            }
            18 => self.stats.script_tags += 1,
            _ => self.stats.other_tags += 1,
        }
        Ok(true)
    }

    fn reader(&self) -> std::sync::MutexGuard<'_, Box<dyn Read + Send>> {
        self.reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read_header(&mut self) -> Result<(), LiveError> {
        let mut header = [0u8; 9];
        let read = {
            let mut reader = self.reader();
            read_upto(&mut **reader, &mut header)?
        };
        if read != header.len() {
            return Err(LiveError::BadStream(format!("FLV 头截断：{read}/9 字节")));
        }
        if &header[0..3] != b"FLV" {
            return Err(LiveError::BadStream(format!(
                "不是 FLV 流（magic {:02x?}）",
                &header[0..3]
            )));
        }
        self.stats.flv_flags = header[4];
        let data_offset = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
        if !(9..=4096).contains(&data_offset) {
            return Err(LiveError::BadStream(format!(
                "非法 FLV data offset {data_offset}"
            )));
        }
        if data_offset > 9 {
            let mut extended = vec![0u8; data_offset - 9];
            let read = {
                let mut reader = self.reader();
                read_upto(&mut **reader, &mut extended)?
            };
            if read != extended.len() {
                return Err(LiveError::BadStream("FLV 扩展头截断".to_owned()));
            }
        }
        let mut previous = [0u8; 4];
        let read = {
            let mut reader = self.reader();
            read_upto(&mut **reader, &mut previous)?
        };
        if read != previous.len() {
            return Err(LiveError::BadStream("FLV PreviousTagSize0 截断".to_owned()));
        }
        self.header_done = true;
        Ok(())
    }

    fn push_audio(&mut self, data_size: usize, ts: u32) -> Result<(), LiveError> {
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
                self.stats.sequence_headers += 1;
                if self.config.is_none() {
                    self.config = Some(AacConfig::parse(&data[2..])?);
                }
            }
            1 => {
                let Some(config) = &self.config else {
                    self.stats.frames_before_asc += 1;
                    return Ok(());
                };
                let payload = &data[2..];
                let header = config.adts_header(payload.len())?;
                if self.keep_latest {
                    self.pending.clear();
                }
                self.pending.extend(header);
                self.pending.extend(payload.iter().copied());
                self.first_audio_at.get_or_insert_with(Instant::now);
                self.first_audio_ts.get_or_insert(ts);
                self.last_audio_ts = Some(ts);
                self.stats.raw_audio_frames += 1;
                self.stats.adts_bytes += (header.len() + payload.len()) as u64;
            }
            _ => {
                // 其他 AAC 包类型（如 end-of-sequence）与播放无关。
            }
        }
        Ok(())
    }
}

impl Read for FlvAdtsSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() && !self.eof {
            match self.pull_tag() {
                Ok(true) => {}
                Ok(false) => self.eof = true,
                Err(error) => {
                    self.eof = true;
                    return Err(io::Error::new(ErrorKind::InvalidData, error.to_string()));
                }
            }
        }
        let mut written = 0;
        while written < buf.len() {
            let Some(byte) = self.pending.pop_front() else {
                break;
            };
            buf[written] = byte;
            written += 1;
        }
        Ok(written)
    }
}

impl Seek for FlvAdtsSource {
    fn seek(&mut self, _position: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(ErrorKind::Unsupported, "直播流不可 seek"))
    }
}

impl MediaSource for FlvAdtsSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
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

    fn synthetic_stream() -> Vec<u8> {
        let mut audio = vec![0xAF, 0x01];
        audio.extend_from_slice(&[0x21, 0x10, 0x56, 0xE5, 0x00, 0x11]);
        flv(
            4,
            &[
                tag(18, b"\x00\x00\x00\x00\x00"),
                tag(8, &ASC_48K_STEREO),
                tag(9, b"\x17\x00\x00\x00\x00\xAA"),
                tag(8, &audio),
            ],
        )
    }

    fn source(bytes: Vec<u8>) -> FlvAdtsSource {
        FlvAdtsSource::new(Box::new(Cursor::new(bytes)))
    }

    #[test]
    fn prime_parses_asc_and_leaves_one_adts_frame() {
        let mut source = source(synthetic_stream());
        let config = source.prime().expect("AAC config");
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.channels(), 2);
        assert_eq!(config.aot, 2);
        assert_eq!(source.stats().audio_tags, 2);
        assert_eq!(source.stats().video_tags, 1);
        assert_eq!(source.stats().video_payload_bytes, 6);
        assert_eq!(source.stats().raw_audio_frames, 1);

        let mut out = [0u8; 64];
        let read = source.read(&mut out).unwrap();
        assert_eq!(read, 7 + 6);
        assert_eq!(&out[..2], &[0xFF, 0xF1]);
        assert_eq!(out[2], 0x4C); // AAC-LC, 48 kHz, stereo
        let frame_len = ((usize::from(out[3] & 0x03)) << 11)
            | (usize::from(out[4]) << 3)
            | (usize::from(out[5]) >> 5);
        assert_eq!(frame_len, 7 + 6);
        assert_eq!(&out[7..13], &[0x21, 0x10, 0x56, 0xE5, 0x00, 0x11]);

        assert_eq!(source.read(&mut out).unwrap(), 0);
        assert!(source.eof);
    }

    #[test]
    fn keep_latest_discards_previous_frames() {
        let mut source = source(synthetic_stream());
        source.config = Some(AacConfig::parse(&ASC_48K_STEREO[2..]).unwrap());
        source.keep_latest = true;
        source.tag_buf = vec![0xAF, 0x01, 7];
        source.push_audio(3, 100).unwrap();
        source.tag_buf = vec![0xAF, 0x01, 9];
        source.push_audio(3, 200).unwrap();

        assert_eq!(source.pending.len(), 8, "only the last ADTS frame survives");
        assert_eq!(*source.pending.back().unwrap(), 9);
        assert_eq!(source.first_audio_ts, Some(100));
        assert_eq!(source.last_audio_ts, Some(200));
    }

    #[test]
    fn video_tags_are_counted_but_never_enter_the_adts_stream() {
        let mut source = source(synthetic_stream());
        source.prime().unwrap();
        let mut adts = Vec::new();
        source.read_to_end(&mut adts).unwrap();
        assert_eq!(adts.len(), 13);
        assert_eq!(source.stats().video_tags, 1);
        assert_eq!(source.stats().script_tags, 1);
        assert_eq!(source.stats().tags_total, 4);
    }

    #[test]
    fn live_source_is_not_seekable() {
        let source = source(synthetic_stream());
        assert!(!source.is_seekable());
        assert!(source.byte_len().is_none());
    }

    #[test]
    fn he_aac_config_is_rejected_with_a_readable_error() {
        let bytes = flv(4, &[tag(8, &[0xAF, 0x00, 0x2B, 0x92, 0x08, 0x00])]);
        let error = source(bytes).prime().unwrap_err();
        assert!(matches!(error, LiveError::UnsupportedAudio(_)));
    }

    #[test]
    fn non_flv_stream_is_rejected() {
        let error = source(vec![0u8; 9]).prime().unwrap_err();
        assert!(matches!(error, LiveError::BadStream(_)));
    }

    #[test]
    fn video_only_stream_has_no_audio() {
        let bytes = flv(1, &[tag(9, b"\x17\x00\x00\x00\x00\xAA")]);
        let error = source(bytes).prime().unwrap_err();
        assert!(matches!(error, LiveError::UnsupportedAudio(_)));
    }

    #[test]
    fn truncated_tag_payload_is_rejected() {
        let mut bytes = synthetic_stream();
        bytes.truncate(bytes.len() - 4);
        let error = source(bytes).prime().unwrap_err();
        assert!(matches!(error, LiveError::BadStream(_) | LiveError::Io(_)));
    }
}
