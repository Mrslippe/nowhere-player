//! Manual playback test.
//!
//! ```text
//! cargo run -p np-audio --example play -- <file> [max_seconds]
//! ```
//!
//! Prints the negotiated formats, the processing plan and the position while
//! playing, then a summary. Exit code is non-zero if playback reported an error.

use std::env;
use std::thread;
use std::time::{Duration, Instant};

use np_audio::Player;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().ok_or("usage: play <file> [max_seconds]")?;
    let max_seconds: Option<f64> = args.next().map(|value| value.parse()).transpose()?;

    let player = Player::open(&path)?;
    println!("device      : {}", player.device_name());
    println!("source      : {}", player.format());
    println!("output      : {}", player.output_format());
    let plan = player.processing_plan();
    println!(
        "plan        : resample={} gain={} bit_perfect={}",
        plan.resample,
        plan.gain,
        plan.is_bit_perfect()
    );
    println!("duration    : {:?} ms", player.duration_ms());
    println!(
        "device caps : {} formats",
        player.device_caps().formats.len()
    );
    println!("playing {} ...", path);

    let started = Instant::now();
    player.play()?;
    let mut next_report = 0.0;
    let mut played_ms;
    loop {
        thread::sleep(Duration::from_millis(50));
        let elapsed = started.elapsed().as_secs_f64();
        played_ms = player.position_ms();
        if let Some(limit) = max_seconds
            && elapsed >= limit
        {
            break;
        }
        if elapsed >= next_report {
            println!("  t={elapsed:>5.1}s  position={played_ms:>7} ms");
            next_report += 0.5;
        }
        if player.is_ended() {
            break;
        }
        if let Some(error) = player.last_error() {
            return Err(std::io::Error::other(format!("playback failed: {error}")).into());
        }
    }
    player.pause()?;

    let duration = player.duration_ms();
    let wall = started.elapsed().as_secs_f64();
    println!(
        "stopped after {wall:.2}s: position={played_ms} ms, duration={duration:?} ms, underruns={}",
        player.underruns()
    );
    if let Some(error) = player.last_error() {
        return Err(std::io::Error::other(format!("playback failed: {error}")).into());
    }
    if played_ms == 0 {
        return Err(
            std::io::Error::other("position never advanced: no audio reached the device").into(),
        );
    }
    Ok(())
}
