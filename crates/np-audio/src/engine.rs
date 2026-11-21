//! Playback engine: device + decode thread + lock-free hand-off.
//!
//! Thread layout (ARCH-006 §4):
//!
//! - this thread (caller / actor): control, never touches audio data;
//! - `np-audio-decode`: blocking decode + optional resample, fills the ring;
//! - cpal's audio thread: runs [`Renderer::render`], pops the ring and applies
//!   the gain envelope. It only touches atomics, a stack buffer and the ring.
//!
//! Seeking is a four-step handshake so that no stale sample can leak into the
//! new position:
//!
//! 1. control bumps `epoch` and publishes the target;
//! 2. the renderer fades out, drains the ring and sets `drained_epoch`;
//! 3. the decoder waits for `drained_epoch`, seeks, sets `seeked_epoch`;
//! 4. the renderer drains once more (dropping pushes that were in flight) and
//!    sets `ready_epoch`; only then does the decoder push new samples.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::AudioError;
use crate::format::{AudioFormat, DeviceCaps, frames_to_ms, negotiate};
use crate::resample::{ProcessingPlan, StreamResampler, map_channels};
use crate::ring::{self, RingConsumer, RingProducer};
use crate::sink::{AudioSink, CpalSharedSink};
use crate::source::{AudioSource, FileSource};
use crate::volume::gain_for_volume;

/// Ring buffer target: 300 ms at the output rate.
///
/// ARCH-006 left 200–500 ms open; 300 ms keeps a comfortable cushion against
/// decode-thread scheduling jitter while keeping startup buffering short.
const RING_MS: u32 = 300;

/// Default ring used by [`crate::Player::open_source`] (matches local files).
pub(crate) const DEFAULT_RING_MS: u32 = RING_MS;

/// Frames decoded per source read. 1024 frames ≈ 21 ms at 48 kHz.
const READ_FRAMES: usize = 1024;

/// Fade ramp for play/pause/seek/stop (ARCH-006/PLAN-001: 8–15 ms).
const FADE_MS: u32 = 12;

/// How long the decoder waits for a renderer handshake before giving up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

/// Ring capacity in interleaved samples for a requested buffering window.
fn ring_capacity_samples(format: AudioFormat, ring_ms: u32) -> usize {
    let frames = (u64::from(format.sample_rate) * u64::from(ring_ms) / 1000).max(1) as usize;
    frames * format.frame_samples()
}

/// State shared between control, decode thread and audio callback.
pub(crate) struct Shared {
    playing: AtomicBool,
    /// Linear gain (`v^3`), stored as f32 bits.
    gain: AtomicU32,
    /// Output mute after the sample tap, so the visualizer keeps its signal.
    output_muted: AtomicBool,
    /// Frames handed to the device since the last seek.
    played_frames: AtomicU64,
    /// Position of the first frame after the last seek.
    base_ms: AtomicU64,
    /// Decoder reached end of file.
    ended: AtomicBool,
    /// The ring was played out after `ended`.
    drained: AtomicBool,
    shutdown: AtomicBool,
    underruns: AtomicU64,

    /// Seek generation. Bumped by control.
    epoch: AtomicU64,
    /// Renderer finished fading out and drained the ring.
    drained_epoch: AtomicU64,
    /// Decoder finished seeking to the current epoch.
    seeked_epoch: AtomicU64,
    /// Renderer drained once more; the decoder may now push new samples.
    ready_epoch: AtomicU64,
    /// Latest seek target in milliseconds.
    seek_request: Mutex<Option<u64>>,

    error: Mutex<Option<String>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            gain: AtomicU32::new(1.0f32.to_bits()),
            output_muted: AtomicBool::new(false),
            played_frames: AtomicU64::new(0),
            base_ms: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            drained_epoch: AtomicU64::new(0),
            seeked_epoch: AtomicU64::new(0),
            ready_epoch: AtomicU64::new(0),
            seek_request: Mutex::new(None),
            error: Mutex::new(None),
        }
    }

    fn set_error(&self, error: &AudioError) {
        if let Ok(mut slot) = self.error.lock() {
            slot.get_or_insert_with(|| error.to_string());
        }
    }
}

/// Fills the device buffer from the ring and applies the gain envelope.
///
/// Runs on the audio thread: no allocation, no locks, no formatting.
pub(crate) struct Renderer {
    consumer: RingConsumer,
    shared: Arc<Shared>,
    channels: usize,
    /// Gain change per frame for a 12 ms full-scale ramp.
    step: f32,
    /// Apply the `v^3` software volume (false = hardware owns the volume).
    software_gain: bool,
    /// Fade envelope only (pause / seek / fade-in); tap sees this.
    gain: f32,
    /// Software volume envelope, applied to the device buffer after the tap.
    volume_gain: f32,
    /// Last seek generation that was fully resumed.
    handled_epoch: u64,
    /// Seek generation currently fading out / waiting for the decoder.
    fade_epoch: Option<u64>,
    tap: Option<crate::tap::SampleTapWriter>,
}

impl Renderer {
    fn new(
        consumer: RingConsumer,
        shared: Arc<Shared>,
        channels: usize,
        sample_rate: u32,
        software_gain: bool,
        tap: Option<crate::tap::SampleTapWriter>,
    ) -> Self {
        let fade_frames = sample_rate as f32 * FADE_MS as f32 / 1000.0;
        let volume_gain = if software_gain {
            f32::from_bits(shared.gain.load(Ordering::Relaxed))
        } else {
            1.0
        };
        Self {
            consumer,
            shared,
            channels,
            step: 1.0 / fade_frames,
            software_gain,
            gain: 0.0,
            volume_gain,
            handled_epoch: 0,
            fade_epoch: None,
            tap,
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        self.render_inner(out);
        // Tap before software volume: visualizers must not change with volume.
        // Fade is already applied, so pause/seek still visibly decays.
        if let Some(tap) = &self.tap {
            tap.write_interleaved(out, self.channels);
        }
        if self.software_gain {
            let target = f32::from_bits(self.shared.gain.load(Ordering::Relaxed));
            for frame in 0..out.len() / self.channels {
                let start = frame * self.channels;
                for sample in &mut out[start..start + self.channels] {
                    *sample *= self.volume_gain;
                }
                self.volume_gain = next_gain(self.volume_gain, target, self.step);
            }
        }
        if self.shared.output_muted.load(Ordering::Acquire) {
            out.fill(0.0);
        }
    }

    fn render_inner(&mut self, out: &mut [f32]) {
        let frames = out.len() / self.channels;
        if frames == 0 {
            return;
        }
        let latest = self.shared.epoch.load(Ordering::Acquire);
        if latest != self.handled_epoch {
            self.fade_epoch = Some(latest);
        }
        match self.fade_epoch {
            Some(epoch) => self.render_seek(out, epoch, frames),
            None => self.render_playback(out, frames),
        }
    }

    /// Fades old audio out, drains the ring, and waits for the decoder.
    fn render_seek(&mut self, out: &mut [f32], epoch: u64, frames: usize) {
        let channels = self.channels;
        if self.gain > 0.0 {
            let pop_frames = ((self.gain / self.step).ceil() as usize).clamp(1, frames);
            let written = self.consumer.pop(&mut out[..pop_frames * channels]);
            let written_frames = written / channels;
            for frame in 0..written_frames {
                self.apply_frame(out, frame);
                self.gain = next_gain(self.gain, 0.0, self.step);
            }
            out[written_frames * channels..].fill(0.0);
            self.shared
                .played_frames
                .fetch_add(written_frames as u64, Ordering::Relaxed);
            // If the ring ran dry mid-fade, finish the ramp through silence.
            let mut frame = written_frames;
            while self.gain > 0.0 && frame < frames {
                self.gain = next_gain(self.gain, 0.0, self.step);
                frame += 1;
            }
            if self.gain <= 0.0 {
                self.consumer.clear();
                self.shared.played_frames.store(0, Ordering::Relaxed);
                self.shared.drained_epoch.store(epoch, Ordering::Relaxed);
            }
        } else if self.shared.drained_epoch.load(Ordering::Relaxed) < epoch {
            self.consumer.clear();
            self.shared.played_frames.store(0, Ordering::Relaxed);
            self.shared.drained_epoch.store(epoch, Ordering::Relaxed);
        } else {
            out.fill(0.0);
        }

        // The decoder has seeked; drop anything that was still in flight and
        // open the gate for new samples.
        if self.shared.seeked_epoch.load(Ordering::Acquire) >= epoch {
            self.consumer.clear();
            self.shared.played_frames.store(0, Ordering::Relaxed);
            self.shared.ready_epoch.store(epoch, Ordering::Release);
            self.handled_epoch = epoch;
            self.fade_epoch = None;
            self.gain = 0.0;
            out.fill(0.0);
        }
    }

    /// Normal playback with fade-in, volume ramps and pause fade-out.
    fn render_playback(&mut self, out: &mut [f32], frames: usize) {
        let shared = &self.shared;
        let playing = shared.playing.load(Ordering::Relaxed);
        // `gain` is the fade envelope only; software volume is applied after
        // the tap in `render`.
        let target = if playing { 1.0 } else { 0.0 };
        if !playing && self.gain <= 0.0 {
            out.fill(0.0);
            return;
        }

        // While pausing, only consume the frames needed for the ramp so the
        // rest of the buffered audio is still there on resume.
        let pop_frames = if playing {
            frames
        } else {
            ((self.gain / self.step).ceil() as usize).clamp(1, frames)
        };
        let written = self.consumer.pop(&mut out[..pop_frames * self.channels]);
        let written_frames = written / self.channels;
        for frame in 0..written_frames {
            self.apply_frame(out, frame);
            self.gain = next_gain(self.gain, target, self.step);
        }
        out[written_frames * self.channels..].fill(0.0);
        shared
            .played_frames
            .fetch_add(written_frames as u64, Ordering::Relaxed);

        if !playing {
            let mut frame = written_frames;
            while self.gain > 0.0 && frame < frames {
                self.gain = next_gain(self.gain, 0.0, self.step);
                frame += 1;
            }
        }

        let ended = shared.ended.load(Ordering::Relaxed);
        if written_frames < frames
            && playing
            && target > 0.0
            && !ended
            && shared.played_frames.load(Ordering::Relaxed) > written_frames as u64
        {
            shared.underruns.fetch_add(1, Ordering::Relaxed);
        }
        if ended && written_frames == 0 {
            shared.drained.store(true, Ordering::Relaxed);
        }
    }

    fn apply_frame(&self, out: &mut [f32], frame: usize) {
        let start = frame * self.channels;
        for sample in &mut out[start..start + self.channels] {
            *sample *= self.gain;
        }
    }
}

/// Moves `gain` toward `target` by at most `step`.
fn next_gain(gain: f32, target: f32, step: f32) -> f32 {
    if gain < target {
        (gain + step).min(target)
    } else if gain > target {
        (gain - step).max(target)
    } else {
        gain
    }
}

/// Owns one opened track: sink, decode thread and their shared state.
pub(crate) struct Engine {
    shared: Arc<Shared>,
    decode: Option<JoinHandle<()>>,
    sink: Box<dyn AudioSink>,
    source_format: AudioFormat,
    output_format: AudioFormat,
    plan: ProcessingPlan,
    duration_ms: Option<u64>,
    seekable: bool,
    caps: DeviceCaps,
    device_name: String,
}

impl Engine {
    pub fn start(path: &Path, tap_writer: crate::tap::SampleTapWriter) -> Result<Self, AudioError> {
        let source = Box::new(FileSource::open(path)?);
        Self::start_source(source, RING_MS, tap_writer)
    }

    /// Starts a decode thread over an arbitrary source and a caller-chosen ring.
    ///
    /// Live streams pass a shorter ring (150 ms) so playback starts quickly and
    /// the listener stays closer to the live edge.
    pub fn start_source(
        source: Box<dyn AudioSource>,
        ring_ms: u32,
        tap_writer: crate::tap::SampleTapWriter,
    ) -> Result<Self, AudioError> {
        if ring_ms == 0 {
            return Err(AudioError::InvalidArgument("ring_ms must be > 0"));
        }
        let source_format = source.format();
        let duration_ms = source.duration_ms();
        let seekable = source.is_seekable();

        let cpal_sink = CpalSharedSink::new()?;
        let device_name = cpal_sink.name().to_owned();
        let caps = cpal_sink.device_caps()?;
        let output_format = negotiate(source_format, &caps)?;
        let plan = ProcessingPlan::compute(source_format, output_format);

        let capacity = ring_capacity_samples(output_format, ring_ms);
        let (producer, consumer) = ring::spsc(capacity);

        let shared = Arc::new(Shared::new());
        let mut renderer = Renderer::new(
            consumer,
            Arc::clone(&shared),
            output_format.frame_samples(),
            output_format.sample_rate,
            plan.gain,
            Some(tap_writer),
        );
        let mut sink: Box<dyn AudioSink> = Box::new(cpal_sink);
        sink.open(
            output_format,
            Box::new(move |out: &mut [f32]| renderer.render(out)),
        )?;

        let decode = spawn_decode(source, plan, output_format, producer, Arc::clone(&shared))?;

        Ok(Self {
            shared,
            decode: Some(decode),
            sink,
            source_format,
            output_format,
            plan,
            duration_ms,
            seekable,
            caps,
            device_name,
        })
    }

    pub fn play(&self) -> Result<(), AudioError> {
        self.check_device()?;
        self.shared.playing.store(true, Ordering::Release);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), AudioError> {
        self.check_device()?;
        self.shared.playing.store(false, Ordering::Release);
        Ok(())
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), AudioError> {
        self.check_device()?;
        if !self.seekable {
            return Err(AudioError::Seek("source is not seekable".to_owned()));
        }
        let target = match self.duration_ms {
            Some(duration) => position_ms.min(duration),
            None => position_ms,
        };
        if let Ok(mut slot) = self.shared.seek_request.lock() {
            *slot = Some(target);
        }
        self.shared.base_ms.store(target, Ordering::Release);
        self.shared.played_frames.store(0, Ordering::Relaxed);
        self.shared.ended.store(false, Ordering::Relaxed);
        self.shared.drained.store(false, Ordering::Relaxed);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), AudioError> {
        self.pause()?;
        if self.seekable { self.seek(0) } else { Ok(()) }
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .gain
            .store(gain_for_volume(volume).to_bits(), Ordering::Release);
    }

    /// 视频模式静音输出，但保留 sample tap 给可视化。
    pub fn set_output_muted(&self, muted: bool) {
        self.shared.output_muted.store(muted, Ordering::Release);
    }

    pub fn position_ms(&self) -> u64 {
        let played = self.shared.played_frames.load(Ordering::Acquire);
        let position = self
            .shared
            .base_ms
            .load(Ordering::Acquire)
            .saturating_add(frames_to_ms(played, self.output_format.sample_rate));
        match self.duration_ms {
            Some(duration) => position.min(duration),
            None => position,
        }
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    pub fn source_format(&self) -> AudioFormat {
        self.source_format
    }

    pub fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    pub fn plan(&self) -> ProcessingPlan {
        self.plan
    }

    pub fn device_caps(&self) -> DeviceCaps {
        self.caps.clone()
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn is_ended(&self) -> bool {
        self.shared.ended.load(Ordering::Acquire) && self.shared.drained.load(Ordering::Acquire)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.shared.error.lock().ok().and_then(|slot| slot.clone())
    }

    /// 消费一次「输出流在运行中报错」的标志。
    ///
    /// 设备热插拔检测用（T-0016 §2.1）：错误回调只翻转一个原子，
    /// 这里把它转成一次布尔探测，不修改 [`AudioSink`] trait 的形状。
    pub fn take_device_error(&self) -> bool {
        self.sink.take_device_error()
    }

    fn check_device(&self) -> Result<(), AudioError> {
        if self.sink.take_device_error() {
            Err(AudioError::DeviceLost)
        } else {
            Ok(())
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.sink.close();
        if let Some(handle) = self.decode.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_decode(
    source: Box<dyn AudioSource>,
    plan: ProcessingPlan,
    output_format: AudioFormat,
    producer: RingProducer,
    shared: Arc<Shared>,
) -> Result<JoinHandle<()>, AudioError> {
    thread::Builder::new()
        .name("np-audio-decode".into())
        .spawn(move || {
            if let Err(error) = decode_loop(source, plan, output_format, producer, &shared) {
                shared.set_error(&error);
                shared.ended.store(true, Ordering::Release);
            }
        })
        .map_err(AudioError::Io)
}

fn decode_loop(
    mut source: Box<dyn AudioSource>,
    plan: ProcessingPlan,
    output_format: AudioFormat,
    mut producer: RingProducer,
    shared: &Shared,
) -> Result<(), AudioError> {
    let source_format = source.format();
    let in_channels = usize::from(source_format.channels);
    let out_channels = usize::from(output_format.channels);
    let mut resampler = if plan.resample {
        Some(StreamResampler::new(source_format, output_format)?)
    } else {
        None
    };

    let mut decoded = vec![0.0f32; READ_FRAMES * in_channels];
    let mut mapped = Vec::with_capacity(READ_FRAMES * out_channels);
    let mut converted = Vec::new();
    let mut handled_epoch = 0u64;
    let mut at_eof = false;

    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }

        let epoch = shared.epoch.load(Ordering::Acquire);
        if epoch != handled_epoch {
            let target = shared
                .seek_request
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .unwrap_or(0);
            wait_for(shared, epoch, |shared| {
                shared.drained_epoch.load(Ordering::Acquire)
            });
            let actual = source.seek(target)?;
            if let Some(resampler) = resampler.as_mut() {
                resampler.reset();
            }
            if shared.epoch.load(Ordering::Acquire) == epoch {
                shared.base_ms.store(actual, Ordering::Release);
                shared.seeked_epoch.store(epoch, Ordering::Release);
                wait_for(shared, epoch, |shared| {
                    shared.ready_epoch.load(Ordering::Acquire)
                });
            }
            handled_epoch = epoch;
            at_eof = false;
            continue;
        }

        if !shared.playing.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(3));
            continue;
        }
        if at_eof {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        // Keep at least a few callback blocks of headroom before decoding more.
        if producer.vacant() < out_channels * 256 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        let frames = source.read_frames(&mut decoded)?;
        mapped.clear();
        converted.clear();
        if frames == 0 {
            if let Some(resampler) = resampler.as_mut() {
                resampler.flush(&mut converted)?;
            }
            if push_all(&mut producer, &converted, shared, epoch)? {
                shared.ended.store(true, Ordering::Release);
                at_eof = true;
            }
            continue;
        }

        map_channels(
            &decoded[..frames * in_channels],
            in_channels,
            out_channels,
            &mut mapped,
        );
        match resampler.as_mut() {
            Some(resampler) => resampler.process(&mapped, &mut converted)?,
            None => converted.extend_from_slice(&mapped),
        }
        let _ = push_all(&mut producer, &converted, shared, epoch)?;
        // If a seek invalidated the pushed samples, the next iteration handles
        // the new epoch before decoding again.
    }
}

fn wait_for(shared: &Shared, epoch: u64, read: impl Fn(&Shared) -> u64) {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while read(shared) < epoch {
        if shared.shutdown.load(Ordering::Acquire) || Instant::now() >= deadline {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// Pushes all samples, waiting for the consumer when the ring is full.
///
/// Returns `false` when a seek happened while pushing; the caller must not
/// continue with the old stream.
fn push_all(
    producer: &mut RingProducer,
    samples: &[f32],
    shared: &Shared,
    epoch: u64,
) -> Result<bool, AudioError> {
    let mut offset = 0;
    while offset < samples.len() {
        if shared.shutdown.load(Ordering::Acquire) || shared.epoch.load(Ordering::Acquire) != epoch
        {
            return Ok(false);
        }
        let pushed = producer.push(&samples[offset..]);
        if pushed == 0 {
            thread::sleep(Duration::from_millis(1));
        } else {
            offset += pushed;
        }
    }
    Ok(shared.epoch.load(Ordering::Acquire) == epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const CHANNELS: usize = 2;
    /// 12 ms at 48 kHz.
    const FADE_FRAMES: usize = 576;

    fn renderer(capacity_frames: usize) -> (Renderer, RingProducer, Arc<Shared>) {
        renderer_with_gain(capacity_frames, true)
    }

    fn renderer_with_gain(
        capacity_frames: usize,
        software_gain: bool,
    ) -> (Renderer, RingProducer, Arc<Shared>) {
        let (producer, consumer) = ring::spsc(capacity_frames * CHANNELS);
        let shared = Arc::new(Shared::new());
        let renderer = Renderer::new(
            consumer,
            Arc::clone(&shared),
            CHANNELS,
            RATE,
            software_gain,
            None,
        );
        (renderer, producer, shared)
    }

    fn renderer_with_tap(
        capacity_frames: usize,
    ) -> (
        Renderer,
        RingProducer,
        Arc<Shared>,
        crate::tap::SampleTapHandle,
    ) {
        let (producer, consumer) = ring::spsc(capacity_frames * CHANNELS);
        let shared = Arc::new(Shared::new());
        let (tap_writer, tap) = crate::tap::create(RATE);
        let renderer = Renderer::new(
            consumer,
            Arc::clone(&shared),
            CHANNELS,
            RATE,
            true,
            Some(tap_writer),
        );
        (renderer, producer, shared, tap)
    }

    fn push(producer: &mut RingProducer, frames: usize) {
        let ones = vec![1.0f32; frames * CHANNELS];
        assert_eq!(producer.push(&ones), ones.len());
    }

    fn max_step(samples: &[f32]) -> f32 {
        let (frames, _) = samples.as_chunks::<CHANNELS>();
        let mut frames = frames.iter().map(|frame| frame[0]);
        let mut previous = frames.next().unwrap_or(0.0);
        let mut max = 0.0f32;
        for value in frames {
            max = max.max((value - previous).abs());
            previous = value;
        }
        max
    }

    #[test]
    fn ring_capacity_scales_with_ring_ms() {
        let format = AudioFormat::new(48_000, 2, crate::format::SampleFormat::F32);
        assert_eq!(ring_capacity_samples(format, 150), 14_400);
        assert_eq!(ring_capacity_samples(format, 300), 28_800);
    }

    #[test]
    fn fade_in_starts_at_zero_and_reaches_unity() {
        let (mut renderer, mut producer, shared) = renderer(4096);
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);

        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);

        assert_eq!(out[0], 0.0);
        assert!(out[CHANNELS] > 0.0 && out[CHANNELS] < 0.01);
        let last = out[699 * CHANNELS];
        assert!((last - 1.0).abs() < 1e-6, "last frame gain {last}");
        assert!(max_step(&out) <= 1.0 / FADE_FRAMES as f32 + 1e-6);
    }

    #[test]
    fn output_mute_zeroes_the_device_buffer() {
        let (mut renderer, mut producer, shared) = renderer(4096);
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);
        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);
        assert!(out.iter().any(|sample| *sample != 0.0));

        shared.output_muted.store(true, Ordering::Release);
        out.fill(0.0);
        renderer.render(&mut out);
        assert!(
            out.iter().all(|sample| *sample == 0.0),
            "muted output must be silent"
        );
    }

    #[test]
    fn output_mute_keeps_the_visualizer_tap_alive() {
        let (mut producer, consumer) = ring::spsc(4096 * CHANNELS);
        let shared = Arc::new(Shared::new());
        let (tap_writer, tap) = crate::tap::create(RATE);
        let mut renderer = Renderer::new(
            consumer,
            Arc::clone(&shared),
            CHANNELS,
            RATE,
            true,
            Some(tap_writer),
        );
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);
        shared.output_muted.store(true, Ordering::Release);

        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        let mut tap_out = [0.0f32; 700];
        tap.latest(&mut tap_out);
        assert!(
            tap_out.iter().any(|sample| *sample != 0.0),
            "mute must not zero the sample tap"
        );
    }

    #[test]
    fn pause_fades_out_and_stops_consuming() {
        let (mut renderer, mut producer, shared) = renderer(4096);
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);
        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);

        shared.playing.store(false, Ordering::Release);
        push(&mut producer, 2000);
        let played_before = shared.played_frames.load(Ordering::Relaxed);
        out.fill(0.0);
        renderer.render(&mut out);

        let during = shared.played_frames.load(Ordering::Relaxed);
        assert_eq!(during - played_before, FADE_FRAMES as u64);
        // f32 accumulation drifts a few ppm; the ramp is still monotonic.
        assert!(
            max_step(&out) <= 1.0 / FADE_FRAMES as f32 + 1e-5,
            "max step {}",
            max_step(&out)
        );

        // Once silent, paused callbacks must not consume the ring.
        posterity_ring_is_intact(&mut producer);
        out.fill(0.0);
        renderer.render(&mut out);
        assert_eq!(shared.played_frames.load(Ordering::Relaxed), during);
        assert!(out.iter().all(|sample| *sample == 0.0));
    }

    fn posterity_ring_is_intact(producer: &mut RingProducer) {
        // 2000 - 700 (play) + 2000 - 576 (pause fade) = 2724 frames left.
        assert_eq!(producer.occupied(), 2724 * CHANNELS);
    }

    #[test]
    fn tap_is_pre_volume_while_output_is_post_volume() {
        let (mut loud, mut loud_producer, loud_shared, loud_tap) = renderer_with_tap(4096);
        let (mut quiet, mut quiet_producer, quiet_shared, quiet_tap) = renderer_with_tap(4096);
        push(&mut loud_producer, 2000);
        push(&mut quiet_producer, 2000);
        loud_shared.playing.store(true, Ordering::Release);
        quiet_shared.playing.store(true, Ordering::Release);
        quiet_shared
            .gain
            .store(gain_for_volume(0.25).to_bits(), Ordering::Release);

        // Let fade-in and the volume ramp settle before measuring peaks.
        let mut warm = vec![0.0f32; 700 * CHANNELS];
        loud.render(&mut warm);
        quiet.render(&mut warm);

        let mut loud_out = vec![0.0f32; 700 * CHANNELS];
        let mut quiet_out = vec![0.0f32; 700 * CHANNELS];
        loud.render(&mut loud_out);
        quiet.render(&mut quiet_out);

        let out_peak = |samples: &[f32]| {
            samples
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
        };
        let loud_peak = out_peak(&loud_out);
        let quiet_peak = out_peak(&quiet_out);
        assert!(loud_peak > 0.0);
        assert!(
            quiet_peak < loud_peak * 0.1,
            "software volume must reach the output: {quiet_peak} vs {loud_peak}"
        );

        let mut loud_tap_samples = vec![0.0f32; 700];
        let mut quiet_tap_samples = vec![0.0f32; 700];
        loud_tap.latest(&mut loud_tap_samples);
        quiet_tap.latest(&mut quiet_tap_samples);
        let tap_peak = out_peak(&loud_tap_samples);
        assert!(tap_peak > 0.0);
        assert!(
            (out_peak(&quiet_tap_samples) - tap_peak).abs() < 1.0e-6,
            "tap must not include software volume"
        );
    }

    #[test]
    fn mute_volume_keeps_the_tap_alive() {
        let (mut renderer, mut producer, shared, tap) = renderer_with_tap(4096);
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);
        shared.gain.store(0.0f32.to_bits(), Ordering::Release);
        shared.output_muted.store(true, Ordering::Release);

        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        let mut tap_out = vec![0.0f32; 700];
        tap.latest(&mut tap_out);
        assert!(tap_out.iter().any(|sample| *sample != 0.0));
    }

    #[test]
    fn hardware_volume_bypasses_software_gain() {
        let (mut renderer, mut producer, shared) = renderer_with_gain(4096, false);
        push(&mut producer, 1000);
        shared.gain.store(0.0f32.to_bits(), Ordering::Release);
        shared.playing.store(true, Ordering::Release);

        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);
        let last = out[699 * CHANNELS];
        assert!((last - 1.0).abs() < 1e-6, "software volume leaked: {last}");
    }

    #[test]
    fn scripted_transport_has_no_sample_jumps() {
        let (mut renderer, mut producer, shared) = renderer(8192);
        let mut captured: Vec<f32> = Vec::new();
        let mut out = vec![0.0f32; 700 * CHANNELS];

        push(&mut producer, 4000);
        shared.playing.store(true, Ordering::Release);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        shared.playing.store(false, Ordering::Release);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        shared.playing.store(true, Ordering::Release);
        push(&mut producer, 2000);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        if let Ok(mut slot) = shared.seek_request.lock() {
            *slot = Some(1_000);
        }
        shared.epoch.fetch_add(1, Ordering::AcqRel);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        shared.seeked_epoch.store(1, Ordering::Release);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        push(&mut producer, 2000);
        renderer.render(&mut out);
        captured.extend_from_slice(&out);

        assert!(captured.iter().all(|sample| sample.is_finite()));
        let jump = max_step(&captured);
        assert!(
            jump <= 1.0 / FADE_FRAMES as f32 + 1e-5,
            "transport produced a {jump} jump"
        );
    }

    #[test]
    fn seek_handshake_drains_and_opens_the_gate() {
        let (mut renderer, mut producer, shared) = renderer(4096);
        push(&mut producer, 2000);
        shared.playing.store(true, Ordering::Release);
        let mut out = vec![0.0f32; 700 * CHANNELS];
        renderer.render(&mut out);
        assert!((renderer.gain - 1.0).abs() < 1e-6);

        // Control issues a seek.
        if let Ok(mut slot) = shared.seek_request.lock() {
            *slot = Some(500);
        }
        shared.base_ms.store(500, Ordering::Release);
        shared.played_frames.store(0, Ordering::Relaxed);
        shared.epoch.fetch_add(1, Ordering::AcqRel);
        push(&mut producer, 400);

        out.fill(0.0);
        renderer.render(&mut out);
        assert_eq!(shared.drained_epoch.load(Ordering::Relaxed), 1);
        assert_eq!(shared.ready_epoch.load(Ordering::Relaxed), 0);
        assert_eq!(shared.played_frames.load(Ordering::Relaxed), 0);
        assert!(
            out[FADE_FRAMES * CHANNELS..]
                .iter()
                .all(|sample| *sample == 0.0)
        );
        assert_eq!(renderer.fade_epoch, Some(1));

        // Decoder finished seeking; the renderer must clear once more and ack.
        shared.seeked_epoch.store(1, Ordering::Release);
        out.fill(0.0);
        renderer.render(&mut out);
        assert_eq!(shared.ready_epoch.load(Ordering::Relaxed), 1);
        assert_eq!(renderer.handled_epoch, 1);
        assert!(renderer.fade_epoch.is_none());
        assert_eq!(renderer.gain, 0.0);
        assert!(out.iter().all(|sample| *sample == 0.0));

        // New samples after the seq start from silence (fade-in).
        push(&mut producer, 1000);
        out.fill(0.0);
        renderer.render(&mut out);
        assert_eq!(out[0], 0.0);
        assert!(out[CHANNELS] > 0.0);
    }
}
