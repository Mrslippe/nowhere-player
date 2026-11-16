//! Decoding sources.
//!
//! [`AudioSource`] is the seam for online streams later (ARCH-006 §2); v1 only
//! ships [`FileSource`], a symphonia-backed local file decoder.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3,
    CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::units::Time;
use symphonia::default as symphonia_default;

use crate::error::AudioError;
use crate::format::{AudioFormat, Codec, SampleFormat, frames_to_ms, ms_to_frames};

/// A decoded audio source: interleaved f32 frames plus sample-accurate seeks.
///
/// Implementations are driven from the decode thread only.
pub trait AudioSource: Send {
    /// Format of the frames returned by [`read_frames`](Self::read_frames).
    fn format(&self) -> AudioFormat;

    /// Container codec (used for diagnostics; `Dsd` is reserved).
    fn codec(&self) -> Codec;

    /// Total duration when the container states it.
    fn duration_ms(&self) -> Option<u64>;

    /// Whether [`seek`](Self::seek) is meaningful for this source.
    ///
    /// Local files are always seekable; live streams override this to `false`.
    fn is_seekable(&self) -> bool {
        true
    }

    /// Fills `out` with interleaved samples, returning the number of frames
    /// written. `0` means end of stream.
    fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, AudioError>;

    /// Seeks to `position_ms` and returns the position actually reached.
    ///
    /// The return value is the decoder's real position, which may be slightly
    /// before the requested one for packet-granular containers.
    fn seek(&mut self, position_ms: u64) -> Result<u64, AudioError>;
}

/// Local file source backed by symphonia.
pub struct FileSource {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<symphonia::core::units::TimeBase>,
    format: AudioFormat,
    codec: Codec,
    duration_ms: Option<u64>,
    decoded: Vec<f32>,
    offset: usize,
    eof: bool,
}

impl FileSource {
    /// Opens `path` and decodes its container header (no audio yet).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let path = path.as_ref();
        let file = File::open(path).map_err(AudioError::Io)?;
        let source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(extension);
        }
        let reader = symphonia_default::get_probe()
            .probe(&hint, source, Default::default(), Default::default())
            .map_err(|err| AudioError::UnsupportedFormat(format!("{}: {err}", path.display())))?;

        let (track_id, params, num_frames, duration, time_base, short_name) = {
            let track = reader
                .first_track_known_codec(TrackType::Audio)
                .ok_or_else(|| AudioError::UnsupportedFormat("no audio track".into()))?;
            let params = match track.codec_params.as_ref() {
                Some(CodecParameters::Audio(params)) => params.clone(),
                _ => return Err(AudioError::UnsupportedFormat("track is not audio".into())),
            };
            (
                track.id,
                params,
                track.num_frames,
                track.duration,
                track.time_base,
                reader.format_info().short_name,
            )
        };

        let sample_rate = params
            .sample_rate
            .ok_or_else(|| AudioError::UnsupportedFormat("missing sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|channels| channels.count())
            .filter(|count| (1..=usize::from(u16::MAX)).contains(count))
            .ok_or_else(|| AudioError::UnsupportedFormat("missing channel layout".into()))?;
        let format = AudioFormat::new(sample_rate, channels as u16, SampleFormat::F32);
        let codec = codec_from_params(params.codec, short_name);

        let duration_ms = num_frames
            .map(|frames| frames_to_ms(frames, sample_rate))
            .or_else(|| {
                let time_base = time_base?;
                let duration = duration?;
                time_base
                    .calc_duration(duration)
                    .map(|time| time.as_millis().max(0) as u64)
            });

        let decoder = symphonia_default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|err| AudioError::Decode(err.to_string()))?;

        Ok(Self {
            reader,
            decoder,
            track_id,
            time_base,
            format,
            codec,
            duration_ms,
            decoded: Vec::new(),
            offset: 0,
            eof: false,
        })
    }

    /// Decodes one packet into `self.decoded`; `false` means end of stream.
    fn decode_next_packet(&mut self) -> Result<bool, AudioError> {
        loop {
            let packet = match self
                .reader
                .next_packet()
                .map_err(|err| AudioError::Decode(err.to_string()))?
            {
                Some(packet) => packet,
                None => return Ok(false),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = self
                .decoder
                .decode(&packet)
                .map_err(|err| AudioError::Decode(err.to_string()))?;
            let frames = decoded.frames();
            let channels = decoded.spec().channels().count();
            if channels != usize::from(self.format.channels) {
                return Err(AudioError::Decode(format!(
                    "channel count changed mid-stream: {} -> {channels}",
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

impl AudioSource for FileSource {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn codec(&self) -> Codec {
        self.codec
    }

    fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
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

    fn seek(&mut self, position_ms: u64) -> Result<u64, AudioError> {
        let time = Time::from_millis_u64(position_ms);
        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|err| AudioError::Seek(err.to_string()))?;
        self.decoder.reset();
        self.decoded.clear();
        self.offset = 0;
        self.eof = false;
        let mut actual_ms = self
            .time_base
            .and_then(|time_base| time_base.calc_time(seeked.actual_ts))
            .map(|time| time.as_millis().max(0) as u64)
            .unwrap_or(position_ms);

        // Containers seek to a packet boundary, which can be up to one block
        // (≈93 ms for FLAC) before the requested time. Decode and drop the
        // difference so seek lands within the card's 50 ms budget.
        let rate = self.format.sample_rate;
        let target_frames = ms_to_frames(position_ms, rate);
        let actual_frames = ms_to_frames(actual_ms, rate);
        let mut skip = target_frames.saturating_sub(actual_frames);
        if skip > 0 {
            let channels = usize::from(self.format.channels);
            let mut scratch = vec![0.0f32; 4096 * channels];
            while skip > 0 {
                let frames = (skip as usize).min(4096);
                let read = self.read_frames(&mut scratch[..frames * channels])?;
                if read == 0 {
                    break;
                }
                skip = skip.saturating_sub(read as u64);
                actual_ms = actual_ms.saturating_add(frames_to_ms(read as u64, rate));
            }
        }
        Ok(actual_ms)
    }
}

fn codec_from_params(
    codec: symphonia::core::codecs::audio::AudioCodecId,
    short_name: &str,
) -> Codec {
    match codec {
        CODEC_ID_FLAC => Codec::Flac,
        CODEC_ID_MP1 | CODEC_ID_MP2 | CODEC_ID_MP3 => Codec::Mp3,
        CODEC_ID_AAC => Codec::Aac,
        CODEC_ID_ALAC => Codec::Alac,
        CODEC_ID_VORBIS => Codec::Vorbis,
        _ => match short_name {
            "wave" => Codec::Wav,
            "aiff" => Codec::Aiff,
            _ => Codec::Unknown,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSource {
        seekable: bool,
    }

    impl AudioSource for FakeSource {
        fn format(&self) -> AudioFormat {
            AudioFormat::new(48_000, 2, SampleFormat::F32)
        }

        fn codec(&self) -> Codec {
            Codec::Aac
        }

        fn duration_ms(&self) -> Option<u64> {
            None
        }

        fn is_seekable(&self) -> bool {
            self.seekable
        }

        fn read_frames(&mut self, _out: &mut [f32]) -> Result<usize, AudioError> {
            Ok(0)
        }

        fn seek(&mut self, _position_ms: u64) -> Result<u64, AudioError> {
            Err(AudioError::Seek("fake source".to_owned()))
        }
    }

    struct DefaultSource;

    impl AudioSource for DefaultSource {
        fn format(&self) -> AudioFormat {
            AudioFormat::new(48_000, 2, SampleFormat::F32)
        }

        fn codec(&self) -> Codec {
            Codec::Aac
        }

        fn duration_ms(&self) -> Option<u64> {
            None
        }

        fn read_frames(&mut self, _out: &mut [f32]) -> Result<usize, AudioError> {
            Ok(0)
        }

        fn seek(&mut self, _position_ms: u64) -> Result<u64, AudioError> {
            Err(AudioError::Seek("not seekable".to_owned()))
        }
    }

    #[test]
    fn is_seekable_defaults_to_true() {
        assert!(DefaultSource.is_seekable());
        assert!(!FakeSource { seekable: false }.is_seekable());
    }
}
