//! B 站直播 API：`getRoomPlayInfo`（默认 `only_audio=1`）与 `room get_info`。
//!
//! 默认请求 qn25000 HEVC 的 only_audio 流；`NP_LIVE_NORMAL=1` 时请求普通 FLV，
//! 仍只解 type=8 音频。`video_play_info_url` / [`fetch_video_play_info`] 走
//! `only_audio=0` + `codec=0,1`（AVC + HEVC），只给 TUI 的直播画面取 FLV 地址
//! （T-0104 / T-0121）。
//!
//! 只走 [`np_sources::Transport`]，测试注入夹具，不会触网。

use np_sources::Transport;
use serde::Deserialize;

use crate::auth::BiliAuth;
use crate::error::LiveError;

/// 直播 playurl 接口。
const PLAY_INFO_URL: &str = "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo";
/// 房间信息接口（标题 / 开播状态 / 分区 / 主播 UID）。
const ROOM_INFO_URL: &str = "https://api.live.bilibili.com/room/v1/Room/get_info";
/// 房间初始化接口；房间信息缺 uid 时用它兜底取主播 UID。
const ROOM_INIT_URL: &str = "https://api.live.bilibili.com/room/v1/Room/room_init";
/// 主播资料接口（主播名）。
const ANCHOR_INFO_URL: &str =
    "https://api.live.bilibili.com/live_user/v1/UserInfo/get_anchor_in_room";
/// 实时同接人数接口（`data.count`，公开 GET、无 WBI）。
const ONLINE_RANK_URL: &str =
    "https://api.live.bilibili.com/xlive/general-interface/v1/rank/queryContributionRank";

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

/// `room_init` URL；返回真实房间号与主播 UID。
pub fn room_init_url(room_id: u64) -> String {
    format!("{ROOM_INIT_URL}?id={room_id}")
}

/// 实时同接人数 URL：公开 GET、无 WBI、不带 cookie。
pub fn online_rank_url(ruid: u64, room_id: u64) -> String {
    format!("{ONLINE_RANK_URL}?ruid={ruid}&room_id={room_id}&page=1&page_size=10&type=online_rank")
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
    /// 最高 `current_qn` 候选的完整 URL；离线时为 `None`。
    pub flv_url: Option<String>,
    /// 所选候选报告的 codec（`avc` / `hevc`，只用于诊断）。
    pub codec: Option<String>,
    /// 所选候选实际返回的清晰度档位（可能低于请求的 qn）。
    pub current_qn: Option<u64>,
    /// 所有 FLV 候选的 `accept_qn` 并集：去重、降序。
    pub accept_qn: Vec<u64>,
}

impl PlayInfo {
    /// 是否直播中。
    pub fn is_live(&self) -> bool {
        self.live_status == 1
    }
}

/// 房间信息（标题 / 分区 / 主播 UID）。主播名走 [`fetch_anchor_name`]。
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
    /// 主播 UID（`data.uid`），用于查询实时同接人数；旧接口可能缺失。
    pub uid: Option<u64>,
}

/// `room_init` 的要点：真实房间号与主播 UID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomInit {
    pub room_id: u64,
    pub uid: Option<u64>,
    pub live_status: i64,
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
    let url = play_info_url(room_id);
    request_play_info(transport, room_id, auth, &url)
}

/// 视频 playurl 默认请求的清晰度：4K（20000）。服务端按权限回退到 2K/原画。
pub const DEFAULT_VIDEO_QN: u64 = 20_000;

/// 直播画面的 playurl URL：`only_audio=0`，同时请求 AVC + HEVC（`codec=0,1`）。
///
/// T-0104：视频解码和窗口由外部播放器负责，TUI 只需要一条 FLV 地址。
pub fn video_play_info_url(room_id: u64) -> String {
    video_play_info_url_with_qn(room_id, DEFAULT_VIDEO_QN)
}

/// 按清晰度档位请求视频 playurl；`qn` 是 B 站清晰度编号（如 20000/10000/400）。
///
/// T-0117：清晰度切换由 TUI 按当前 qn 重取 URL，再交给 np-video 的 mpv
/// 执行 `loadfile ... replace`，不重建窗口。
/// T-0121：`codec=0,1` 同时要 AVC 与 HEVC——2K/4K 可能只出现在 HEVC 流里。
pub fn video_play_info_url_with_qn(room_id: u64, qn: u64) -> String {
    format!(
        "{PLAY_INFO_URL}?room_id={room_id}&protocol=0&format=0&codec=0,1&qn={qn}&platform=web&ptype=8&only_audio=0"
    )
}

/// 取直播画面的 playurl（`only_audio=0`）；`flv_url` 为第一条 http FLV 流。
///
/// 调用方负责把 [`PlayInfo::live_status`] / `flv_url` 映射成
/// [`LiveError::Offline`] / 中文的“没有可用 FLV 视频流”提示。
pub fn fetch_video_play_info(
    transport: &dyn Transport,
    room_id: u64,
) -> Result<PlayInfo, LiveError> {
    fetch_video_play_info_with_auth(transport, room_id, None)
}

/// 带登录 cookie 请求视频 playurl；匿名时与 [`fetch_video_play_info`] 相同。
pub fn fetch_video_play_info_with_auth(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
) -> Result<PlayInfo, LiveError> {
    fetch_video_play_info_with_auth_qn(transport, room_id, auth, DEFAULT_VIDEO_QN)
}

/// 带登录 cookie 按 qn 请求视频 playurl；T-0117 清晰度切换的取流入口。
pub fn fetch_video_play_info_with_auth_qn(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
    qn: u64,
) -> Result<PlayInfo, LiveError> {
    let url = video_play_info_url_with_qn(room_id, qn);
    request_play_info(transport, room_id, auth, &url)
}

fn request_play_info(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
    url: &str,
) -> Result<PlayInfo, LiveError> {
    validate_room(room_id)?;
    let cookie = auth.map(BiliAuth::cookie_header).unwrap_or_default();
    let response = if cookie.is_empty() {
        transport.get_with_headers(url, &[("Referer", "https://live.bilibili.com/")])?
    } else {
        transport.get_with_headers(
            url,
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

/// 取房间初始化信息（真实房间号 + 主播 UID）。best-effort。
pub fn fetch_room_init(transport: &dyn Transport, room_id: u64) -> Result<RoomInit, LiveError> {
    validate_room(room_id)?;
    let body = transport.get(&room_init_url(room_id))?;
    parse_room_init(&body)
}

/// 取实时同接人数（`queryContributionRank?type=online_rank` 的 `data.count`）。
///
/// 公开 GET、无 WBI、不带 cookie；失败由调用方显示 ` -- `，不阻断播放。
pub fn fetch_online_count(
    transport: &dyn Transport,
    ruid: u64,
    room_id: u64,
) -> Result<u64, LiveError> {
    validate_room(room_id)?;
    if ruid == 0 {
        return Err(LiveError::BadStream("主播 UID 必须是正整数".to_owned()));
    }
    let body = transport.get(&online_rank_url(ruid, room_id))?;
    parse_online_count(&body)
}

/// 解析 `getRoomPlayInfo` 响应。
///
/// T-0121：视频档位不再取第一条 FLV。遍历所有 `http_stream/flv` codec，选
/// `current_qn` 最大的候选（同 qn 优先 HEVC），`accept_qn` 取并集降序。
pub fn parse_play_info(body: &[u8], requested_room: u64) -> Result<PlayInfo, LiveError> {
    let envelope: Envelope<PlayData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("playinfo JSON 解析失败：{error}")))?;
    envelope.ensure_ok("playinfo")?;
    let data = envelope
        .data
        .ok_or_else(|| LiveError::BadStream("playinfo 响应缺少 data".to_owned()))?;
    let candidates = data
        .playurl_info
        .as_ref()
        .and_then(|info| info.playurl.as_ref())
        .map(|playurl| collect_flv_candidates(&playurl.stream))
        .unwrap_or_default();
    let best = pick_best_flv(&candidates);
    Ok(PlayInfo {
        requested_room,
        room_id: data.room_id,
        live_status: data.live_status,
        flv_url: best.map(|candidate| candidate.url.clone()),
        codec: best.map(|candidate| candidate.codec.clone()),
        current_qn: best.and_then(|candidate| candidate.current_qn),
        accept_qn: union_accept_qn(&candidates),
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
        uid: data.uid,
    })
}

/// 解析 `room_init` 响应。
pub fn parse_room_init(body: &[u8]) -> Result<RoomInit, LiveError> {
    let envelope: Envelope<RoomInitData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("room_init JSON 解析失败：{error}")))?;
    envelope.ensure_ok("room_init")?;
    let data = envelope
        .data
        .ok_or_else(|| LiveError::BadStream("room_init 响应缺少 data".to_owned()))?;
    Ok(RoomInit {
        room_id: data.room_id,
        uid: data.uid,
        live_status: data.live_status,
    })
}

/// 解析 `queryContributionRank?type=online_rank` 响应；`data.count` 即同接人数。
pub fn parse_online_count(body: &[u8]) -> Result<u64, LiveError> {
    let envelope: Envelope<OnlineRankData> = serde_json::from_slice(body)
        .map_err(|error| LiveError::BadStream(format!("同接 JSON 解析失败：{error}")))?;
    envelope.ensure_ok("同接")?;
    envelope
        .data
        .and_then(|data| data.count)
        .ok_or_else(|| LiveError::BadStream("同接响应缺少 count".to_owned()))
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

/// 收集所有可用的 `http_stream/flv` 候选；host / base_url 为空的丢弃。
fn collect_flv_candidates(streams: &[StreamInfo]) -> Vec<FlvCandidate> {
    let mut candidates = Vec::new();
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
                candidates.push(FlvCandidate {
                    url: join_url(&info.host, &codec.base_url, &info.extra),
                    codec: codec.codec_name.clone(),
                    current_qn: codec.current_qn,
                    accept_qn: codec.accept_qn.clone(),
                });
            }
        }
    }
    candidates
}

/// 选出 `current_qn` 最高的候选；同 qn 优先 HEVC（高帧率档多在 HEVC）。
fn pick_best_flv(candidates: &[FlvCandidate]) -> Option<&FlvCandidate> {
    let mut best: Option<&FlvCandidate> = None;
    for candidate in candidates {
        if best.is_none_or(|current| candidate_rank(candidate) > candidate_rank(current)) {
            best = Some(candidate);
        }
    }
    best
}

/// 候选排序键：`current_qn` 升序，HEVC 在同 qn 里更大。
fn candidate_rank(candidate: &FlvCandidate) -> (u64, bool) {
    (
        candidate.current_qn.unwrap_or(0),
        candidate.codec.eq_ignore_ascii_case("hevc"),
    )
}

/// 所有候选 `accept_qn` 的并集：去重、降序。
fn union_accept_qn(candidates: &[FlvCandidate]) -> Vec<u64> {
    let mut union: Vec<u64> = Vec::new();
    for qn in candidates
        .iter()
        .flat_map(|candidate| candidate.accept_qn.iter())
    {
        if !union.contains(qn) {
            union.push(*qn);
        }
    }
    union.sort_unstable_by(|left, right| right.cmp(left));
    union
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
    current_qn: Option<u64>,
    #[serde(default)]
    accept_qn: Vec<u64>,
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
    current_qn: Option<u64>,
    accept_qn: Vec<u64>,
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
    uid: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RoomInitData {
    #[serde(default)]
    room_id: u64,
    #[serde(default)]
    uid: Option<u64>,
    #[serde(default)]
    live_status: i64,
}

#[derive(Debug, Deserialize)]
struct OnlineRankData {
    #[serde(default)]
    count: Option<u64>,
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

    /// 多 codec：AVC current=10000、HEVC current=15000，accept_qn 有重叠。
    const MULTI_CODEC: &str = r#"{"code":0,"data":{"room_id":21622811,"live_status":1,"playurl_info":{"playurl":{"stream":[{"protocol_name":"http_stream","format":[{"format_name":"flv","codec":[
        {"codec_name":"avc","current_qn":10000,"accept_qn":[10000,400,250],"base_url":"/avc/stream.flv?","url_info":[{"host":"https://avc.invalid","extra":"token=a"}]},
        {"codec_name":"hevc","current_qn":15000,"accept_qn":[15000,10000,400,250,80],"base_url":"/hevc/stream.flv?","url_info":[{"host":"https://hevc.invalid","extra":"token=h"}]}
    ]}]}]}}}}"#;

    /// 同 qn 的两路 codec：AVC 在前，HEVC 在后。
    const TIE_CODEC: &str = r#"{"code":0,"data":{"room_id":21622811,"live_status":1,"playurl_info":{"playurl":{"stream":[{"protocol_name":"http_stream","format":[{"format_name":"flv","codec":[
        {"codec_name":"avc","current_qn":15000,"accept_qn":[15000,10000],"base_url":"/avc/stream.flv?","url_info":[{"host":"https://avc.invalid"}]},
        {"codec_name":"hevc","current_qn":15000,"accept_qn":[10000,400,15000],"base_url":"/hevc/stream.flv?","url_info":[{"host":"https://hevc.invalid"}]}
    ]}]}]}}}}"#;

    /// 请求 20000，服务端按权限只给 15000。
    const FALLBACK_CODEC: &str = r#"{"code":0,"data":{"room_id":21622811,"live_status":1,"playurl_info":{"playurl":{"stream":[{"protocol_name":"http_stream","format":[{"format_name":"flv","codec":[
        {"codec_name":"hevc","current_qn":15000,"accept_qn":[15000,10000],"base_url":"/hevc/stream.flv?","url_info":[{"host":"https://hevc.invalid","extra":"token=h"}]}
    ]}]}]}}}}"#;

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
    fn video_play_info_url_requests_both_codecs_at_the_top_quality() {
        let url = video_play_info_url(21622811);
        assert!(url.contains("room_id=21622811"));
        assert!(url.contains("only_audio=0"));
        assert!(!url.contains("only_audio=1"));
        assert!(url.contains("protocol=0"));
        assert!(url.contains("format=0"));
        assert!(url.contains("codec=0,1"));
        assert!(url.contains("qn=20000"));
    }

    #[test]
    fn video_quality_switch_requests_the_selected_qn_with_both_codecs() {
        let url = video_play_info_url_with_qn(21622811, 15_000);
        assert!(url.contains("qn=15000"), "{url}");
        assert!(url.contains("codec=0,1"), "{url}");
        assert!(url.contains("only_audio=0"), "{url}");
    }

    #[test]
    fn video_play_info_url_accepts_every_qn() {
        for qn in [20_000u64, 15_000, 10_000, 400, 250, 150] {
            let url = video_play_info_url_with_qn(21622811, qn);
            assert!(url.contains(&format!("qn={qn}")), "{url}");
            assert!(url.contains("codec=0,1"), "{url}");
            assert!(url.contains("only_audio=0"));
        }
        // 默认入口请求最高常规档 4K，服务端按权限回退。
        assert_eq!(
            video_play_info_url(21622811),
            video_play_info_url_with_qn(21622811, DEFAULT_VIDEO_QN)
        );
    }

    #[test]
    fn fetch_video_play_info_with_qn_uses_the_requested_qn() {
        let transport = RecordingTransport::new(LIVE);
        let info = fetch_video_play_info_with_auth_qn(&transport, 21622811, None, 400).unwrap();
        assert!(info.is_live());
        let urls = transport.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("qn=400"), "{}", urls[0]);
        assert!(urls[0].contains("codec=0,1"), "{}", urls[0]);
        assert!(urls[0].contains("only_audio=0"));
    }

    #[test]
    fn fetch_video_play_info_uses_the_video_url_and_parses_flv() {
        let transport = RecordingTransport::new(LIVE);
        let info = fetch_video_play_info(&transport, 21622811).unwrap();
        assert!(info.is_live());
        assert_eq!(info.room_id, 21622811);
        assert!(info.flv_url.is_some());
        let urls = transport.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("only_audio=0"));
        assert!(urls[0].contains("codec=0,1"));
        assert!(urls[0].contains("qn=20000"));
    }

    #[test]
    fn fetch_video_play_info_defaults_to_20000_and_keeps_the_server_fallback() {
        let transport = RecordingTransport::new(FALLBACK_CODEC);
        let info = fetch_video_play_info(&transport, 21622811).unwrap();
        assert_eq!(DEFAULT_VIDEO_QN, 20_000);
        // 请求 4K，服务端只给 2K：current_qn 以响应为准，不是请求值。
        assert_eq!(info.current_qn, Some(15_000));
        assert_eq!(info.accept_qn, vec![15_000, 10_000]);
        let urls = transport.urls.lock().unwrap();
        assert!(urls[0].contains("qn=20000"), "{}", urls[0]);
        assert!(urls[0].contains("codec=0,1"), "{}", urls[0]);
    }

    #[test]
    fn parse_play_info_picks_the_highest_current_qn_and_returns_hevc() {
        let info = parse_play_info(MULTI_CODEC.as_bytes(), 21622811).unwrap();
        // AVC current=10000 / HEVC current=15000 → 选 HEVC。
        assert_eq!(info.codec.as_deref(), Some("hevc"));
        assert_eq!(info.current_qn, Some(15_000));
        assert_eq!(
            info.flv_url.as_deref(),
            Some("https://hevc.invalid/hevc/stream.flv?token=h")
        );
        // union 去重降序：两路 accept_qn 合并，15000 在最前。
        assert_eq!(info.accept_qn, vec![15_000, 10_000, 400, 250, 80]);
    }

    #[test]
    fn parse_play_info_prefers_hevc_when_current_qn_ties() {
        let info = parse_play_info(TIE_CODEC.as_bytes(), 21622811).unwrap();
        // AVC 在前、同 qn 的 HEVC 在后 → 仍选 HEVC。
        assert_eq!(info.codec.as_deref(), Some("hevc"));
        assert_eq!(info.current_qn, Some(15_000));
        assert!(info.flv_url.unwrap().starts_with("https://hevc.invalid/"));
        // 并集跨候选去重：15000 / 10000 / 400。
        assert_eq!(info.accept_qn, vec![15_000, 10_000, 400]);
    }

    #[test]
    fn parse_play_info_skips_candidates_without_host_or_base_url() {
        let body = r#"{"code":0,"data":{"room_id":1,"live_status":1,"playurl_info":{"playurl":{"stream":[{"protocol_name":"http_stream","format":[{"format_name":"flv","codec":[
            {"codec_name":"avc","current_qn":20000,"accept_qn":[20000],"base_url":"","url_info":[{"host":"https://empty.invalid"}]},
            {"codec_name":"hevc","current_qn":15000,"accept_qn":[15000],"base_url":"/ok.flv?","url_info":[{"host":"https://ok.invalid","extra":"x=1"}]}
        ]}]}]}}}}"#;
        let info = parse_play_info(body.as_bytes(), 1).unwrap();
        assert_eq!(info.codec.as_deref(), Some("hevc"));
        assert_eq!(info.current_qn, Some(15_000));
        assert_eq!(info.accept_qn, vec![15_000]);
    }

    #[test]
    fn video_play_info_rejects_zero_room_before_any_request() {
        let transport = RecordingTransport::new(LIVE);
        assert!(matches!(
            fetch_video_play_info(&transport, 0).unwrap_err(),
            LiveError::InvalidRoom(_)
        ));
        assert!(transport.urls.lock().unwrap().is_empty());
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
    fn room_fixture_yields_the_anchor_uid() {
        let room = parse_room_info(ROOM.as_bytes()).unwrap();
        assert_eq!(room.uid, Some(474_595_627));

        let room =
            parse_room_info(br#"{"code":0,"data":{"room_id":1,"title":"x","live_status":1}}"#)
                .unwrap();
        assert_eq!(room.uid, None);
    }

    #[test]
    fn room_init_url_and_fixture_yield_the_anchor_uid() {
        let url = room_init_url(21622811);
        assert!(url.contains("/room/v1/Room/room_init"));
        assert!(url.contains("id=21622811"));

        let init = parse_room_init(
            br#"{"code":0,"data":{"room_id":21622811,"short_id":0,"uid":474595627,"live_status":1}}"#,
        )
        .unwrap();
        assert_eq!(init.room_id, 21622811);
        assert_eq!(init.uid, Some(474_595_627));
        assert!(init.live_status == 1);
    }

    #[test]
    fn online_rank_url_is_public_and_requests_online_rank_count() {
        let url = online_rank_url(474_595_627, 21622811);
        assert!(url.contains("queryContributionRank"));
        assert!(url.contains("ruid=474595627"));
        assert!(url.contains("room_id=21622811"));
        assert!(url.contains("page=1"));
        assert!(url.contains("page_size=10"));
        assert!(url.contains("type=online_rank"));
        assert!(!url.contains("wbi"));
    }

    #[test]
    fn online_rank_parses_count_and_rejects_bad_payloads() {
        assert_eq!(
            parse_online_count(br#"{"code":0,"message":"0","data":{"count":12345}}"#).unwrap(),
            12345
        );
        assert_eq!(
            parse_online_count(br#"{"code":0,"data":{"count":0}}"#).unwrap(),
            0
        );
        assert!(matches!(
            parse_online_count(br#"{"code":0,"data":{}}"#).unwrap_err(),
            LiveError::BadStream(_)
        ));
        assert!(matches!(
            parse_online_count(br#"{"code":-400,"message":"bad"}"#).unwrap_err(),
            LiveError::Api { code: -400, .. }
        ));
    }

    #[test]
    fn fetch_online_count_uses_the_online_rank_url() {
        let transport = RecordingTransport::new(r#"{"code":0,"data":{"count":321}}"#);
        assert_eq!(fetch_online_count(&transport, 42, 7).unwrap(), 321);
        let urls = transport.urls.lock().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(urls[0].contains("type=online_rank"));
        assert!(urls[0].contains("ruid=42"));
    }

    #[test]
    fn fetch_online_count_rejects_a_zero_uid_before_any_request() {
        let transport = RecordingTransport::new(r#"{"code":0,"data":{"count":1}}"#);
        assert!(fetch_online_count(&transport, 0, 7).is_err());
        assert!(transport.urls.lock().unwrap().is_empty());
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
