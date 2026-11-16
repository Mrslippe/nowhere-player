//! 把 HTTP-FLV 的 ADTS 字节流包装成 [`np_audio::AudioSource`]。
//!
//! 直播是无限长、不可 seek 的源：`duration_ms = None`，`is_seekable = false`，
//! `seek` 明确失败。EOF 或网络错误都让解码线程结束，由 TUI 决定停止。

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::default::formats::AdtsReader;

use np_audio::format::{AudioFormat, Codec, SampleFormat};
use np_audio::{AudioError, AudioSource};

use crate::error::LiveError;
use crate::flv::FlvAdtsSource;

/// 直播音频 source：`AdtsReader` over 阻塞、不可 seek 的 FLV source。
pub(crate) struct LiveAudioSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    format: AudioFormat,
    decoded: Vec<f32>,
    offset: usize,
    eof: bool,
}

impl LiveAudioSource {
    /// 先读 FLV 头 + AAC 配置，再建 `AdtsReader`。可能阻塞在网络读取上，
    /// 因此只允许在 prepare 线程里调用。
    pub(crate) fn open(mut flv: FlvAdtsSource) -> Result<Self, LiveError> {
        let config = flv.prime()?;
        // `MediaSourceStreamOptions::buffer_len` 必须 > 32KB 且是 2 的幂
        // （symphonia 0.6.1 内部 assert），所以不能用 8KB。实际预读量由
        // FlvAdtsSource 一次只吐一个 ADTS 帧限制，不会把 64KB ring 填满。
        let media = MediaSourceStream::new(Box::new(flv), MediaSourceStreamOptions::default());
        let reader: Box<dyn FormatReader> = Box::new(
            AdtsReader::try_new(media, FormatOptions::default()).map_err(|error| {
                LiveError::UnsupportedAudio(format!("无法打开 AAC 流：{error}"))
            })?,
        );

        let (track_id, params) = {
            let track = reader
                .tracks()
                .first()
                .ok_or_else(|| LiveError::UnsupportedAudio("ADTS 流没有音轨".to_owned()))?;
            let params = track
                .codec_params
                .as_ref()
                .and_then(|params| params.audio())
                .cloned()
                .ok_or_else(|| LiveError::UnsupportedAudio("ADTS 音轨缺少 AAC 参数".to_owned()))?;
            (track.id, params)
        };

        let sample_rate = params.sample_rate.unwrap_or(config.sample_rate());
        let channels = params
            .channels
            .as_ref()
            .map(|channels| channels.count())
            .filter(|count| *count > 0)
            .unwrap_or(usize::from(config.channels()));
        let channels = u16::try_from(channels)
            .map_err(|_| LiveError::UnsupportedAudio("声道数超出范围".to_owned()))?;
        if channels == 0 {
            return Err(LiveError::UnsupportedAudio("声道数为 0".to_owned()));
        }
        let format = AudioFormat::new(sample_rate, channels, SampleFormat::F32);

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(false))
            .map_err(|error| {
                LiveError::UnsupportedAudio(format!("创建 AAC 解码器失败：{error}"))
            })?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            format,
            decoded: Vec::new(),
            offset: 0,
            eof: false,
        })
    }

    fn decode_next_packet(&mut self) -> Result<bool, AudioError> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(false),
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(false);
                }
                Err(error) => {
                    return Err(AudioError::Decode(format!("直播流读取失败：{error}")));
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = self
                .decoder
                .decode(&packet)
                .map_err(|error| AudioError::Decode(format!("直播 AAC 解码失败：{error}")))?;
            let frames = decoded.frames();
            let channels = decoded.spec().channels().count();
            if channels != usize::from(self.format.channels) {
                return Err(AudioError::Decode(format!(
                    "直播声道数变化：{} -> {channels}",
                    self.format.channels
                )));
            }
            self.decoded.resize(frames * channels, 0.0);
            decoded.copy_to_slice_interleaved::<f32, _>(&mut self.decoded[..]);
            self.offset = 0;
            return Ok(true);
        }
    }
}

impl AudioSource for LiveAudioSource {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn codec(&self) -> Codec {
        Codec::Aac
    }

    fn duration_ms(&self) -> Option<u64> {
        None
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        let channels = usize::from(self.format.channels);
        let max_frames = out.len() / channels;
        let mut written = 0;
        while written < max_frames {
            if self.offset < self.decoded.len() {
                let available = (self.decoded.len() - self.offset) / channels;
                let frames = available.min(max_frames - written);
                let start = written * channels;
                let end = start + frames * channels;
                out[start..end]
                    .copy_from_slice(&self.decoded[self.offset..self.offset + frames * channels]);
                self.offset += frames * channels;
                written += frames;
                continue;
            }
            if self.eof {
                break;
            }
            if !self.decode_next_packet()? {
                self.eof = true;
                break;
            }
        }
        Ok(written)
    }

    fn seek(&mut self, _position_ms: u64) -> Result<u64, AudioError> {
        Err(AudioError::Seek("直播流不可 seek".to_owned()))
    }
}
