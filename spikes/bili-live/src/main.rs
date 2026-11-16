//! B0 spike：B 站普通直播 HTTP-FLV -> FLV 音频 tag -> ADTS -> symphonia AAC -> PCM。
//!
//! 用法：
//!   cargo run --release -- --probe [--room <id>]
//!   cargo run --release -- --dry-run-send <room> <text>
//!   cargo run --release -- --send-danmaku <room> <text>
//!
//! 凭据仅从 NP_BILI_SESSDATA / NP_BILI_JCT 读取；缺失时发送模式直接 SKIP。

mod aac;
mod api;
mod direct;
mod flv;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};

const DEFAULT_ROOMS: [u64; 4] = [21622811, 510, 21452505, 6];

fn usage() -> u8 {
    eprintln!(
        "usage:\n  \
         bili-live-spike --probe [--room <id>]\n  \
         bili-live-spike --dry-run-send <room> <text>\n  \
         bili-live-spike --send-danmaku <room> <text>"
    );
    2
}

fn parse_room(s: &str) -> Option<u64> {
    s.parse::<u64>().ok().filter(|r| *r > 0)
}

fn round(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let code = match args.first().map(String::as_str) {
        None | Some("--probe") => run_probe(&args),
        Some("--stream") => run_stream(&args),
        Some("--direct") => run_direct(&args),
        Some("--dry-run-send") => {
            if args.len() < 3 {
                usage()
            } else {
                match parse_room(&args[1]) {
                    Some(room) => dry_run_send(room, &args[2..].join(" ")),
                    None => usage(),
                }
            }
        }
        Some("--send-danmaku") => {
            if args.len() < 3 {
                usage()
            } else {
                match parse_room(&args[1]) {
                    Some(room) => send_danmaku(room, &args[2..].join(" ")),
                    None => usage(),
                }
            }
        }
        Some("--help" | "-h") => {
            usage();
            0
        }
        Some(other) => {
            eprintln!("unknown argument: {other}");
            usage()
        }
    };

    ExitCode::from(code)
}

fn run_probe(args: &[String]) -> u8 {
    let mut explicit_room: Option<u64> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--room" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--room requires a value");
                    return 2;
                }
                match parse_room(&args[i]) {
                    Some(r) => explicit_room = Some(r),
                    None => {
                        eprintln!("invalid room id: {}", args[i]);
                        return 2;
                    }
                }
            }
            other => {
                eprintln!("unknown probe argument: {other}");
                return 2;
            }
        }
        i += 1;
    }

    let started = Instant::now();
    let client = match api::http_client(60) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "{}",
                json!({"status": "FAILED", "only_audio": true, "stage": e.stage, "error": e.message})
            );
            return 1;
        }
    };

    let rooms: Vec<u64> = match explicit_room {
        Some(r) => vec![r],
        None => DEFAULT_ROOMS.to_vec(),
    };

    let mut tried: Vec<Value> = Vec::new();
    let mut chosen: Option<api::PlayInfo> = None;

    for &room in &rooms {
        match api::fetch_play_info(&client, room) {
            Ok(info) => {
                let has_flv = info.flv_url.is_some();
                tried.push(json!({
                    "requested_room": room,
                    "room_id": info.room_id,
                    "live_status": info.live_status,
                    "flv": has_flv,
                    "api_ms": info.api_ms
                }));
                if info.live_status == 1 && has_flv {
                    chosen = Some(info);
                    break;
                }
            }
            Err(e) => tried.push(json!({
                "requested_room": room,
                "error": e.message,
                "stage": e.stage
            })),
        }
    }

    let Some(info) = chosen else {
        println!(
            "{}",
            json!({
                "status": "BLOCKED",
                "only_audio": true,
                "reason": "no room in target list reported live_status=1 with an FLV stream",
                "rooms": rooms,
                "rooms_tried": tried
            })
        );
        return 2;
    };

    let room_info = api::fetch_room_info(&client, info.room_id).ok();
    let title = room_info.as_ref().and_then(|r| r.title.clone());
    let uid = room_info.as_ref().and_then(|r| r.uid);
    let room_api_ms = room_info.as_ref().map(|r| r.api_ms).unwrap_or(0);
    let room_live_status = room_info.as_ref().and_then(|r| r.live_status);

    let url = info
        .flv_url
        .as_ref()
        .expect("chosen play info always has a URL");
    let capture = match flv::capture(&client, url) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "only_audio": true,
                    "stage": e.stage,
                    "error": e.message,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    },
                    "timing": {"api_ms": info.api_ms, "total_ms": started.elapsed().as_millis()}
                })
            );
            return 1;
        }
    };

    let decode_started = Instant::now();
    let decoded = match aac::decode(
        &capture.adts,
        capture.config.sample_rate,
        capture.config.channels,
    ) {
        Ok(d) => d,
        Err(e) => {
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "only_audio": true,
                    "stage": "DECODE",
                    "error": e,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    },
                    "audio": {
                        "codec": "aac-lc",
                        "aot": capture.config.aot,
                        "sample_rate": capture.config.sample_rate,
                        "channels": capture.config.channels,
                        "asc_hex": capture.config.asc_hex()
                    },
                    "flv": {
                        "aac_frames_wrapped": capture.frames,
                        "adts_bytes": capture.adts.len()
                    },
                    "timing": {"total_ms": started.elapsed().as_millis()}
                })
            );
            return 1;
        }
    };
    let decode_ms = decode_started.elapsed().as_millis();

    let st = &capture.stats;
    let output = json!({
        "status": "OK",
        "only_audio": true,
        "room": {
            "requested_room": info.requested_room,
            "room_id": info.room_id,
            "title": title,
            "uid": uid,
            "live_status": info.live_status,
            "room_info_live_status": room_live_status
        },
        "stream": {
            "protocol": "http-flv",
            "video_codec": info.codec,
            "current_qn": info.current_qn,
            "url_redacted": true,
            "url_len": url.len()
        },
        "audio": {
            "codec": "aac-lc",
            "aot": capture.config.aot,
            "sample_rate": capture.config.sample_rate,
            "channels": capture.config.channels,
            "asc_hex": capture.config.asc_hex(),
            "aac_frames_wrapped": capture.frames,
            "first_audio_ts_ms": st.first_audio_ts_ms,
            "last_audio_ts_ms": st.last_audio_ts_ms,
            "audio_ts_span_ms": st.last_audio_ts_ms.zip(st.first_audio_ts_ms).map(|(a, b)| a.saturating_sub(b))
        },
        "pcm": {
            "frames": decoded.frames,
            "seconds": round(decoded.seconds, 3),
            "sample_rate": decoded.sample_rate,
            "channels": decoded.channels,
            "packets_decoded": decoded.packets,
            "decode_errors": decoded.decode_errors,
            "rms": round(decoded.rms, 5),
            "peak": round(decoded.peak, 4),
            "non_silent": decoded.rms > 1e-4
        },
        "flv": {
            "bytes_total": st.bytes_total,
            "audio_tag_bytes": st.audio_payload_bytes,
            "video_tag_bytes": st.video_payload_bytes,
            "script_tag_bytes": st.script_payload_bytes,
            "video_bytes_discarded": st.video_payload_bytes,
            "adts_bytes": st.adts_bytes,
            "tags_total": st.tags_total,
            "audio_tags": st.audio_tags,
            "video_tags": st.video_tags,
            "script_tags": st.script_tags,
            "sequence_headers": st.sequence_headers,
            "raw_audio_frames": st.raw_audio_frames,
            "frames_before_asc": st.frames_before_asc,
            "non_aac_audio_tags": st.non_aac_audio_tags,
            "malformed_audio_tags": st.malformed_audio_tags,
            "flv_flags": st.flv_flags,
            "eof_before_target": st.eof_before_target,
            "wall_limit_hit": st.wall_limit_hit,
            "byte_budget_hit": st.byte_budget_hit
        },
        "timing": {
            "api_ms": info.api_ms,
            "room_api_ms": room_api_ms,
            "flv_connect_ms": capture.connect_ms,
            "flv_read_ms": capture.read_ms,
            "flv_elapsed_ms": capture.elapsed_ms,
            "decode_ms": decode_ms,
            "total_ms": started.elapsed().as_millis()
        },
        "config": {
            "target_rooms": rooms,
            "rooms_tried": tried
        }
    });

    println!("{}", serde_json::to_string(&output).unwrap());
    0
}

fn run_stream(args: &[String]) -> u8 {
    let mut explicit_room: Option<u64> = None;
    let mut seconds: u64 = 10;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--room" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--room requires a value");
                    return 2;
                }
                match parse_room(&args[i]) {
                    Some(r) => explicit_room = Some(r),
                    None => {
                        eprintln!("invalid room id: {}", args[i]);
                        return 2;
                    }
                }
            }
            "--seconds" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--seconds requires a value");
                    return 2;
                }
                match args[i].parse::<u64>() {
                    Ok(v) if (1..=120).contains(&v) => seconds = v,
                    _ => {
                        eprintln!("--seconds must be 1..=120");
                        return 2;
                    }
                }
            }
            other => {
                eprintln!("unknown stream argument: {other}");
                return 2;
            }
        }
        i += 1;
    }

    let started = Instant::now();
    let client = match api::http_client(60) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "{}",
                json!({"status": "FAILED", "mode": "stream", "only_audio": true, "stage": e.stage, "error": e.message})
            );
            return 1;
        }
    };

    let rooms: Vec<u64> = match explicit_room {
        Some(r) => vec![r],
        None => vec![21622811, 23197314],
    };
    let mut tried: Vec<Value> = Vec::new();
    let mut chosen: Option<api::PlayInfo> = None;

    for &room in &rooms {
        match api::fetch_play_info(&client, room) {
            Ok(info) => {
                let has_flv = info.flv_url.is_some();
                tried.push(json!({
                    "requested_room": room,
                    "room_id": info.room_id,
                    "live_status": info.live_status,
                    "flv": has_flv,
                    "api_ms": info.api_ms
                }));
                if info.live_status == 1 && has_flv {
                    chosen = Some(info);
                    break;
                }
            }
            Err(e) => tried.push(json!({
                "requested_room": room,
                "error": e.message,
                "stage": e.stage
            })),
        }
    }

    let Some(info) = chosen else {
        println!(
            "{}",
            json!({
                "status": "BLOCKED",
                "mode": "stream",
                "only_audio": true,
                "reason": "no room in target list reported live_status=1 with an FLV stream",
                "rooms": rooms,
                "rooms_tried": tried
            })
        );
        return 2;
    };

    let room_info = api::fetch_room_info(&client, info.room_id).ok();
    let title = room_info.as_ref().and_then(|r| r.title.clone());
    let uid = room_info.as_ref().and_then(|r| r.uid);
    let room_api_ms = room_info.as_ref().map(|r| r.api_ms).unwrap_or(0);

    let url = info
        .flv_url
        .as_ref()
        .expect("chosen play info always has a URL");
    let request_started = Instant::now();
    let opened = match flv::open_stream_source(&client, url, Duration::from_secs(seconds)) {
        Ok(o) => o,
        Err(e) => {
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "mode": "stream",
                    "only_audio": true,
                    "stage": e.stage,
                    "error": e.message,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    },
                    "timing": {"api_ms": info.api_ms, "total_ms": started.elapsed().as_millis()}
                })
            );
            return 1;
        }
    };

    let opened_at = opened.opened_at;
    let connect_ms = opened.connect_ms;
    let header_ms = opened.header_ms;
    let stats = opened.stats.clone();
    let started_flag = opened.started.clone();
    let source = opened.source;

    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let decode_started = Instant::now();
    let decoded = match aac::decode_adts_media(mss, started_flag) {
        Ok(d) => d,
        Err(e) => {
            let st = stream_stats(&stats);
            let stage = st.fatal_stage.unwrap_or("DECODE");
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "mode": "stream",
                    "only_audio": true,
                    "stage": stage,
                    "error": e,
                    "fatal": st.fatal,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    },
                    "flv": {
                        "audio_tags": st.audio_tags,
                        "video_tags": st.video_tags,
                        "bytes_total": st.bytes_total,
                        "adts_bytes": st.adts_bytes,
                        "raw_audio_frames": st.raw_audio_frames
                    },
                    "timing": {"total_ms": started.elapsed().as_millis()}
                })
            );
            return 1;
        }
    };
    let finished = Instant::now();
    let st = stream_stats(&stats);

    let deadline = opened_at + Duration::from_secs(seconds);
    let shutdown_ms = finished.saturating_duration_since(deadline).as_millis();
    let wall_seconds = opened_at.elapsed().as_secs_f64();
    let mss_ring_bytes = 64 * 1024u64;
    let peak_memory_bytes =
        mss_ring_bytes + st.max_pending_bytes as u64 + st.max_tag_payload_bytes as u64;

    let output = json!({
        "status": "OK",
        "mode": "stream",
        "only_audio": true,
        "room": {
            "requested_room": info.requested_room,
            "room_id": info.room_id,
            "title": title,
            "uid": uid,
            "live_status": info.live_status
        },
        "stream": {
            "path": "AdtsReader over custom MediaSource (HTTP FLV -> ADTS inline)",
            "requested_seconds": seconds,
            "wall_seconds": round(wall_seconds, 3),
            "ended_at_deadline": finished >= deadline,
            "media_ts_span_ms": st.last_audio_ts_ms.zip(st.first_audio_ts_ms).map(|(a, b)| a.saturating_sub(b)),
            "url_redacted": true,
            "url_len": url.len()
        },
        "audio": {
            "codec": "aac-lc",
            "aot": st.aac_aot,
            "sample_rate": st.aac_sample_rate,
            "channels": st.aac_channels,
            "asc_hex": st.asc_hex
        },
        "latency": {
            "connect_ms": connect_ms,
            "flv_header_ms": header_ms,
            "adts_reader_init_ms": decoded.reader_init_ms,
            "first_audio_frame_ms_from_connect": st.first_audio_frame_at.map(|t| t.saturating_duration_since(opened_at).as_millis()),
            "first_audio_frame_us_from_connect": st.first_audio_frame_at.map(|t| t.saturating_duration_since(opened_at).as_micros()),
            "first_pcm_ms_from_connect": decoded.first_pcm_at.map(|t| t.saturating_duration_since(opened_at).as_millis()),
            "first_pcm_us_from_connect": decoded.first_pcm_at.map(|t| t.saturating_duration_since(opened_at).as_micros()),
            "first_pcm_ms_from_request": decoded.first_pcm_at.map(|t| t.saturating_duration_since(request_started).as_millis()),
            "first_pcm_us_from_request": decoded.first_pcm_at.map(|t| t.saturating_duration_since(request_started).as_micros())
        },
        "pcm": {
            "frames": decoded.frames,
            "seconds": round(decoded.seconds, 3),
            "sample_rate": decoded.sample_rate,
            "channels": decoded.channels,
            "packets_decoded": decoded.packets,
            "decode_errors": decoded.decode_errors,
            "rms": round(decoded.rms, 5),
            "peak": round(decoded.peak, 4),
            "non_silent": decoded.rms > 1e-4
        },
        "flv": {
            "bytes_total": st.bytes_total,
            "audio_tag_bytes": st.audio_payload_bytes,
            "video_tag_bytes": st.video_payload_bytes,
            "video_bytes_discarded": st.video_payload_bytes,
            "script_tag_bytes": st.script_payload_bytes,
            "adts_bytes": st.adts_bytes,
            "tags_total": st.tags_total,
            "audio_tags": st.audio_tags,
            "video_tags": st.video_tags,
            "script_tags": st.script_tags,
            "sequence_headers": st.sequence_headers,
            "raw_audio_frames": st.raw_audio_frames,
            "frames_before_asc": st.frames_before_asc,
            "non_aac_audio_tags": st.non_aac_audio_tags,
            "flv_flags": st.flv_flags,
            "eof": st.eof
        },
        "network": {
            "tag_reads": st.tag_reads,
            "total_tag_read_ms": st.total_tag_read_ms,
            "max_tag_read_ms": st.max_tag_read_ms,
            "slow_tag_reads_gt_100ms": st.slow_tag_reads_gt_100ms,
            "max_tag_gap_ms": st.max_tag_gap_ms,
            "underruns": st.underruns,
            "underrun_definition": "FLV tag read blocked >100ms after first PCM",
            "fatal": st.fatal
        },
        "memory": {
            "mss_ring_buffer_bytes": mss_ring_bytes,
            "max_pending_adts_bytes": st.max_pending_bytes,
            "max_tag_payload_bytes": st.max_tag_payload_bytes,
            "peak_estimate_bytes": peak_memory_bytes,
            "note": "rough estimate: symphonia 64KB ring + max ADTS pending + max tag payload; not RSS"
        },
        "decode": {
            "cpu_us": decoded.decode_cpu_us,
            "cpu_ms": round(decoded.decode_cpu_us as f64 / 1000.0, 3),
            "loop_ms": decoded.loop_ms,
            "max_packet_gap_ms": decoded.max_packet_gap_ms,
            "last_error": decoded.last_error
        },
        "shutdown": {
            "deadline_ms": seconds * 1000,
            "finished_after_deadline_ms": shutdown_ms,
            "within_1s": shutdown_ms <= 1000,
            "note": "measured from deadline to AdtsReader/decode return (drops HTTP reader)"
        },
        "timing": {
            "api_ms": info.api_ms,
            "room_api_ms": room_api_ms,
            "connect_ms": connect_ms,
            "decode_loop_ms": finished.saturating_duration_since(decode_started).as_millis(),
            "total_ms": started.elapsed().as_millis()
        },
        "config": {
            "target_rooms": rooms,
            "rooms_tried": tried
        }
    });

    println!("{}", serde_json::to_string(&output).unwrap());
    0
}

fn stream_stats(stats: &std::sync::Arc<std::sync::Mutex<flv::StreamStats>>) -> flv::StreamStats {
    stats
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn run_direct(args: &[String]) -> u8 {
    let mut explicit_room: Option<u64> = None;
    let mut seconds: u64 = 10;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--room" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--room requires a value");
                    return 2;
                }
                match parse_room(&args[i]) {
                    Some(r) => explicit_room = Some(r),
                    None => {
                        eprintln!("invalid room id: {}", args[i]);
                        return 2;
                    }
                }
            }
            "--seconds" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--seconds requires a value");
                    return 2;
                }
                match args[i].parse::<u64>() {
                    Ok(v) if (1..=120).contains(&v) => seconds = v,
                    _ => {
                        eprintln!("--seconds must be 1..=120");
                        return 2;
                    }
                }
            }
            other => {
                eprintln!("unknown direct argument: {other}");
                return 2;
            }
        }
        i += 1;
    }

    let started = Instant::now();
    let client = match api::http_client(60) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "{}",
                json!({"status": "FAILED", "mode": "direct", "only_audio": true, "stage": e.stage, "error": e.message})
            );
            return 1;
        }
    };

    let rooms: Vec<u64> = match explicit_room {
        Some(r) => vec![r],
        None => vec![21622811, 23197314],
    };
    let mut tried: Vec<Value> = Vec::new();
    let mut chosen: Option<api::PlayInfo> = None;

    for &room in &rooms {
        match api::fetch_play_info(&client, room) {
            Ok(info) => {
                let has_flv = info.flv_url.is_some();
                tried.push(json!({
                    "requested_room": room,
                    "room_id": info.room_id,
                    "live_status": info.live_status,
                    "flv": has_flv,
                    "api_ms": info.api_ms
                }));
                if info.live_status == 1 && has_flv {
                    chosen = Some(info);
                    break;
                }
            }
            Err(e) => tried.push(json!({
                "requested_room": room,
                "error": e.message,
                "stage": e.stage
            })),
        }
    }

    let Some(info) = chosen else {
        println!(
            "{}",
            json!({
                "status": "BLOCKED",
                "mode": "direct",
                "only_audio": true,
                "reason": "no room in target list reported live_status=1 with an FLV stream",
                "rooms": rooms,
                "rooms_tried": tried
            })
        );
        return 2;
    };

    let room_info = api::fetch_room_info(&client, info.room_id).ok();
    let title = room_info.as_ref().and_then(|r| r.title.clone());
    let uid = room_info.as_ref().and_then(|r| r.uid);
    let room_api_ms = room_info.as_ref().map(|r| r.api_ms).unwrap_or(0);

    let url = info
        .flv_url
        .as_ref()
        .expect("chosen play info always has a URL");
    let request_started = Instant::now();
    let source = match flv::RawFrameSource::open(&client, url, Duration::from_secs(seconds + 5)) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "mode": "direct",
                    "only_audio": true,
                    "stage": e.stage,
                    "error": e.message,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    }
                })
            );
            return 1;
        }
    };
    let opened_at = source.opened_at;
    let connect_ms = opened_at.duration_since(request_started).as_millis();

    let result = match direct::run(source, seconds) {
        Ok(r) => r,
        Err(e) => {
            println!(
                "{}",
                json!({
                    "status": "FAILED",
                    "mode": "direct",
                    "only_audio": true,
                    "stage": "DECODE",
                    "error": e,
                    "room": {
                        "requested_room": info.requested_room,
                        "room_id": info.room_id,
                        "title": title,
                        "live_status": info.live_status
                    },
                    "timing": {"api_ms": info.api_ms, "total_ms": started.elapsed().as_millis()}
                })
            );
            return 1;
        }
    };

    let st = &result.net;
    let first_pcm_ms = result
        .first_pcm_at
        .map(|t| t.saturating_duration_since(opened_at).as_millis());
    let first_pcm_us = result
        .first_pcm_at
        .map(|t| t.saturating_duration_since(opened_at).as_micros());
    let first_pcm_req_ms = result
        .first_pcm_at
        .map(|t| t.saturating_duration_since(request_started).as_millis());
    let first_frame_ms = st
        .first_frame_at
        .map(|t| t.saturating_duration_since(opened_at).as_millis());

    let output = json!({
        "status": "OK",
        "mode": "direct",
        "only_audio": true,
        "room": {
            "requested_room": info.requested_room,
            "room_id": info.room_id,
            "title": title,
            "uid": uid,
            "live_status": info.live_status
        },
        "parameters": {
            "target_ms": direct::TARGET_MS,
            "max_ms": direct::MAX_MS,
            "min_ms": direct::MIN_MS,
            "control_ms": direct::CONTROL_MS,
            "cooldown_ms": direct::COOLDOWN_MS,
            "prebuffer_ms": direct::PREBUFFER_MS,
            "queue_cap_frames": direct::QUEUE_CAP_FRAMES,
            "no_drop": result.no_drop
        },
        "audio": {
            "codec": "aac-lc",
            "aot": 2,
            "sample_rate": 48000,
            "channels": 2,
            "path": "AacDecoder raw AAC + ASC extra_data"
        },
        "latency": {
            "connect_ms": connect_ms,
            "first_frame_ms_from_connect": first_frame_ms,
            "first_pcm_ms_from_connect": first_pcm_ms,
            "first_pcm_us_from_connect": first_pcm_us,
            "first_pcm_ms_from_request": first_pcm_req_ms,
            "decoder_init_ms": result.decoder_init_ms
        },
        "decode_cpu_us": result.decode_cpu_us,
        "pcm": {
            "frames": result.pcm.frames,
            "seconds": round(result.pcm.seconds, 3),
            "sample_rate": 48000,
            "channels": 2,
            "packets_decoded": result.pcm.packets,
            "decode_errors": result.pcm.decode_errors,
            "rms": round(result.pcm.rms, 5),
            "peak": round(result.pcm.peak, 4),
            "non_silent": result.pcm.rms > 1e-4
        },
        "control": {
            "control_ticks": result.control_ticks,
            "avg_buffer_ms": round(result.avg_buffer_ms, 1),
            "max_buffer_ms": result.max_buffer_ms,
            "min_buffer_ms": result.min_buffer_ms,
            "drop_events": result.drop_events,
            "dropped_frames": result.dropped_frames,
            "dropped_bytes": result.dropped_bytes,
            "dropped_media_ms": result.dropped_media_ms,
            "startup_trim_frames": result.startup_trim_frames,
            "startup_trim_ms": result.startup_trim_ms,
            "underrun_events": result.underrun_events,
            "underrun_ms": result.underrun_ms,
            "stale_frames": result.stale_frames
        },
        "queue": {
            "peak_frames": result.queue_peak_frames,
            "peak_bytes": result.queue_peak_bytes,
            "peak_span_ms": result.queue_peak_span_ms,
            "final_frames": result.queue_final_frames,
            "max_queue_wait_ms": result.max_queue_wait_ms,
            "producer_overflow_waits": result.producer_overflow_waits
        },
        "flv": {
            "bytes_total": st.bytes_total,
            "audio_tag_bytes": st.audio_payload_bytes,
            "video_tag_bytes": st.video_payload_bytes,
            "video_bytes_discarded": st.video_payload_bytes,
            "tags_total": st.tags_total,
            "audio_tags": st.audio_tags,
            "video_tags": st.video_tags,
            "script_tags": st.script_tags,
            "raw_audio_frames": st.raw_frames,
            "sequence_headers": st.sequence_headers,
            "frames_before_asc": st.frames_before_asc,
            "malformed_audio_tags": st.malformed_audio_tags,
            "flv_flags": st.flv_flags,
            "eof": st.eof,
            "error": st.error
        },
        "timing": {
            "playback_ms": result.playback_ms,
            "wall_ms": result.wall_ms,
            "api_ms": info.api_ms,
            "room_api_ms": room_api_ms,
            "total_ms": started.elapsed().as_millis()
        },
        "producer_error": result.producer_error,
        "last_error": result.last_error,
        "config": {
            "target_rooms": rooms,
            "rooms_tried": tried
        }
    });

    println!("{}", serde_json::to_string(&output).unwrap());
    0
}

fn dry_run_send(room: u64, text: &str) -> u8 {
    let creds = api::Credentials::from_env();
    let (sess_len, jct_len) = creds.as_ref().map(|c| c.lengths()).unwrap_or((0, 0));

    let out = json!({
        "status": "DRY_RUN",
        "network": false,
        "method": "POST",
        "endpoint": "https://api.live.bilibili.com/msg/send",
        "credentials_present": creds.is_some(),
        "fields": {
            "roomid": {"len": room.to_string().len()},
            "msg": {"chars": text.chars().count(), "bytes": text.len()},
            "color": {"len": "16777215".len()},
            "fontsize": {"len": "25".len()},
            "mode": {"len": "1".len()},
            "rnd": {"len": 13},
            "csrf": {"len": jct_len},
            "csrf_token": {"len": jct_len},
            "bubble": {"len": "0".len()}
        },
        "cookie_fields": {
            "SESSDATA": {"len": sess_len},
            "bili_jct": {"len": jct_len}
        },
        "note": "no request was made; values are not printed"
    });
    println!("{}", serde_json::to_string(&out).unwrap());
    0
}

fn send_danmaku(room: u64, text: &str) -> u8 {
    let Some(creds) = api::Credentials::from_env() else {
        println!("SKIP: credentials unset");
        return 0;
    };
    if text.is_empty() {
        eprintln!("empty danmaku text");
        return 2;
    }

    let client = match api::http_client(20) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "{}",
                json!({"status": "FAILED", "stage": e.stage, "error": e.message})
            );
            return 1;
        }
    };

    match api::send_danmaku(&client, room, text, &creds) {
        Ok((code, message)) => {
            let out = json!({
                "status": if code == 0 { "SENT" } else { "REJECTED" },
                "room_id": room,
                "code": code,
                "message": message
            });
            println!("{}", serde_json::to_string(&out).unwrap());
            if code == 0 { 0 } else { 1 }
        }
        Err(e) => {
            println!(
                "{}",
                json!({"status": "FAILED", "stage": e.stage, "error": e.message})
            );
            1
        }
    }
}
