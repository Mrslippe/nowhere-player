//! Audio format descriptors, device capabilities and format negotiation.

use crate::error::AudioError;

/// Sample format of one audio stream.
///
/// The internal pipeline always works on `f32`; this type describes what a
/// device actually accepts (or what the decoder produced).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SampleFormat {
    F32,
    I16,
    I32,
}

impl SampleFormat {
    /// Lower is better when choosing a device format.
    const fn preference(self) -> u8 {
        match self {
            Self::F32 => 0,
            Self::I32 => 1,
            Self::I16 => 2,
        }
    }
}

impl std::fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::F32 => "f32",
            Self::I16 => "i16",
            Self::I32 => "i32",
        };
        f.write_str(name)
    }
}

/// Codec of the current source.
///
/// [`Codec::Dsd`] is a reserved value (ARCH-006 §3): v1 never produces it, but
/// keeping it here means adding DSD later does not change the decoder signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    Flac,
    Mp3,
    Aac,
    Alac,
    Wav,
    Vorbis,
    Aiff,
    Dsd,
    Unknown,
}

/// A concrete audio format: rate, channel count and sample representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

impl AudioFormat {
    pub const fn new(sample_rate: u32, channels: u16, sample_format: SampleFormat) -> Self {
        Self {
            sample_rate,
            channels,
            sample_format,
        }
    }

    /// Number of samples in one frame, i.e. one per channel.
    pub const fn frame_samples(self) -> usize {
        self.channels as usize
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} Hz / {} ch / {}",
            self.sample_rate, self.channels, self.sample_format
        )
    }
}

/// What one output device can do.
///
/// `formats` lists every rate/channel/sample-format combination we can drive.
/// `exclusive` is always `false` for the v1 shared-mode sink; the field exists
/// so a future exclusive backend can advertise itself without changing the
/// player-facing API (ARCH-006 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCaps {
    pub formats: Vec<AudioFormat>,
    pub exclusive: bool,
}

impl DeviceCaps {
    pub fn supports(&self, format: AudioFormat) -> bool {
        self.formats.contains(&format)
    }
}

/// Chooses the device format for `source`.
///
/// Preference order: same sample rate, same channel count, the rate closest to
/// the source (cheapest resampling), then the sample format that needs the
/// least conversion (f32 > i32 > i16). Picking a device format with the source
/// rate is what keeps `ProcessingPlan::resample` false and the bit-perfect path
/// open.
pub fn negotiate(source: AudioFormat, caps: &DeviceCaps) -> Result<AudioFormat, AudioError> {
    caps.formats
        .iter()
        .copied()
        .min_by_key(|candidate| {
            (
                u8::from(candidate.sample_rate != source.sample_rate),
                u8::from(candidate.channels != source.channels),
                candidate.sample_rate.abs_diff(source.sample_rate),
                candidate.sample_format.preference(),
                candidate.sample_rate,
            )
        })
        .ok_or(AudioError::NoSupportedFormat)
}

/// Converts a seek target in milliseconds to a frame index at `sample_rate`.
pub fn ms_to_frames(ms: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    ms.saturating_mul(u64::from(sample_rate)) / 1000
}

/// Converts a played frame count back to milliseconds (rounded down).
pub fn frames_to_ms(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    frames.saturating_mul(1000) / u64::from(sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(rate: u32, channels: u16, sample_format: SampleFormat) -> AudioFormat {
        AudioFormat::new(rate, channels, sample_format)
    }

    #[test]
    fn negotiate_prefers_identical_rate_and_channels() {
        let caps = DeviceCaps {
            formats: vec![
                fmt(48_000, 2, SampleFormat::I16),
                fmt(44_100, 2, SampleFormat::I16),
                fmt(44_100, 1, SampleFormat::F32),
            ],
            exclusive: false,
        };
        let chosen = negotiate(fmt(44_100, 2, SampleFormat::F32), &caps).unwrap();
        assert_eq!(chosen, fmt(44_100, 2, SampleFormat::I16));
    }

    #[test]
    fn negotiate_prefers_f32_when_rate_and_channels_tie() {
        let caps = DeviceCaps {
            formats: vec![
                fmt(48_000, 2, SampleFormat::I16),
                fmt(48_000, 2, SampleFormat::F32),
            ],
            exclusive: false,
        };
        let chosen = negotiate(fmt(48_000, 2, SampleFormat::F32), &caps).unwrap();
        assert_eq!(chosen, fmt(48_000, 2, SampleFormat::F32));
    }

    #[test]
    fn negotiate_falls_back_when_source_rate_is_unsupported() {
        let caps = DeviceCaps {
            formats: vec![
                fmt(48_000, 2, SampleFormat::I16),
                fmt(96_000, 2, SampleFormat::I32),
            ],
            exclusive: false,
        };
        let chosen = negotiate(fmt(44_100, 2, SampleFormat::F32), &caps).unwrap();
        assert_eq!(chosen, fmt(48_000, 2, SampleFormat::I16));
    }

    #[test]
    fn negotiate_without_caps_is_an_error() {
        let caps = DeviceCaps {
            formats: Vec::new(),
            exclusive: false,
        };
        assert!(matches!(
            negotiate(fmt(44_100, 2, SampleFormat::F32), &caps),
            Err(AudioError::NoSupportedFormat)
        ));
    }

    #[test]
    fn seek_conversion_round_trips_within_one_frame() {
        for rate in [44_100, 48_000, 96_000] {
            let frames = ms_to_frames(1_234, rate);
            let back = frames_to_ms(frames, rate);
            assert!(back.abs_diff(1_234) <= 1, "rate {rate}: {back} ms");
        }
    }

    #[test]
    fn seek_conversion_handles_zero_rate() {
        assert_eq!(ms_to_frames(1_000, 0), 0);
        assert_eq!(frames_to_ms(1_000, 0), 0);
    }
}
