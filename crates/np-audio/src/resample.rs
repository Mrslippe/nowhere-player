//! Processing decisions and sample-rate conversion.
//!
//! [`ProcessingPlan`] is the bit-perfect switchboard from ARCH-006 §3: when both
//! flags are `false` the samples go from decoder to device untouched.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, WindowFunction};

use crate::error::AudioError;
use crate::format::AudioFormat;

/// Resampler chunk size in frames. 1024 frames ≈ 21 ms at 48 kHz, small enough
/// that seek/stop only throw away a few milliseconds of work.
const CHUNK_FRAMES: usize = 1024;

/// Who owns the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainPolicy {
    /// Apply the `v^3` curve in software (v1 shared mode).
    Software,
    /// Leave samples untouched and let hardware/system volume do the work
    /// (reserved for exclusive / bit-perfect backends).
    Hardware,
}

/// What the pipeline must do for one source → device pair.
///
/// Both flags `false` means bit-perfect pass-through: no resampling and no
/// software gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessingPlan {
    pub resample: bool,
    pub gain: bool,
}

impl ProcessingPlan {
    /// Plan for the v1 shared-mode sink (software volume).
    pub fn compute(source: AudioFormat, output: AudioFormat) -> Self {
        Self::with_gain_policy(source, output, GainPolicy::Software)
    }

    /// Plan with an explicit volume owner.
    ///
    /// Channel-count conversion is a separate concern handled by
    /// [`map_channels`]; `resample` only describes sample-rate conversion.
    pub fn with_gain_policy(
        source: AudioFormat,
        output: AudioFormat,
        gain_policy: GainPolicy,
    ) -> Self {
        Self {
            resample: source.sample_rate != output.sample_rate && source.channels > 0,
            gain: gain_policy == GainPolicy::Software,
        }
    }

    /// `true` when the device receives exactly the decoded samples.
    pub fn is_bit_perfect(&self) -> bool {
        !self.resample && !self.gain
    }
}

/// Maps an interleaved buffer from `in_channels` to `out_channels`.
///
/// Mono is duplicated, multichannel is averaged down, extra output channels are
/// filled with silence. This runs on the decode thread, so allocation is fine.
pub fn map_channels(input: &[f32], in_channels: usize, out_channels: usize, out: &mut Vec<f32>) {
    if in_channels == 0 || out_channels == 0 {
        return;
    }
    if in_channels == out_channels {
        out.extend_from_slice(input);
        return;
    }
    let frames = input.len() / in_channels;
    out.reserve(frames * out_channels);
    if in_channels == 1 {
        for &sample in input {
            out.extend(std::iter::repeat_n(sample, out_channels));
        }
    } else if out_channels == 1 {
        for frame in input.chunks_exact(in_channels) {
            let sum: f32 = frame.iter().sum();
            out.push(sum / in_channels as f32);
        }
    } else if out_channels < in_channels {
        for frame in input.chunks_exact(in_channels) {
            out.extend_from_slice(&frame[..out_channels]);
        }
    } else {
        for frame in input.chunks_exact(in_channels) {
            out.extend_from_slice(frame);
            out.extend(std::iter::repeat_n(0.0, out_channels - in_channels));
        }
    }
}

/// Streaming sinc resampler (rubato).
///
/// Owned by the decode thread. Input and output are interleaved f32; rubato's
/// adapters wrap the slices directly, so no de-interleaving copies are needed.
pub(crate) struct StreamResampler {
    inner: Async<f32>,
    channels: usize,
    ratio: f64,
    pending: Vec<f32>,
    scratch: Vec<f32>,
}

impl StreamResampler {
    /// Creates a resampler for `input` → `output`. Channel counts must match.
    pub fn new(input: AudioFormat, output: AudioFormat) -> Result<Self, AudioError> {
        if input.channels != output.channels {
            return Err(AudioError::InvalidArgument(
                "resampler requires equal channel counts",
            ));
        }
        let channels = usize::from(output.channels);
        let ratio = f64::from(output.sample_rate) / f64::from(input.sample_rate.max(1));
        let parameters = SincInterpolationParameters::new(256, WindowFunction::BlackmanHarris2);
        let inner = Async::<f32>::new_sinc(
            ratio,
            1.0,
            &parameters,
            CHUNK_FRAMES,
            channels,
            FixedAsync::Input,
        )
        .map_err(resample_error)?;
        Ok(Self {
            inner,
            channels,
            ratio,
            pending: Vec::new(),
            scratch: Vec::new(),
        })
    }

    /// Feeds interleaved samples and appends the resampled frames to `output`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), AudioError> {
        debug_assert_eq!(input.len() % self.channels, 0);
        self.pending.extend_from_slice(input);
        loop {
            let needed_frames = self.inner.input_frames_next();
            let needed = needed_frames * self.channels;
            if self.pending.len() < needed {
                break;
            }
            let out_frames = self.inner.output_frames_next();
            self.scratch.resize(out_frames * self.channels, 0.0);
            let (consumed, produced) = {
                let input_adapter =
                    InterleavedSlice::new(&self.pending[..needed], self.channels, needed_frames)
                        .map_err(resample_error)?;
                let mut output_adapter =
                    InterleavedSlice::new_mut(&mut self.scratch, self.channels, out_frames)
                        .map_err(resample_error)?;
                self.inner
                    .process_into_buffer(&input_adapter, &mut output_adapter, None)
                    .map_err(resample_error)?
            };
            output.extend_from_slice(&self.scratch[..produced * self.channels]);
            self.pending.drain(..consumed * self.channels);
        }
        Ok(())
    }

    /// Resamples the remaining (< one chunk) frames at end of stream.
    pub fn flush(&mut self, output: &mut Vec<f32>) -> Result<(), AudioError> {
        let frames = self.pending.len() / self.channels;
        if frames == 0 {
            return Ok(());
        }
        let out_frames = self.inner.output_frames_next();
        self.scratch.resize(out_frames * self.channels, 0.0);
        let produced = {
            let input_adapter = InterleavedSlice::new(&self.pending, self.channels, frames)
                .map_err(resample_error)?;
            let mut output_adapter =
                InterleavedSlice::new_mut(&mut self.scratch, self.channels, out_frames)
                    .map_err(resample_error)?;
            let indexing = Indexing::new().partial_len(frames);
            self.inner
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
                .map_err(resample_error)?
                .1
        };
        let expected = ((frames as f64) * self.ratio).round().max(0.0) as usize;
        let take = expected.min(produced).min(out_frames);
        output.extend_from_slice(&self.scratch[..take * self.channels]);
        self.pending.clear();
        Ok(())
    }

    /// Drops pending input; called after a seek.
    pub fn reset(&mut self) {
        self.inner.reset();
        self.pending.clear();
    }
}

fn resample_error(error: impl std::fmt::Display) -> AudioError {
    AudioError::Resample(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::SampleFormat;

    fn format(rate: u32, channels: u16) -> AudioFormat {
        AudioFormat::new(rate, channels, SampleFormat::F32)
    }

    #[test]
    fn plan_keeps_bit_perfect_path_open() {
        let source = format(48_000, 2);

        let shared = ProcessingPlan::compute(source, source);
        assert!(!shared.resample);
        assert!(shared.gain);
        assert!(!shared.is_bit_perfect());

        let exclusive = ProcessingPlan::with_gain_policy(source, source, GainPolicy::Hardware);
        assert!(!exclusive.resample);
        assert!(!exclusive.gain);
        assert!(exclusive.is_bit_perfect());
    }

    #[test]
    fn plan_detects_sample_rate_mismatch() {
        let plan = ProcessingPlan::compute(format(44_100, 2), format(48_000, 2));
        assert!(plan.resample);
        assert!(plan.gain);
        assert!(!plan.is_bit_perfect());
    }

    #[test]
    fn channels_are_duplicated_and_downmixed() {
        let mut stereo = Vec::new();
        map_channels(&[1.0, 2.0], 1, 2, &mut stereo);
        assert_eq!(stereo, [1.0, 1.0, 2.0, 2.0]);

        let mut mono = Vec::new();
        map_channels(&[1.0, 3.0, 2.0, 4.0], 2, 1, &mut mono);
        assert_eq!(mono, [2.0, 3.0]);
    }

    #[test]
    fn resampler_produces_ratio_many_frames() {
        let input = format(44_100, 2);
        let output = format(48_000, 2);
        let mut resampler = StreamResampler::new(input, output).unwrap();
        let mut out = Vec::new();
        let mut phase = 0.0f32;
        let mut fed = 0;
        // Feed 10 chunks of 441 frames in irregular pieces.
        for _ in 0..40 {
            let frames = 110;
            let mut chunk = Vec::with_capacity(frames * 2);
            for _ in 0..frames {
                let value = phase.sin() * 0.5;
                phase += std::f32::consts::TAU * 440.0 / 44_100.0;
                chunk.push(value);
                chunk.push(value);
            }
            resampler.process(&chunk, &mut out).unwrap();
            fed += frames;
        }
        resampler.flush(&mut out).unwrap();
        let produced = out.len() / 2;
        let expected = (fed as f64 * 48_000.0 / 44_100.0) as usize;
        assert!(
            produced.abs_diff(expected) < expected / 20,
            "produced {produced}, expected ≈ {expected}"
        );
        assert!(out.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn resampling_does_not_shift_pitch() {
        let mut resampler = StreamResampler::new(format(44_100, 2), format(48_000, 2)).unwrap();
        let frames = 44_100 / 2; // 0.5 s
        let mut input = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let value = (std::f32::consts::TAU * 440.0 * frame as f32 / 44_100.0).sin() * 0.5;
            input.push(value);
            input.push(value);
        }
        let mut out = Vec::new();
        for chunk in input.chunks(441 * 2) {
            resampler.process(chunk, &mut out).unwrap();
        }
        resampler.flush(&mut out).unwrap();

        // Skip the filter delay at both ends, then count sign changes.
        let (frames, _) = out.as_chunks::<2>();
        let left: Vec<f32> = frames.iter().map(|frame| frame[0]).collect();
        let body = &left[2_000..left.len() - 2_000];
        let crossings = body
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        let seconds = body.len() as f64 / 48_000.0;
        let measured = crossings as f64 / (2.0 * seconds);
        assert!(
            (measured - 440.0).abs() < 4.4,
            "pitch shifted: {measured} Hz"
        );
        assert_eq!(left.len(), out.len() / 2);
    }
}
