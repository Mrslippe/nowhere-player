//! B 站接口的响应结构与字段映射。
//!
//! 结构体只声明用得到的字段，其余字段由 serde 忽略；所有字段都按 `Option`
//! 解析，单个条目缺字段时只丢该条目，不会让整页搜索失败。

use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::model::{SourceId, SourceTrack};
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
    if duration_ms == 0 {
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
