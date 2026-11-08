//! Writes a tiny 16-bit PCM WAV sine wave for manual playback tests.
//!
//! Kept pure Rust on purpose: no sample files are committed to the repo, and
//! the audio crate's tests must not depend on external media.
//!
//! ```text
//! cargo run -p np-audio --example make_wav -- target/sine-44100.wav [rate] [seconds] [channels]
//! ```

use std::env;
use std::f32::consts::TAU;
use std::fs::File;
use std::io::{BufWriter, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "target/sine-44100.wav".into());
    let rate: u32 = args.next().map(|value| value.parse()).transpose()?.unwrap_or(44_100);
    let seconds: u32 = args.next().map(|value| value.parse()).transpose()?.unwrap_or(5);
    let channels: u16 = args.next().map(|value| value.parse()).transpose()?.unwrap_or(2);

    let frames = rate * seconds;
    let mut samples = Vec::with_capacity((frames as usize) * usize::from(channels));
    let fade_frames = (rate as f32 * 0.02) as u32;
    for frame in 0..frames {
        let mut value = (TAU * 440.0 * frame as f32 / rate as f32).sin() * 0.3;
        if frame < fade_frames {
            value *= frame as f32 / fade_frames as f32;
        } else if frame + fade_frames > frames {
            value *= (frames - frame) as f32 / fade_frames as f32;
        }
        let pcm = (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        for _ in 0..channels {
            samples.push(pcm);
        }
    }

    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = BufWriter::new(File::create(&path)?);
    let data_len = (samples.len() * 2) as u32;
    let byte_rate = rate * u32::from(channels) * 2;
    let block_align = channels * 2;

    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_len).to_le_bytes())?;
    out.write_all(b"WAVE")?;
    out.write_all(b"fmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&1u16.to_le_bytes())?; // PCM
    out.write_all(&channels.to_le_bytes())?;
    out.write_all(&rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&16u16.to_le_bytes())?; // bits per sample
    out.write_all(b"data")?;
    out.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()?;

    println!(
        "wrote {path}: {rate} Hz, {channels} ch, {seconds} s, 440 Hz sine (16-bit PCM)"
    );
    Ok(())
}
