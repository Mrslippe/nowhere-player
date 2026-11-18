//! B 站直播 API：`getRoomPlayInfo`（默认 `only_audio=1`）与 `room get_info`。
//!
//! 默认请求 qn25000 HEVC 的 only_audio 流；`NP_LIVE_NORMAL=1` 时请求普通 FLV，
//! 仍只解 type=8 音频。
//!
//! 只走 [`np_sources::Transport`]，测试注入夹具，不会触网。

use np_sources::Transport;
use serde::Deserialize;

use crate::auth::BiliAuth;
use crate::error::LiveError;

/// 直播 playurl 接口。
const PLAY_INFO_URL: &str = "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo";
/// 房间信息接口（标题 / 开播状态 / 分区）。
const ROOM_INFO_URL: &str = "https://api.live.bilibili.com/room/v1/Room/get_info";
/// 主播资料接口（主播名）。
const ANCHOR_INFO_URL: &str =
    "https://api.live.bilibili.com/live_user/v1/UserInfo/get_anchor_in_room";

/// 打开 CDN 长连接时必须带的请求头（B 站缺 `Referer` 会 403）。
const STREAM_HEADERS: [(&str, &str); 3] = [
    ("Referer", "https://live.bilibili.com/"),
    ("Origin", "https://live.bilibili.com"),
    ("Accept", "*/*"),
];

/// `getRoomPlayInfo` URL。
///
/// 默认 qn25000 HEVC only_audio；`NP_LIVE_NORMAL=1` 请求普通 FLV。
pub fn play_info_url(room_id: u64) -> String {
    let only_audio = std::env::var_os("NP_LIVE_NORMAL").is_none();
    play_info_url_with_audio_mode(room_id, only_audio)
}

fn play_info_url_with_audio_mode(room_id: u64, only_audio: bool) -> String {
    let suffix = if only_audio { "&only_audio=1" } else { "" };
    let qn = std::env::var("NP_LIVE_QN").unwrap_or_else(|_| "25000".to_owned());
    let codec = std::env::var("NP_LIVE_CODEC").unwrap_or_else(|_| "1".to_owned());
    format!(
        "{PLAY_INFO_URL}?room_id={room_id}&protocol=0&format=0&codec={codec}&qn={qn}&platform=web&ptype=8{suffix}"
    )
}

/// `get_info` URL。
pub fn room_info_url(room_id: u64) -> String {
    format!("{ROOM_INFO_URL}?room_id={room_id}")
}

/// 主播资料 URL（`get_anchor_in_room`，参数名是 `roomid`）。
pub fn anchor_info_url(room_id: u64) -> String {
    format!("{ANCHOR_INFO_URL}?roomid={room_id}")
}

/// CDN 长连接请求头。
pub fn stream_headers() -> &'static [(&'static str, &'static str)] {
    &STREAM_HEADERS
}

/// 一次 playurl 解析的结果。
#[derive(Debug, Clone)]
pub struct PlayInfo {
    /// 调用方请求的房间号（可能是短号）。
    pub requested_room: u64,
    /// 接口返回的真实房间号。
    pub room_id: u64,
    /// `1` 表示直播中。
    pub live_status: i64,
    /// 第一个 `http_stream/flv` 的完整 URL；离线时为 `None`。
    pub flv_url: Option<String>,
    /// 服务端报告的 codec（只用于诊断，不能据此判断流内是否真有视频）。
    pub codec: Option<String>,
}

impl PlayInfo {
    /// 是否直播中。
    pub fn is_live(&self) -> bool {
        self.live_status == 1
    }
}

/// 房间信息（标题 / 分区）。主播名走 [`fetch_anchor_name`]。
#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub room_id: u64,
    pub live_status: i64,
    pub title: String,
    /// 二级分区名，例如「游戏赛事」。
    pub area_name: String,
    /// 一级分区名，例如「赛事」；`area_name` 缺失时用它兜底。
    pub parent_area_name: String,
    /// B站开播时间（UTC+8 解析成 Unix 秒）；解析失败为 `None`。
    pub live_started_at: Option<u64>,
    /// 当前同接人数（`data.online`）；接口缺失时为 `None`。
    pub online: Option<u64>,
}

/// 取 playurl。调用方负责把 [`PlayInfo::live_status`] / `flv_url` 映射成
/// [`LiveError::Offline`] / [`LiveError::NoStream`]。
pub fn fetch_play_info(transport: &dyn Transport, room_id: u64) -> Result<PlayInfo, LiveError> {
    fetch_play_info_with_auth(transport, room_id, None)
}

/// 带登录 cookie 请求 playurl；匿名时与 [`fetch_play_info`] 相同。
pub fn fetch_play_info_with_auth(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
) -> Result<PlayInfo, LiveError> {
    validate_room(room_id)?;
    let url = play_info_url(room_id);
    let cookie = auth.map(BiliAuth::cookie_header).unwrap_or_default();
    let response = if cookie.is_empty() {
        transport.get_with_headers(&url, &[("Referer", "https://live.bilibili.com/")])?
    } else {
        transport.get_with_headers(
            &url,
            &[
                ("Referer", "https://live.bilibili.com/"),
                ("Cookie", &cookie),
            ],
        )?
    };
    if !(200..=299).contains(&response.status) {
        return Err(LiveError::Source(np_sources::SourceError::Http(
            response.status,
        )));
    }
    parse_play_info(&response.body, room_id)
}

/// 取房间信息。标题是 best-effort，失败不应阻断播放。
pub fn fetch_room_info(transport: &dyn Transport, room_id: u64) -> Result<RoomInfo, LiveError> {
    validate_room(room_id)?;
    let body = transport.get(&room_info_url(room_id))?;
    parse_room_info(&body)
}

/// 取主播名。best-effort：失败只影响 Player 顶边展示，不影响播放。
pub fn fetch_anchor_name(transport: &dyn Transport, room_id: u64) -> Result<String, LiveError> {
    validate_room(room_id)?;
    let body = transport.get(&anchor_info_url(room_id))?;
    parse_anchor_name(&body)
}

/// 解析 `getRoomPlayInfo` 响应。
pub fn parse_play_info(body: &[u8], requested_room: u64) -> Result<PlayInfo, LiveError> {
    let envelope: Envelope<PlayData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("playinfo JSON 解析失败：{error}")))?;
    envelope.ensure_ok("playinfo")?;
    let data = envelope
        .data
        .ok_or_else(|| LiveError::BadStream("playinfo 响应缺少 data".to_owned()))?;
    let flv = data
        .playurl_info
        .as_ref()
        .and_then(|info| info.playurl.as_ref())
        .and_then(|playurl| find_flv(&playurl.stream));
    Ok(PlayInfo {
        requested_room,
        room_id: data.room_id,
        live_status: data.live_status,
        flv_url: flv.as_ref().map(|candidate| candidate.url.clone()),
        codec: flv.map(|candidate| candidate.codec),
    })
}

/// 解析 `room get_info` 响应。
pub fn parse_room_info(body: &[u8]) -> Result<RoomInfo, LiveError> {
    let envelope: Envelope<RoomData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("房间信息 JSON 解析失败：{error}")))?;
    envelope.ensure_ok("房间信息")?;
    let data = envelope
        .data
        .ok_or_else(|| LiveError::BadStream("房间信息响应缺少 data".to_owned()))?;
    Ok(RoomInfo {
        room_id: data.room_id,
        live_status: data.live_status,
        title: data.title,
        area_name: data.area_name,
        parent_area_name: data.parent_area_name,
        live_started_at: parse_live_time(&data.live_time),
        online: data.online,
    })
}

/// 解析 `get_anchor_in_room` 响应；`uname` 为空视为坏响应。
pub fn parse_anchor_name(body: &[u8]) -> Result<String, LiveError> {
    let envelope: Envelope<AnchorData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("主播信息 JSON 解析失败：{error}")))?;
    envelope.ensure_ok("主播信息")?;
    envelope
        .data
        .and_then(|data| data.info)
        .map(|info| info.uname)
        .filter(|uname| !uname.trim().is_empty())
        .ok_or_else(|| LiveError::BadStream("主播信息响应缺少 uname".to_owned()))
}

fn validate_room(room_id: u64) -> Result<(), LiveError> {
    if room_id == 0 {
        return Err(LiveError::InvalidRoom("房间号必须是正整数".to_owned()));
    }
    Ok(())
}

fn find_flv(streams: &[StreamInfo]) -> Option<FlvCandidate> {
    for stream in streams {
        if stream.protocol_name != "http_stream" {
            continue;
        }
        for format in &stream.format {
            if format.format_name != "flv" {
                continue;
            }
            for codec in &format.codec {
                let Some(info) = codec.url_info.first() else {
                    continue;
                };
                if info.host.is_empty() || codec.base_url.is_empty() {
                    continue;
                }
                return Some(FlvCandidate {
                    url: join_url(&info.host, &codec.base_url, &info.extra),
                    codec: codec.codec_name.clone(),
                });
            }
        }
    }
    None
}

fn join_url(host: &str, base_url: &str, extra: &str) -> String {
    let separator = if extra.is_empty() || base_url.ends_with('?') {
        ""
    } else {
        "?"
    };
    format!("{host}{base_url}{separator}{extra}")
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    fn ensure_ok(&self, label: &str) -> Result<(), LiveError> {
        if self.code != 0 {
            return Err(LiveError::Api {
                code: self.code,
                message: if self.message.is_empty() {
                    format!("{label} 请求失败")
                } else {
                    self.message.clone()
                },
            });
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct PlayData {
    room_id: u64,
    #[serde(default)]
    live_status: i64,
    #[serde(default)]
    playurl_info: Option<PlayurlInfo>,
}

#[derive(Debug, Deserialize)]
struct PlayurlInfo {
    #[serde(default)]
    playurl: Option<Playurl>,
}

#[derive(Debug, Deserialize)]
struct Playurl {
    #[serde(default)]
    stream: Vec<StreamInfo>,
}

#[derive(Debug, Deserialize)]
struct StreamInfo {
    #[serde(default)]
    protocol_name: String,
    #[serde(default)]
    format: Vec<FormatInfo>,
}

#[derive(Debug, Deserialize)]
struct FormatInfo {
    #[serde(default)]
    format_name: String,
    #[serde(default)]
    codec: Vec<CodecInfo>,
}

#[derive(Debug, Deserialize)]
struct CodecInfo {
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    url_info: Vec<UrlInfo>,
}

#[derive(Debug, Deserialize)]
struct UrlInfo {
    #[serde(default)]
    host: String,
    #[serde(default)]
    extra: String,
}

struct FlvCandidate {
    url: String,
    codec: String,
}

#[derive(Debug, Deserialize)]
struct RoomData {
    #[serde(default)]
    room_id: u64,
    #[serde(default)]
    live_status: i64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    area_name: String,
    #[serde(default)]
    parent_area_name: String,
    #[serde(default)]
    live_time: String,
    #[serde(default)]
    online: Option<u64>,
}

fn parse_live_time(value: &str) -> Option<u64> {
    let (date, clock) = value.trim().split_once(' ')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let unix = days * 86_400 + hour * 3_600 + minute * 60 + second - 8 * 3_600;
    u64::try_from(unix).ok()
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[derive(Debug, Deserialize)]
struct AnchorData {
    #[serde(default)]
    info: Option<AnchorInfo>,
}

#[derive(Debug, Deserialize)]
struct AnchorInfo {
    #[serde(default)]
    uname: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const LIVE: &str = include_str!("../fixtures/live_playinfo.json");
    const OFFLINE: &str = include_str!("../fixtures/offline_playinfo.json");
    const ERROR: &str = include_str!("../fixtures/error_playinfo.json");
    const ROOM: &str = include_str!("../fixtures/room_info.json");

    struct RecordingTransport {
        body: &'static str,
        urls: Mutex<Vec<String>>,
    }

    impl RecordingTransport {
        fn new(body: &'static str) -> Self {
            Self {
                body,
                urls: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for RecordingTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            self.urls.lock().unwrap().push(url.to_owned());
            Ok(self.body.as_bytes().to_vec())
        }
    }

    #[test]
    fn play_info_url_requests_audio_only_by_default() {
        let url = play_info_url_with_audio_mode(21622811, true);
        assert!(url.contains("room_id=21622811"));
        assert!(url.contains("only_audio=1"));
        assert!(url.contains("qn=25000"));
        assert!(url.contains("codec=1"));
        assert!(url.contains("protocol=0"));
        assert!(url.contains("format=0"));
    }

    #[test]
    fn play_info_url_can_request_normal_flv_for_diagnostics() {
        let url = play_info_url_with_audio_mode(21622811, false);
        assert!(!url.contains("only_audio=1"));
        assert!(url.contains("protocol=0"));
    }

    #[test]
    fn live_fixture_yields_flv_url_and_codec() {
        let info = parse_play_info(LIVE.as_bytes(), 21622811).unwrap();
        assert_eq!(info.room_id, 21622811);
        assert!(info.is_live());
        let url = info.flv_url.unwrap();
        assert!(url.starts_with("https://example.invalid/redacted/stream.flv"));
        assert!(!url.ends_with('?'));
        assert_eq!(info.codec.as_deref(), Some("avc"));
    }

    #[test]
    fn offline_fixture_has_no_stream() {
        let info = parse_play_info(OFFLINE.as_bytes(), 510).unwrap();
        assert_eq!(info.room_id, 80397);
        assert!(!info.is_live());
        assert!(info.flv_url.is_none());
    }

    #[test]
    fn error_fixture_maps_api_code() {
        let error = parse_play_info(ERROR.as_bytes(), 99999999).unwrap_err();
        assert!(matches!(
            error,
            LiveError::Api {
                code: 60004,
                message: _
            }
        ));
    }

    #[test]
    fn room_fixture_yields_title() {
        let room = parse_room_info(ROOM.as_bytes()).unwrap();
        assert_eq!(room.room_id, 21622811);
        assert_eq!(room.live_status, 1);
        assert!(!room.title.is_empty());
    }

    #[test]
    fn room_fixture_yields_area_names() {
        let room = parse_room_info(ROOM.as_bytes()).unwrap();
        assert_eq!(room.area_name, "游戏赛事");
        assert_eq!(room.parent_area_name, "赛事");
    }

    #[test]
    fn room_fixture_yields_online_and_missing_online_is_none() {
        let room = parse_room_info(ROOM.as_bytes()).unwrap();
        assert_eq!(room.online, Some(8400));

        let room =
            parse_room_info(br#"{"code":0,"data":{"room_id":1,"title":"x","live_status":1}}"#)
                .unwrap();
        assert_eq!(room.online, None);
    }

    #[test]
    fn anchor_info_url_uses_the_user_api_roomid_parameter() {
        let url = anchor_info_url(21622811);
        assert!(url.contains("get_anchor_in_room"));
        assert!(url.contains("roomid=21622811"));
    }

    #[test]
    fn anchor_fixture_yields_uname() {
        let name = parse_anchor_name(
            br#"{"code":0,"msg":"ok","data":{"info":{"uid":474595627,"uname":"\u76f4\u64ad\u4e3b"}}}"#,
        )
        .unwrap();
        assert_eq!(name, "直播主");
    }

    #[test]
    fn anchor_error_maps_api_code() {
        let error = parse_anchor_name(br#"{"code":60004,"message":"room not found"}"#).unwrap_err();
        assert!(matches!(error, LiveError::Api { code: 60004, .. }));
    }

    #[test]
    fn anchor_without_uname_is_a_stream_error() {
        let error = parse_anchor_name(br#"{"code":0,"data":{"info":{"uname":"  "}}}"#).unwrap_err();
        assert!(matches!(error, LiveError::BadStream(_)));
    }

    #[test]
    fn fetch_anchor_name_requests_the_user_api() {
        let transport =
            RecordingTransport::new(r#"{"code":0,"data":{"info":{"uname":"Nowhere"}}}"#);
        assert_eq!(fetch_anchor_name(&transport, 21622811).unwrap(), "Nowhere");
        let urls = transport.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("get_anchor_in_room"));
        assert!(urls[0].contains("roomid=21622811"));
    }

    #[test]
    fn fetch_play_info_uses_audio_only_hevc_url() {
        let transport = RecordingTransport::new(LIVE);
        fetch_play_info(&transport, 21622811).unwrap();
        let urls = transport.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("only_audio=1"));
        assert!(urls[0].contains("qn=25000"));
        assert!(urls[0].contains("codec=1"));
    }

    #[test]
    fn invalid_room_is_rejected_before_any_request() {
        let transport = RecordingTransport::new(LIVE);
        let error = fetch_play_info(&transport, 0).unwrap_err();
        assert!(matches!(error, LiveError::InvalidRoom(_)));
        assert!(transport.urls.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_a_readable_stream_error() {
        let error = parse_play_info(b"{not json", 1).unwrap_err();
        assert!(matches!(error, LiveError::BadStream(_)));
    }
}
