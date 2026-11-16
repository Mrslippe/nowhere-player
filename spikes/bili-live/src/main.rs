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
mod flv;

use std::process::ExitCode;
use std::time::Instant;

use serde_json::{Value, json};

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
