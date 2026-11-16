//! B 站直播 API：取流地址、房间信息、弹幕发送。
//!
//! 本文件刻意不打印完整 signed URL / Cookie / 表单值。

use reqwest::blocking::Client;
use serde_json::Value;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const REFERER: &str = "https://live.bilibili.com/";
pub const ORIGIN: &str = "https://live.bilibili.com";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/131.0.0.0 Safari/537.36";

const PLAY_INFO_URL: &str = "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo";
const ROOM_INFO_URL: &str = "https://api.live.bilibili.com/room/v1/Room/get_info";
const SEND_URL: &str = "https://api.live.bilibili.com/msg/send";

#[derive(Debug)]
pub struct StageError {
    pub stage: &'static str,
    pub message: String,
}

impl StageError {
    pub fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)
    }
}

pub fn http_client(total_timeout_secs: u64) -> Result<Client, StageError> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(total_timeout_secs))
        .user_agent(UA)
        .build()
        .map_err(|e| StageError::new("API", format!("build client: {}", e.without_url())))
}

fn get_json(
    client: &Client,
    url: &str,
    query: &[(&str, &str)],
    stage: &'static str,
) -> Result<Value, StageError> {
    let resp = client
        .get(url)
        .query(query)
        .header("Referer", REFERER)
        .header("Origin", ORIGIN)
        .header("Accept", "application/json")
        .send()
        .map_err(|e| StageError::new(stage, format!("request: {}", e.without_url())))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(StageError::new(stage, format!("http status {status}")));
    }

    let body = resp
        .text()
        .map_err(|e| StageError::new(stage, format!("read body: {}", e.without_url())))?;
    serde_json::from_str(&body).map_err(|e| StageError::new(stage, format!("json decode: {e}")))
}

#[derive(Debug, Clone)]
pub struct PlayInfo {
    pub requested_room: u64,
    pub room_id: u64,
    pub live_status: i64,
    pub flv_url: Option<String>,
    pub codec: Option<String>,
    pub current_qn: Option<i64>,
    pub api_ms: u128,
}

/// 从 playurl 结构里挑第一个 http_stream/FLV 候选。
fn find_flv(playurl: &Value) -> Option<(String, String, i64)> {
    let streams = playurl.get("stream")?.as_array()?;
    for stream in streams {
        if stream.get("protocol_name").and_then(Value::as_str) != Some("http_stream") {
            continue;
        }
        let Some(formats) = stream.get("format").and_then(Value::as_array) else {
            continue;
        };
        for format in formats {
            if format.get("format_name").and_then(Value::as_str) != Some("flv") {
                continue;
            }
            let Some(codecs) = format.get("codec").and_then(Value::as_array) else {
                continue;
            };
            for codec in codecs {
                let codec_name = codec
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                let current_qn = codec.get("current_qn").and_then(Value::as_i64).unwrap_or(0);
                let Some(base_url) = codec.get("base_url").and_then(Value::as_str) else {
                    continue;
                };
                let Some(url_info) = codec.get("url_info").and_then(Value::as_array) else {
                    continue;
                };
                for ui in url_info {
                    let Some(host) = ui.get("host").and_then(Value::as_str) else {
                        continue;
                    };
                    let extra = ui.get("extra").and_then(Value::as_str).unwrap_or("");
                    let sep = if extra.is_empty() || base_url.ends_with('?') {
                        ""
                    } else {
                        "?"
                    };
                    return Some((
                        format!("{host}{base_url}{sep}{extra}"),
                        codec_name,
                        current_qn,
                    ));
                }
            }
        }
    }
    None
}

pub fn fetch_play_info(client: &Client, room_id: u64) -> Result<PlayInfo, StageError> {
    let started = Instant::now();
    let room_s = room_id.to_string();
    let v = get_json(
        client,
        PLAY_INFO_URL,
        &[
            ("room_id", room_s.as_str()),
            ("protocol", "0"),
            ("format", "0"),
            ("codec", "0"),
            ("qn", "80"),
            ("platform", "web"),
            ("ptype", "8"),
            ("only_audio", "1"),
        ],
        "API",
    )?;
    let api_ms = started.elapsed().as_millis();

    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(StageError::new(
            "API",
            format!("getRoomPlayInfo code={code} message={msg}"),
        ));
    }

    let data = v
        .get("data")
        .ok_or_else(|| StageError::new("API", "response has no data"))?;
    let actual_room = data
        .get("room_id")
        .and_then(Value::as_u64)
        .unwrap_or(room_id);
    let live_status = data
        .get("live_status")
        .and_then(Value::as_i64)
        .unwrap_or(-1);

    let flv = data
        .get("playurl_info")
        .and_then(|pi| pi.get("playurl"))
        .and_then(find_flv);

    Ok(PlayInfo {
        requested_room: room_id,
        room_id: actual_room,
        live_status,
        flv_url: flv.as_ref().map(|(u, _, _)| u.clone()),
        codec: flv.as_ref().map(|(_, c, _)| c.clone()),
        current_qn: flv.map(|(_, _, q)| q),
        api_ms,
    })
}

#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub title: Option<String>,
    pub uid: Option<u64>,
    pub live_status: Option<i64>,
    pub api_ms: u128,
}

pub fn fetch_room_info(client: &Client, room_id: u64) -> Result<RoomInfo, StageError> {
    let started = Instant::now();
    let room_s = room_id.to_string();
    let v = get_json(
        client,
        ROOM_INFO_URL,
        &[("room_id", room_s.as_str())],
        "API",
    )?;
    let api_ms = started.elapsed().as_millis();

    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(StageError::new(
            "API",
            format!("get_info code={code} msg={msg}"),
        ));
    }
    let data = v
        .get("data")
        .ok_or_else(|| StageError::new("API", "room info has no data"))?;

    Ok(RoomInfo {
        title: data
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        uid: data.get("uid").and_then(Value::as_u64),
        live_status: data.get("live_status").and_then(Value::as_i64),
        api_ms,
    })
}

/// 凭据只存在内存，Debug 不输出内容。
pub struct Credentials {
    sessdata: String,
    bili_jct: String,
}

impl Credentials {
    pub fn from_env() -> Option<Self> {
        let sessdata = std::env::var("NP_BILI_SESSDATA").ok()?.trim().to_string();
        let bili_jct = std::env::var("NP_BILI_JCT").ok()?.trim().to_string();
        if sessdata.is_empty() || bili_jct.is_empty() {
            return None;
        }
        Some(Self { sessdata, bili_jct })
    }

    pub fn lengths(&self) -> (usize, usize) {
        (self.sessdata.len(), self.bili_jct.len())
    }
}

/// 返回 (code, message)；不打印 Cookie、表单或完整响应。
pub fn send_danmaku(
    client: &Client,
    room_id: u64,
    text: &str,
    creds: &Credentials,
) -> Result<(i64, String), StageError> {
    let rnd = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string();
    let room_s = room_id.to_string();
    let cookie = format!("SESSDATA={}; bili_jct={}", creds.sessdata, creds.bili_jct);

    let resp = client
        .post(SEND_URL)
        .header("Referer", REFERER)
        .header("Origin", ORIGIN)
        .header("Cookie", cookie)
        .form(&[
            ("roomid", room_s),
            ("msg", text.to_string()),
            ("color", "16777215".to_string()),
            ("fontsize", "25".to_string()),
            ("mode", "1".to_string()),
            ("rnd", rnd),
            ("csrf", creds.bili_jct.clone()),
            ("csrf_token", creds.bili_jct.clone()),
            ("bubble", "0".to_string()),
        ])
        .send()
        .map_err(|e| StageError::new("SEND", format!("request: {}", e.without_url())))?;

    let status = resp.status();
    let body = resp.text().map_err(|e| {
        StageError::new("SEND", format!("http {status}, read: {}", e.without_url()))
    })?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| StageError::new("SEND", format!("http {status}, json: {e}")))?;
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(-1);
    let message = v
        .get("message")
        .or_else(|| v.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((code, message))
}
