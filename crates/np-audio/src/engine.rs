//! Playback engine: device + decode thread + lock-free hand-off.
//!
//! Thread layout (ARCH-006 §4):
//!
//! - this thread (caller / actor): control, never touches audio data;
//! - `np-audio-decode`: blocking decode + optional resample, fills the ring;
//! - cpal's audio thread: runs [`Renderer::render`], pops the ring and applies
//!   gain. It only touches atomics, a stack buffer and the ring.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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

/// Frames decoded per source read. 1024 frames ≈ 21 ms at 48 kHz.
const READ_FRAMES: usize = 1024;

/// State shared between control, decode thread and audio callback.
pub(crate) struct Shared {
    playing: AtomicBool,
    /// Linear gain (`v^3`), stored as f32 bits.
    gain: AtomicU32,
    /// Frames handed to the device since playback started.
    played_frames: AtomicU64,
    ended: AtomicBool,
    /// Set once the decoder hit EOF *and* the ring was drained to the device.
    drained: AtomicBool,
    shutdown: AtomicBool,
    underruns: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            gain: AtomicU32::new(1.0f32.to_bits()),
            played_frames: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            error: Mutex::new(None),
        }
    }

    fn set_error(&self, error: &AudioError) {
        if let Ok(mut slot) = self.error.lock() {
            slot.get_or_insert_with(|| error.to_string());
        }
    }
}

/// Fills the device buffer from the ring and applies gain.
///
/// Runs on the audio thread: no allocation, no locks, no formatting.
pub(crate) struct Renderer {
    consumer: RingConsumer,
    shared: Arc<Shared>,
    channels: usize,
    gain: f32,
}

impl Renderer {
    fn new(consumer: RingConsumer, shared: Arc<Shared>, channels: usize) -> Self {
        Self {
            consumer,
            shared,
            channels,
            gain: 0.0,
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let target = if self.shared.playing.load(Ordering::Relaxed) {
            f32::from_bits(self.shared.gain.load(Ordering::Relaxed))
        } else {
            0.0
        };
        self.gain = target;
        if self.gain <= 0.0 {
            out.fill(0.0);
            return;
        }
        let written = self.consumer.pop(out);
        for sample in &mut out[..written] {
            *sample *= self.gain;
        }
        for sample in &mut out[written..] {
            *sample = 0.0;
        }
        if written == 0 {
            let ended = self.shared.ended.load(Ordering::Relaxed);
            // Startup buffering and the tail after EOF are expected silence,
            // not underruns; only a dry ring mid-track is a real one.
            if !ended && self.shared.played_frames.load(Ordering::Relaxed) > 0 {
                self.shared.underruns.fetch_add(1, Ordering::Relaxed);
            }
            if ended {
                self.shared.drained.store(true, Ordering::Relaxed);
            }
        }
        self.shared
            .played_frames
            .fetch_add((written / self.channels) as u64, Ordering::Relaxed);
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
    caps: DeviceCaps,
    device_name: String,
}

impl Engine {
    pub fn start(path: &Path) -> Result<Self, AudioError> {
        let source = FileSource::open(path)?;
        let source_format = source.format();
        let duration_ms = source.duration_ms();

        let cpal_sink = CpalSharedSink::new()?;
        let device_name = cpal_sink.name().to_owned();
        let caps = cpal_sink.device_caps()?;
        let output_format = negotiate(source_format, &caps)?;
        let plan = ProcessingPlan::compute(source_format, output_format);

        let ring_frames = (u64::from(output_format.sample_rate) * u64::from(RING_MS) / 1000) as usize;
        let (producer, consumer) = ring::spsc(ring_frames * output_format.frame_samples());

        let shared = Arc::new(Shared::new());
        let mut renderer = Renderer::new(
            consumer,
            Arc::clone(&shared),
            output_format.frame_samples(),
        );
        let mut sink: Box<dyn AudioSink> = Box::new(cpal_sink);
        sink.open(
            output_format,
            Box::new(move |out: &mut [f32]| renderer.render(out)),
        )?;

        let decode = spawn_decode(
            Box::new(source),
            plan,
            output_format,
            producer,
            Arc::clone(&shared),
        )?;

        Ok(Self {
            shared,
            decode: Some(decode),
            sink,
            source_format,
            output_format,
            plan,
            duration_ms,
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

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .gain
            .store(gain_for_volume(volume).to_bits(), Ordering::Release);
    }

    pub fn position_ms(&self) -> u64 {
        frames_to_ms(
            self.shared.played_frames.load(Ordering::Acquire),
            self.output_format.sample_rate,
        )
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
        self.shared
            .error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
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
    let mut resampler = if plan.resample {
        Some(StreamResampler::new(source_format, output_format)?)
    } else {
        None
    };

    let mut decoded = vec![0.0f32; READ_FRAMES * in_channels];
    let mut mapped = Vec::with_capacity(READ_FRAMES * usize::from(output_format.channels));
    let mut converted = Vec::new();

    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        if !shared.playing.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(3));
            continue;
        }
        // Keep at least a few callback blocks of headroom before decoding more.
        if producer.vacant() < usize::from(output_format.channels) * 256 {
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
            push_all(&mut producer, &converted, shared)?;
            shared.ended.store(true, Ordering::Release);
            return Ok(());
        }

        map_channels(
            &decoded[..frames * in_channels],
            in_channels,
            usize::from(output_format.channels),
            &mut mapped,
        );
        match resampler.as_mut() {
            Some(resampler) => resampler.process(&mapped, &mut converted)?,
            None => converted.extend_from_slice(&mapped),
        }
        push_all(&mut producer, &converted, shared)?;
    }
}

/// Pushes all samples, waiting for the consumer when the ring is full.
fn push_all(
    producer: &mut RingProducer,
    samples: &[f32],
    shared: &Shared,
) -> Result<(), AudioError> {
    let mut offset = 0;
    while offset < samples.len() {
        if shared.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        let pushed = producer.push(&samples[offset..]);
        if pushed == 0 {
            thread::sleep(Duration::from_millis(1));
        } else {
            offset += pushed;
        }
    }
    Ok(())
}
