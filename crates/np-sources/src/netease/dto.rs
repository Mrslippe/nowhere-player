//! 网易云匿名接口（`/api/search/get` 与 `/api/song/enhance/player/url`）
//! 的响应解析。
//!
//! 这两个老接口在 2026-11-09 实测无需 cookie、无需 `weapi` 加密：
//! 搜索返回 `code: 200`；取流对免费曲目返回 320k/192k/128k 的 http 直链，
//! 对版权受限/会员曲目返回 `data[0].url: null` + `data[0].code: -110`。
//! 因此本 crate 不引入 AES/RSA。

use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::error::{Result, SourceError};
use crate::model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack};

/// 版权受限 / 需要会员时的用户可读文案（不要直接甩错误码）。
pub(crate) const UNPLAYABLE_MESSAGE: &str = "该曲目在网易云不可播放（可能版权受限或需要会员）";

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    code: i64,
    msg: Option<String>,
    message: Option<String>,
    result: Option<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    songs: Option<Vec<Song>>,
}

#[derive(Debug, Deserialize)]
struct Song {
    id: Option<u64>,
    name: Option<String>,
    /// 老接口字段，毫秒。
    duration: Option<u64>,
    /// 新接口字段，毫秒。
    dt: Option<u64>,
    /// 老接口字段。
    artists: Option<Vec<Artist>>,
    /// 新接口字段。
    ar: Option<Vec<Artist>>,
    /// 老接口字段。
    album: Option<Album>,
    /// 新接口字段。
    al: Option<Album>,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Album {
    name: Option<String>,
    #[serde(default, rename = "picUrl")]
    pic_url_camel: Option<String>,
    #[serde(default, rename = "pic_url")]
    pic_url_snake: Option<String>,
}

impl Album {
    fn pic_url(&self) -> Option<&str> {
        self.pic_url_camel
            .as_deref()
            .or(self.pic_url_snake.as_deref())
            .filter(|url| !url.is_empty())
    }
}

#[derive(Debug, Deserialize)]
struct PlayerEnvelope {
    code: i64,
    msg: Option<String>,
    message: Option<String>,
    data: Option<Vec<PlayerEntry>>,
}

#[derive(Debug, Deserialize)]
struct PlayerEntry {
    url: Option<String>,
    code: Option<i64>,
    /// 过期剩余秒数（实测 1200，不是时间戳）。
    expi: Option<u64>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// 解析搜索响应；`result.songs` 缺失 / 为空都返回空 Vec。
pub(crate) fn parse_search(body: &[u8]) -> Result<Vec<SourceTrack>> {
    let envelope: SearchEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("网易云搜索响应不是合法 JSON: {error}")))?;
    if envelope.code != 200 {
        return Err(SourceError::Unavailable(format!(
            "网易云搜索返回 code={}: {}",
            envelope.code,
            error_text(envelope.message, envelope.msg)
        )));
    }
    let songs = envelope
        .result
        .and_then(|result| result.songs)
        .unwrap_or_default();
    Ok(songs.into_iter().filter_map(map_song).collect())
}

fn map_song(song: Song) -> Option<SourceTrack> {
    let id = song.id.filter(|&id| id > 0)?;
    let title = song.name.unwrap_or_default().trim().to_string();
    if title.is_empty() {
        return None;
    }
    let duration_ms = song.dt.or(song.duration)?;
    if duration_ms == 0 || duration_ms > MAX_TRACK_DURATION_MS {
        return None;
    }
    let artists = song.ar.or(song.artists).unwrap_or_default();
    let artist = artists
        .iter()
        .filter_map(|artist| artist.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    let album = song.al.or(song.album);
    let cover_url = album
        .as_ref()
        .and_then(Album::pic_url)
        .map(httpsify)
        .filter(|url| !url.is_empty());
    let album_name = album
        .and_then(|album| album.name)
        .filter(|name| !name.trim().is_empty());
    Some(SourceTrack {
        source: SourceId::Netease,
        source_id: id.to_string(),
        title,
        artist,
        duration_ms,
        album: album_name,
        cover_url,
    })
}

#[derive(Debug, Deserialize)]
struct DetailEnvelope {
    code: i64,
    msg: Option<String>,
    message: Option<String>,
    songs: Option<Vec<DetailSong>>,
}

#[derive(Debug, Deserialize)]
struct DetailSong {
    id: Option<u64>,
    album: Option<Album>,
    al: Option<Album>,
}

/// 解析 `song/detail` 批量响应，返回 `(song_id, https 封面 URL)`。
///
/// 老接口返回 `songs[].album.picUrl`；新的可能用 `al`，两者都支持。
pub(crate) fn parse_detail(body: &[u8]) -> Result<Vec<(u64, String)>> {
    let envelope: DetailEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("网易云歌曲详情响应不是合法 JSON: {error}")))?;
    if envelope.code != 200 {
        return Err(SourceError::Unavailable(format!(
            "网易云歌曲详情返回 code={}: {}",
            envelope.code,
            error_text(envelope.message, envelope.msg)
        )));
    }
    Ok(envelope
        .songs
        .unwrap_or_default()
        .into_iter()
        .filter_map(|song| {
            let id = song.id?;
            let album = song.al.or(song.album)?;
            let url = album.pic_url()?;
            Some((id, httpsify(url)))
        })
        .collect())
}

/// 解析取流响应。
///
/// - `Ok(Some(stream))`：这个码率可用；
/// - `Ok(None)`：接口本身正常，但该码率没有地址（版权 / 会员），调用方降级；
/// - `Err(Unavailable)`：接口级错误（如 -460），不降级。
pub(crate) fn parse_player_url(body: &[u8]) -> Result<Option<ResolvedStream>> {
    parse_player_url_at(body, SystemTime::now())
}

pub(crate) fn parse_player_url_at(body: &[u8], now: SystemTime) -> Result<Option<ResolvedStream>> {
    let envelope: PlayerEnvelope = serde_json::from_slice(body)
        .map_err(|error| SourceError::Parse(format!("网易云取流响应不是合法 JSON: {error}")))?;
    if envelope.code != 200 {
        return Err(SourceError::Unavailable(format!(
            "网易云取流返回 code={}: {}",
            envelope.code,
            error_text(envelope.message, envelope.msg)
        )));
    }

    let Some(entry) = envelope.data.and_then(|data| data.into_iter().next()) else {
        return Ok(None);
    };
    if entry.code.is_some_and(|code| code != 200) {
        return Ok(None);
    }
    let Some(url) = entry.url.filter(|url| !url.is_empty()) else {
        return Ok(None);
    };

    Ok(Some(ResolvedStream {
        url: httpsify(&url),
        mime: mime_for_type(entry.kind.as_deref()).to_string(),
        expires_at: expires_at_from_expi(entry.expi, now),
        headers: Vec::new(),
    }))
}

fn error_text(message: Option<String>, msg: Option<String>) -> String {
    message.or(msg).unwrap_or_default()
}

/// 网易云 CDN 直链常是 `http://`；WebView / CSP 只接受 https。
pub(crate) fn httpsify(url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        url.to_string()
    }
}

pub(crate) fn mime_for_type(kind: Option<&str>) -> &'static str {
    match kind
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => "audio/mpeg",
    }
}

/// `expi` 是**剩余秒数**（实测 1200），不是时间戳。
pub(crate) fn expires_at_from_expi(expi: Option<u64>, now: SystemTime) -> Option<SystemTime> {
    let seconds = expi?;
    if seconds == 0 {
        return None;
    }
    now.checked_add(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn httpsify_upgrades_http_and_protocol_relative_urls() {
        assert_eq!(
            httpsify("http://m801.music.126.net/a.mp3"),
            "https://m801.music.126.net/a.mp3"
        );
        assert_eq!(
            httpsify("//m801.music.126.net/a.mp3"),
            "https://m801.music.126.net/a.mp3"
        );
        assert_eq!(
            httpsify("https://m801.music.126.net/a.mp3"),
            "https://m801.music.126.net/a.mp3"
        );
    }

    #[test]
    fn expi_is_relative_seconds() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            expires_at_from_expi(Some(1200), now),
            Some(UNIX_EPOCH + Duration::from_secs(2200))
        );
        assert_eq!(expires_at_from_expi(Some(0), now), None);
        assert_eq!(expires_at_from_expi(None, now), None);
    }

    #[test]
    fn mime_mapping_covers_mp3_m4a_and_flac() {
        assert_eq!(mime_for_type(Some("mp3")), "audio/mpeg");
        assert_eq!(mime_for_type(Some("m4a")), "audio/mp4");
        assert_eq!(mime_for_type(Some("flac")), "audio/flac");
        assert_eq!(mime_for_type(None), "audio/mpeg");
    }

    #[test]
    fn detail_parses_old_and_new_album_fields() {
        let body = r#"{"code":200,"songs":[
            {"id":1,"album":{"picUrl":"http://p1.music.126.net/a.jpg"}},
            {"id":2,"al":{"picUrl":"https://p1.music.126.net/b.jpg"}},
            {"id":3,"album":{}}
        ]}"#
        .as_bytes();
        let covers = parse_detail(body).expect("解析");
        assert_eq!(covers.len(), 2);
        assert_eq!(covers[0], (1, "https://p1.music.126.net/a.jpg".to_string()));
        assert_eq!(covers[1], (2, "https://p1.music.126.net/b.jpg".to_string()));
    }

    #[test]
    fn detail_api_error_is_unavailable() {
        let error = parse_detail(br#"{"code":-460,"message":"Cheating"}"#).expect_err("接口错误");
        assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    }

    #[test]
    fn player_url_null_is_not_an_error() {
        let body =
            r#"{"code":200,"data":[{"id":1,"url":null,"code":-110,"expi":1200,"type":null}]}"#
                .as_bytes();
        assert_eq!(parse_player_url(body).expect("解析"), None);
    }

    #[test]
    fn player_url_maps_https_mime_and_expiry() {
        let body = r#"{"code":200,"data":[{"url":"http://m801.music.126.net/a.mp3","code":200,"expi":1200,"type":"mp3"}]}"#.as_bytes();
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let stream = parse_player_url_at(body, now)
            .expect("解析")
            .expect("有地址");
        assert_eq!(stream.url, "https://m801.music.126.net/a.mp3");
        assert_eq!(stream.mime, "audio/mpeg");
        assert!(stream.headers.is_empty());
        assert_eq!(
            stream.expires_at,
            Some(UNIX_EPOCH + Duration::from_secs(2200))
        );
    }

    #[test]
    fn player_api_error_is_unavailable() {
        let body = r#"{"code":-460,"message":"Cheating","data":null}"#.as_bytes();
        let error = parse_player_url(body).expect_err("接口错误");
        assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    }

    #[test]
    fn search_filters_overlong_and_maps_old_new_fields() {
        let body = r#"{"code":200,"result":{"songs":[
            {"id":1,"name":"老字段","duration":238698,"artists":[{"name":"A"},{"name":"B"}],"album":{"picUrl":"http://p1.music.126.net/a.jpg"}},
            {"id":2,"name":"新字段","dt":200000,"ar":[{"name":"C"}],"al":{"picUrl":"https://p1.music.126.net/b.jpg"}},
            {"id":3,"name":"超长","duration":1800001,"artists":[],"album":{}},
            {"id":0,"name":"非法 id","duration":1000}
        ]}}"#.as_bytes();
        let tracks = parse_search(body).expect("解析");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].source, SourceId::Netease);
        assert_eq!(tracks[0].source_id, "1");
        assert_eq!(tracks[0].artist, "A,B");
        assert_eq!(tracks[0].duration_ms, 238_698);
        assert_eq!(
            tracks[0].cover_url.as_deref(),
            Some("https://p1.music.126.net/a.jpg")
        );
        assert_eq!(tracks[1].duration_ms, 200_000);
        assert_eq!(tracks[1].artist, "C");
    }
}
