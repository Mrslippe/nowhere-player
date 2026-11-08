//! Smoke test for the sink + ring buffer without any decoder.
//!
//! Generates a 440 Hz sine on the main thread, feeds it through the same
//! lock-free ring buffer the real player uses, and lets the cpal callback pull
//! it. This is the smallest proof that "ring → device" works and makes sound.
//!
//! ```text
//! cargo run -p np-audio --example tone
//! ```

use std::thread;
use std::time::Duration;

use np_audio::format::{AudioFormat, SampleFormat, negotiate};
use np_audio::ring;
use np_audio::sink::{AudioSink, CpalSharedSink};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sink = CpalSharedSink::new()?;
    let caps = sink.device_caps()?;
    println!("device: {}", sink.name());
    println!("device caps:");
    for format in &caps.formats {
        println!("  {format}");
    }

    // A 48 kHz stereo request will normally be served at 48 kHz; if the device
    // only has 44.1 kHz, negotiate() picks that and the sine is generated at the
    // device rate (this example has no resampler in the path).
    let requested = AudioFormat::new(48_000, 2, SampleFormat::F32);
    let format = negotiate(requested, &caps)?;
    println!("output format: {format}");

    let channels = format.frame_samples();
    let rate = format.sample_rate as f32;
    let capacity = (format.sample_rate as usize / 1000) * 300 * channels;
    let (mut producer, consumer) = ring::spsc(capacity);

    let mut phase = 0.0f32;
    let tone = |frames: usize, phase: &mut f32| -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames * channels);
        for _ in 0..frames {
            let value = (phase.sin()) * 0.2;
            *phase += std::f32::consts::TAU * 440.0 / rate;
            if *phase > std::f32::consts::TAU {
                *phase -= std::f32::consts::TAU;
            }
            for _ in 0..channels {
                samples.push(value);
            }
        }
        samples
    };

    // Pre-fill 200 ms so the stream starts with a healthy cushion.
    let prefill = (format.sample_rate as usize / 1000) * 200;
    let initial = tone(prefill, &mut phase);
    let pushed = producer.push(&initial);
    println!("prefilled {pushed} samples");

    let mut consumer = consumer;
    let render = Box::new(move |out: &mut [f32]| {
        let written = consumer.pop(out);
        for sample in &mut out[written..] {
            *sample = 0.0;
        }
    });
    sink.open(format, render)?;

    println!("playing 3 seconds of 440 Hz sine ...");
    let chunk_frames = (format.sample_rate as usize / 1000) * 20;
    for step in 0..150 {
        let samples = tone(chunk_frames, &mut phase);
        let pushed = producer.push(&samples);
        if step % 25 == 0 {
            println!(
                "  t={:>4} ms  pushed={pushed:>4}  buffered={:>5}",
                step * 20,
                producer.occupied()
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(Duration::from_millis(300));

    sink.close();
    println!("done; if you heard a clean 440 Hz tone the sink works.");
    Ok(())
}
