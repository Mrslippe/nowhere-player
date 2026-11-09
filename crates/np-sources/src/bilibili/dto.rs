//! B 站接口的响应结构与字段映射。
//!
//! 结构体只声明用得到的字段，其余字段由 serde 忽略；所有字段都按 `Option`
//! 解析，单个条目缺字段时只丢该条目，不会让整页搜索失败。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack};
use crate::wbi;

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    code: i64,
    message: Option<String>,
    data: Option<SearchData>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    result: Option<Vec<SearchItem>>,
}

#[derive(Debug, Deserialize)]
struct SearchItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<i64>,
    bvid: Option<String>,
    title: Option<String>,
    author: Option<String>,
    pic: Option<String>,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NavEnvelope {
    code: i64,
    message: Option<String>,
    data: Option<NavData>,
}

#[derive(Debug, Deserialize)]
struct NavData {
    wbi_img: Option<WbiImg>,
}

#[derive(Debug, Deserialize)]
struct WbiImg {
    img_url: String,
    sub_url: String,
}

/// 解析搜索响应，返回可用的音频候选。
///
/// 接口 `code` 非 0（如 -412 风控、-403 签名错）一律映射为
/// [`SourceError::Unavailable`]；`data.result` 为 `null` 时返回空列表而不是错误。
pub(crate) fn parse_search(body: &[u8]) -> Result<Vec<SourceTrack>> {
    let envelope: SearchEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("B 站搜索响应不是合法 JSON: {error}")))?;
    if envelope.code != 0 {
        return Err(SourceError::Unavailable(format!(
            "B 站搜索返回 code={}: {}",
            envelope.code,
            envelope.message.unwrap_or_default()
        )));
    }
    let items = envelope
        .data
        .and_then(|data| data.result)
        .unwrap_or_default();
    Ok(items.into_iter().filter_map(map_item).collect())
}

/// 解析 nav 响应，返回 mixin key。
///
/// nav 未登录时 `code` 为 -101，但 `data.wbi_img` 仍会返回，所以这里不看 `code`。
pub(crate) fn parse_nav(body: &[u8]) -> Result<String> {
    let envelope: NavEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("B 站 nav 响应不是合法 JSON: {error}")))?;
    let NavEnvelope {
        code,
        message,
        data,
    } = envelope;
    let wbi_img = data.and_then(|data| data.wbi_img).ok_or_else(|| {
        SourceError::Unavailable(format!(
            "B 站 nav 未返回 wbi_img（code={code}, message={}）",
            message.unwrap_or_default()
        ))
    })?;
    let img_key = wbi::key_from_url(&wbi_img.img_url)
        .ok_or_else(|| SourceError::Unavailable("B 站 nav 的 img_url 无法解析出 wbi key".into()))?;
    let sub_key = wbi::key_from_url(&wbi_img.sub_url)
        .ok_or_else(|| SourceError::Unavailable("B 站 nav 的 sub_url 无法解析出 wbi key".into()))?;
    Ok(wbi::mixin_key(img_key, sub_key))
}

fn map_item(item: SearchItem) -> Option<SourceTrack> {
    if item.kind.as_deref().is_some_and(|kind| kind != "video") {
        return None;
    }
    let bvid = item.bvid.unwrap_or_default();
    let aid = item.id.unwrap_or(0);
    let source_id = if is_bvid(&bvid) {
        bvid
    } else if aid > 0 {
        aid.to_string()
    } else {
        return None;
    };
    let duration_ms = parse_duration_ms(item.duration.as_deref().unwrap_or_default())?;
    if duration_ms == 0 || duration_ms > MAX_TRACK_DURATION_MS {
        return None;
    }
    let title = clean_title(item.title.as_deref().unwrap_or_default());
    if title.is_empty() {
        return None;
    }
    Some(SourceTrack {
        source: SourceId::Bilibili,
        source_id,
        title,
        artist: decode_entities(item.author.as_deref().unwrap_or_default())
            .trim()
            .to_string(),
        duration_ms,
        album: None,
        cover_url: normalize_cover(item.pic.as_deref().unwrap_or_default()),
    })
}

fn is_bvid(value: &str) -> bool {
    value.len() >= 12
        && value.starts_with("BV")
        && value[2..].bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// 把 B 站搜索结果的 `duration`（`m:ss` 或 `h:mm:ss`）转成毫秒。
///
/// 分钟位可能超过 59（如 `962:9` 表示 962 分 9 秒）。
pub(crate) fn parse_duration_ms(raw: &str) -> Option<u64> {
    let parts: Vec<&str> = raw.trim().split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0, parse_part(minutes)?, parse_part(seconds)?),
        [hours, minutes, seconds] => (
            parse_part(hours)?,
            parse_part(minutes)?,
            parse_part(seconds)?,
        ),
        _ => return None,
    };
    let total_seconds = hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    total_seconds.checked_mul(1000)
}

fn parse_part(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// 去掉搜索高亮标签并解码 HTML 实体。
pub(crate) fn clean_title(raw: &str) -> String {
    let stripped = raw
        .replace("<em class=\"keyword\">", "")
        .replace("</em>", "");
    decode_entities(&stripped).trim().to_string()
}

/// 解码 B 站标题 / 作者里常见的 HTML 实体（单次扫描，不会二次解码）。
pub(crate) fn decode_entities(raw: &str) -> String {
    let mut decoded = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(index) = rest.find('&') {
        decoded.push_str(&rest[..index]);
        let tail = &rest[index..];
        let Some(end) = tail.find(';') else {
            decoded.push_str(tail);
            return decoded;
        };
        match &tail[..=end] {
            "&amp;" => decoded.push('&'),
            "&lt;" => decoded.push('<'),
            "&gt;" => decoded.push('>'),
            "&quot;" => decoded.push('"'),
            "&#39;" | "&apos;" => decoded.push('\''),
            "&nbsp;" => decoded.push(' '),
            _ => {
                decoded.push('&');
                rest = &tail[1..];
                continue;
            }
        }
        rest = &tail[end + 1..];
    }
    decoded.push_str(rest);
    decoded
}

/// 封面地址归一化：B 站返回 `//i0.hdslb.com/...`，补上 `https:`。
pub(crate) fn normalize_cover(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty() {
        return None;
    }
    if let Some(rest) = url.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url.to_string());
    }
    None
}

/// 响应 JSON 顶层 `code` 是否等于给定值；非法 JSON 返回 false。
pub(crate) fn is_code(body: &[u8], expected: i64) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("code").and_then(|code| code.as_i64()))
        == Some(expected)
}

#[derive(Debug, Deserialize)]
struct ViewEnvelope {
    code: i64,
    message: Option<String>,
    data: Option<ViewData>,
}

#[derive(Debug, Deserialize)]
struct ViewData {
    bvid: Option<String>,
    cid: Option<u64>,
    pages: Option<Vec<ViewPage>>,
}

#[derive(Debug, Deserialize)]
struct ViewPage {
    cid: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PlayurlEnvelope {
    code: i64,
    message: Option<String>,
    data: Option<PlayurlData>,
}

#[derive(Debug, Deserialize)]
struct PlayurlData {
    format: Option<String>,
    dash: Option<Dash>,
    durl: Option<Vec<Durl>>,
}

#[derive(Debug, Deserialize)]
struct Dash {
    audio: Option<Vec<DashAudio>>,
}

#[derive(Debug, Deserialize)]
struct DashAudio {
    // B 站真实响应会同时给 camelCase 与 snake_case 两份；分开收，
    // 用 alias 会被 serde 判成 duplicate field。
    #[serde(default, rename = "baseUrl")]
    base_url: Option<String>,
    #[serde(default, rename = "base_url")]
    base_url_snake: Option<String>,
    bandwidth: Option<u64>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default, rename = "mime_type")]
    mime_type_snake: Option<String>,
}

impl DashAudio {
    fn url(&self) -> Option<&str> {
        self.base_url
            .as_deref()
            .or(self.base_url_snake.as_deref())
            .filter(|url| !url.is_empty())
    }

    fn mime(&self) -> &str {
        self.mime_type
            .as_deref()
            .or(self.mime_type_snake.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct Durl {
    url: Option<String>,
}

/// B 站 CDN 防盗链：缺 `Referer` 直接 403（2026-11-09 实测：
/// 无头/仅 UA 均 403，带 Referer 200）。
const BILIBILI_REFERER: &str = "https://www.bilibili.com";
const BILIBILI_ORIGIN: &str = "https://www.bilibili.com";

fn bilibili_headers() -> Vec<(String, String)> {
    vec![
        ("Referer".to_string(), BILIBILI_REFERER.to_string()),
        ("Origin".to_string(), BILIBILI_ORIGIN.to_string()),
    ]
}

/// `view` 接口里 resolve 需要的字段。
pub(crate) struct VideoInfo {
    pub(crate) bvid: String,
    pub(crate) cid: u64,
}

/// 解析 `view` 响应：优先 `data.cid`，缺了再看 `data.pages[0].cid`。
///
/// 搜索接口不返回 `cid`，所以 `resolve` 固定要调一次 `view` 补齐
/// （任务卡 §6.1）；aid 进、bvid 出也靠这一步。
pub(crate) fn parse_view(body: &[u8]) -> Result<VideoInfo> {
    let envelope: ViewEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("B 站 view 响应不是合法 JSON: {error}")))?;
    if envelope.code != 0 {
        return Err(SourceError::Unavailable(format!(
            "B 站 view 返回 code={}: {}",
            envelope.code,
            envelope.message.unwrap_or_default()
        )));
    }
    let data = envelope
        .data
        .ok_or_else(|| SourceError::Unavailable("B 站 view 响应缺少 data".into()))?;
    let bvid = data
        .bvid
        .filter(|bvid| !bvid.is_empty())
        .ok_or_else(|| SourceError::Parse("B 站 view 缺少 bvid".into()))?;
    let cid = data
        .cid
        .filter(|&cid| cid > 0)
        .or_else(|| {
            data.pages
                .as_ref()
                .and_then(|pages| pages.first())
                .and_then(|page| page.cid)
                .filter(|&cid| cid > 0)
        })
        .ok_or_else(|| SourceError::Parse("B 站 view 缺少 cid".into()))?;
    Ok(VideoInfo { bvid, cid })
}

/// 解析 `playurl` 响应，挑最高码率的可播音频。
///
/// - 有 `dash.audio`：只接受 `audio/mp4`，按 `bandwidth` 取最大；
///   DASH 存在但没有 audio/mp4 时直接报不可用，不偷偷回退 durl。
/// - 没有 DASH：回退 `durl[0]`，只接受 MP4（FLV 会被播放器拒绝）。
/// - `expires_at` 来自 URL 的 `deadline` 参数（秒级 Unix 时间戳）。
pub(crate) fn parse_playurl(body: &[u8]) -> Result<ResolvedStream> {
    let envelope: PlayurlEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("B 站 playurl 响应不是合法 JSON: {error}")))?;
    if envelope.code != 0 {
        return Err(SourceError::Unavailable(format!(
            "B 站 playurl 返回 code={}: {}",
            envelope.code,
            envelope.message.unwrap_or_default()
        )));
    }
    let data = envelope
        .data
        .ok_or_else(|| SourceError::Unavailable("B 站 playurl 响应缺少 data".into()))?;

    if let Some(audio) = data
        .dash
        .as_ref()
        .and_then(|dash| dash.audio.as_ref())
        .filter(|audio| !audio.is_empty())
    {
        let selected = audio
            .iter()
            .filter(|entry| is_audio_mp4(entry.mime()))
            .filter(|entry| entry.url().is_some())
            .max_by_key(|entry| entry.bandwidth.unwrap_or(0))
            .ok_or_else(|| {
                SourceError::Unavailable(format!(
                    "DASH 音频没有可用的 audio/mp4 地址（共 {} 条）",
                    audio.len()
                ))
            })?;
        let url = selected.url().expect("已过滤空 URL").to_string();
        return Ok(ResolvedStream {
            expires_at: parse_deadline(&url),
            url,
            mime: "audio/mp4".to_string(),
            headers: bilibili_headers(),
        });
    }

    if let Some(durl) = data.durl.as_ref().and_then(|durl| durl.first()) {
        let url = durl
            .url
            .as_deref()
            .filter(|url| !url.is_empty())
            .ok_or_else(|| SourceError::Parse("B 站 durl 缺少 url".into()))?
            .to_string();
        let mime = durl_mime(data.format.as_deref(), &url)?;
        return Ok(ResolvedStream {
            expires_at: parse_deadline(&url),
            url,
            mime,
            headers: bilibili_headers(),
        });
    }

    Err(SourceError::Unavailable(
        "B 站 playurl 既没有 DASH 音频也没有 durl".into(),
    ))
}

fn is_audio_mp4(mime: &str) -> bool {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("audio/mp4")
}

/// durl 回退的 MIME：只允许 MP4；FLV 明确拒绝（symphonia 播放器会失败）。
fn durl_mime(format: Option<&str>, url: &str) -> Result<String> {
    let format = format.unwrap_or_default().trim().to_ascii_lowercase();
    if format.starts_with("mp4") {
        return Ok("video/mp4".to_string());
    }
    if format.starts_with("flv") {
        return Err(SourceError::Unavailable(
            "B 站 durl 回退拿到 FLV，播放器无法解码".into(),
        ));
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if path.ends_with(".m4a") {
        return Ok("audio/mp4".to_string());
    }
    if path.ends_with(".mp4") {
        return Ok("video/mp4".to_string());
    }
    Err(SourceError::Unavailable(format!(
        "无法确定 B 站 durl 的 MIME（format={format:?}）"
    )))
}

/// 从下载 URL 的 `deadline` 参数解析过期时间；解析不到返回 `None`。
pub(crate) fn parse_deadline(url: &str) -> Option<SystemTime> {
    let query = url.split_once('?')?.1;
    let query = query.split('#').next().unwrap_or(query);
    let raw = query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "deadline").then_some(value)
    })?;
    let seconds: u64 = raw.parse().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minutes_seconds_and_hours() {
        assert_eq!(parse_duration_ms("3:45"), Some(225_000));
        assert_eq!(parse_duration_ms("962:9"), Some(57_729_000));
        assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms(" 0:07 "), Some(7_000));
    }

    #[test]
    fn rejects_invalid_durations() {
        for raw in ["", "LIVE", "1:xx", "1:2:3:4", "-1:00", "1:-2"] {
            assert_eq!(parse_duration_ms(raw), None, "raw={raw}");
        }
    }

    #[test]
    fn clean_title_strips_highlight_and_decodes_entities() {
        assert_eq!(
            clean_title("测试<em class=\"keyword\">搜索</em> &amp; 标题"),
            "测试搜索 & 标题"
        );
        assert_eq!(clean_title("A &#39;q&#39; &lt;b&gt;"), "A 'q' <b>");
    }

    #[test]
    fn decode_entities_leaves_unknown_entities_alone() {
        assert_eq!(decode_entities("a &unknown; b"), "a &unknown; b");
        assert_eq!(decode_entities("a &amp;lt; b"), "a &lt; b");
        assert_eq!(
            decode_entities("100% & no semicolon"),
            "100% & no semicolon"
        );
    }

    #[test]
    fn normalize_cover_handles_protocol_relative_urls() {
        assert_eq!(
            normalize_cover("//i0.hdslb.com/cover.jpg"),
            Some("https://i0.hdslb.com/cover.jpg".to_string())
        );
        assert_eq!(
            normalize_cover("https://i0.hdslb.com/cover.jpg"),
            Some("https://i0.hdslb.com/cover.jpg".to_string())
        );
        assert_eq!(normalize_cover(""), None);
        assert_eq!(normalize_cover("ftp://example.com/a.jpg"), None);
    }

    #[test]
    fn bvid_validation_requires_prefix_and_length() {
        assert!(is_bvid("BV1xx411c7mD"));
        assert!(!is_bvid("BV1"));
        assert!(!is_bvid("av1001"));
        assert!(!is_bvid("BV1xx411c7m!"));
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn parse_deadline_reads_unix_seconds() {
        assert_eq!(
            parse_deadline("https://example.invalid/a.m4s?deadline=1794234249&token=x"),
            UNIX_EPOCH.checked_add(Duration::from_secs(1_794_234_249))
        );
        assert_eq!(
            parse_deadline("https://example.invalid/a.m4s?x=1&deadline=10#frag"),
            UNIX_EPOCH.checked_add(Duration::from_secs(10))
        );
    }

    #[test]
    fn parse_deadline_missing_or_invalid_is_none() {
        assert_eq!(parse_deadline("https://example.invalid/a.m4s"), None);
        assert_eq!(
            parse_deadline("https://example.invalid/a.m4s?deadline=abc"),
            None
        );
        assert_eq!(
            parse_deadline("https://example.invalid/a.m4s?deadline="),
            None
        );
    }

    #[test]
    fn parse_view_prefers_top_level_cid() {
        let body = br#"{"code":0,"data":{"bvid":"BV1xx411c7mD","cid":11,"pages":[{"cid":22}]}}"#;
        let info = parse_view(body).expect("解析");
        assert_eq!(info.bvid, "BV1xx411c7mD");
        assert_eq!(info.cid, 11);
    }

    #[test]
    fn parse_view_falls_back_to_pages_cid() {
        let body = br#"{"code":0,"data":{"bvid":"BV1xx411c7mD","pages":[{"cid":22}]}}"#;
        let info = parse_view(body).expect("解析");
        assert_eq!(info.cid, 22);
    }

    #[test]
    fn durl_mime_rejects_flv_and_unknown_formats() {
        assert!(matches!(
            durl_mime(Some("flv720"), "https://example.invalid/a.flv"),
            Err(SourceError::Unavailable(_))
        ));
        assert!(matches!(
            durl_mime(Some("mpeg4"), "https://example.invalid/a.bin"),
            Err(SourceError::Unavailable(_))
        ));
        assert_eq!(
            durl_mime(Some("mp4"), "https://example.invalid/a.mp4").expect("mp4"),
            "video/mp4"
        );
        assert_eq!(
            durl_mime(None, "https://example.invalid/a.m4a").expect("m4a"),
            "audio/mp4"
        );
    }

    #[test]
    fn playurl_without_dash_or_durl_is_unavailable() {
        let body = br#"{"code":0,"data":{"format":"mp4"}}"#;
        assert!(matches!(
            parse_playurl(body),
            Err(SourceError::Unavailable(_))
        ));
    }

    #[test]
    fn playurl_dash_without_audio_mp4_is_unavailable() {
        let body = br#"{"code":0,"data":{"dash":{"audio":[{"baseUrl":"https://example.invalid/a.flac","bandwidth":9,"mimeType":"audio/flac"}]}}}"#;
        let error = parse_playurl(body).expect_err("不应回退 durl");
        assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
        assert!(error.to_string().contains("audio/mp4"));
    }

    #[test]
    fn dash_audio_accepts_camel_and_snake_case_fields() {
        // 真实响应同时包含 baseUrl/base_url 与 mimeType/mime_type。
        let body = br#"{"code":0,"data":{"dash":{"audio":[{"baseUrl":"https://example.invalid/camel.m4s","base_url":"https://example.invalid/snake.m4s","bandwidth":2,"mimeType":"audio/mp4","mime_type":"audio/mp4"}]}}}"#;
        let stream = parse_playurl(body).expect("解析");
        assert_eq!(stream.url, "https://example.invalid/camel.m4s");
        assert_eq!(stream.mime, "audio/mp4");
    }

    #[test]
    fn dash_audio_accepts_snake_case_only() {
        let body = br#"{"code":0,"data":{"dash":{"audio":[{"base_url":"https://example.invalid/snake.m4s","bandwidth":2,"mime_type":"audio/mp4"}]}}}"#;
        let stream = parse_playurl(body).expect("解析");
        assert_eq!(stream.url, "https://example.invalid/snake.m4s");
        assert_eq!(stream.mime, "audio/mp4");
    }

    #[test]
    fn detects_api_error_codes() {
        assert!(is_code(br#"{"code":-403}"#, -403));
        assert!(!is_code(br#"{"code":0}"#, -403));
        assert!(!is_code(b"not json", -403));
    }
}
