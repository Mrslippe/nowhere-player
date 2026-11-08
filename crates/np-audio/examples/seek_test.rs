//! Scripted pause / resume / seek / stop run for manual verification.
//!
//! ```text
//! cargo run -p np-audio --example seek_test -- <file>
//! ```
//!
//! It prints the position after every operation. Listen for clicks at the
//! pause, resume, seek and stop transitions; the unit tests only prove that the
//! sample-level gain ramp is continuous.

use std::env;
use std::thread;
use std::time::Duration;

use np_audio::Player;

fn wait(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or("usage: seek_test <file>")?;
    let player = Player::open(&path)?;
    println!(
        "source {} -> output {} (resample={})",
        player.format(),
        player.output_format(),
        player.processing_plan().resample
    );

    player.play()?;
    wait(1_000);
    let after_play = player.position_ms();
    println!("after 1.0 s play       : {after_play} ms");

    player.pause()?;
    wait(400);
    let paused_a = player.position_ms();
    wait(300);
    let paused_b = player.position_ms();
    println!("paused                 : {paused_a} ms -> {paused_b} ms (should be stable)");
    assert_eq!(paused_a, paused_b, "position moved while paused");

    player.play()?;
    wait(500);
    println!("after resume 0.5 s     : {} ms", player.position_ms());

    player.seek(2_000)?;
    wait(150);
    let after_seek = player.position_ms();
    println!("150 ms after seek(2000): {after_seek} ms (expect 2000–2200)");
    assert!(
        (2_000..=2_200).contains(&after_seek),
        "seek landed at {after_seek} ms"
    );

    wait(500);
    println!("0.5 s after seek       : {} ms", player.position_ms());

    // Seek precision while paused: the decoder reports its real landing spot.
    player.pause()?;
    player.seek(2_500)?;
    wait(300);
    let landed = player.position_ms();
    println!("seek(2500) paused      : {landed} ms (expect 2500 ±50)");
    assert!(
        landed.abs_diff(2_500) <= 50,
        "seek error {} ms",
        landed.abs_diff(2_500)
    );

    player.stop()?;
    wait(200);
    let after_stop = player.position_ms();
    println!("after stop             : {after_stop} ms (expect 0)");
    assert_eq!(after_stop, 0, "stop did not rewind");

    player.play()?;
    wait(500);
    println!(
        "after replay 0.5 s     : {} ms (expect ≈ 500)",
        player.position_ms()
    );

    player.stop()?;
    if let Some(error) = player.last_error() {
        return Err(std::io::Error::other(error).into());
    }
    println!("ok; underruns={}", player.underruns());
    Ok(())
}
