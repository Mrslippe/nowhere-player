//! 网易云匿名音源：搜索与取流。
//!
//! 实测（2026-11-09）老接口 `/api/search/get` 与
//! `/api/song/enhance/player/url` 无需 cookie、无需 `weapi` 加密即可用，
//! 所以这里**没有 AES/RSA**，也没有登录态。
//!
//! 版权受限 / 会员曲目返回 `url: null` 是常态：`resolve` 会依次降级
//! 320k → 192k → 128k，仍拿不到时抛出用户可读的 [`dto::UNPLAYABLE_MESSAGE`]。

mod dto;

use std::sync::{Arc, Mutex};

use crate::AudioSource;
use crate::error::{Result, SourceError};
use crate::model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack};
use crate::transport::Transport;

const SEARCH_ENDPOINT: &str = "https://music.163.com/api/search/get";
const PLAYER_URL_ENDPOINT: &str = "https://music.163.com/api/song/enhance/player/url";
const DETAIL_ENDPOINT: &str = "https://music.163.com/api/song/detail";
/// `song/detail` 单次最多 20 个 id。
const DETAIL_BATCH: usize = 20;
/// 老接口单页上限（保守值）。
const SEARCH_LIMIT_MAX: usize = 100;
/// 取流码率降级顺序。
const QUALITY_LEVELS: [u32; 3] = [320_000, 192_000, 128_000];

/// 网易云匿名音源。
pub struct NeteaseSource {
    transport: Arc<dyn Transport>,
    /// ADR-0004 §4：搜索串行。
    search_lock: Mutex<()>,
}

impl NeteaseSource {
    /// 用给定传输层构造。测试注入夹具，生产注入 [`crate::ReqwestTransport`]。
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            search_lock: Mutex::new(()),
        }
    }
}

impl AudioSource for NeteaseSource {
    fn id(&self) -> SourceId {
        SourceId::Netease
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SourceTrack>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let _search_guard = self
            .search_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let limit = limit.min(SEARCH_LIMIT_MAX);
        let url = format!(
            "{SEARCH_ENDPOINT}?s={}&type=1&limit={limit}",
            encode_query(query)
        );
        let body = self.transport.get(&url)?;
        let mut tracks = dto::parse_search(&body)?;
        tracks.truncate(limit);
        self.enrich_covers(&mut tracks);
        Ok(tracks)
    }

    fn resolve(&self, track: &SourceTrack) -> Result<ResolvedStream> {
        if track.duration_ms > MAX_TRACK_DURATION_MS {
            return Err(SourceError::Unavailable(
                "该曲目时长超过 30 分钟，暂不支持播放（音频文件可能过大）".to_string(),
            ));
        }
        let id = parse_song_id(&track.source_id)?;

        for bitrate in QUALITY_LEVELS {
            let url = format!("{PLAYER_URL_ENDPOINT}?ids=%5B{id}%5D&br={bitrate}");
            let body = self.transport.get(&url)?;
            if let Some(stream) = dto::parse_player_url(&body)? {
                return Ok(stream);
            }
        }

        Err(SourceError::Unavailable(
            dto::UNPLAYABLE_MESSAGE.to_string(),
        ))
    }
}

impl NeteaseSource {
    /// 老搜索接口没有 `picUrl`，按 20 个一批调 `song/detail` 补齐。
    ///
    /// 封面是可选信息：详情请求失败时静默保留 `None`，不影响搜索结果。
    fn enrich_covers(&self, tracks: &mut [SourceTrack]) {
        for chunk in tracks.chunks_mut(DETAIL_BATCH) {
            let ids: Vec<String> = chunk
                .iter()
                .filter(|track| track.cover_url.is_none())
                .map(|track| track.source_id.clone())
                .collect();
            if ids.is_empty() {
                continue;
            }
            let ids_arg = format!("[{}]", ids.join(","));
            let url = format!("{DETAIL_ENDPOINT}?ids={}", encode_query(&ids_arg));
            let Ok(body) = self.transport.get(&url) else {
                continue;
            };
            let Ok(covers) = dto::parse_detail(&body) else {
                continue;
            };
            for track in chunk.iter_mut() {
                if track.cover_url.is_some() {
                    continue;
                }
                if let Some((_, cover)) = covers
                    .iter()
                    .find(|(id, _)| id.to_string() == track.source_id)
                {
                    track.cover_url = Some(cover.clone());
                }
            }
        }
    }
}

fn parse_song_id(source_id: &str) -> Result<u64> {
    if !source_id.is_empty()
        && source_id.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(id) = source_id.parse::<u64>()
        && id > 0
    {
        return Ok(id);
    }
    Err(SourceError::Unavailable(format!(
        "网易云曲目 ID 非法: {source_id:?}"
    )))
}

/// 只用于 query value 的百分号编码；字母数字与 `-_.~` 保留，空格 `%20`。
fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}
