//! B 站音源：搜索（T-0034）与取流地址（T-0035）。
//!
//! 下载与缓存不在这里：`BilibiliSource` 只返回 [`ResolvedStream`]，
//! 由 `download` / `cache` 模块落到本地文件。

mod dto;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::AudioSource;
use crate::error::{Result, SourceError};
use crate::model::{ResolvedStream, SourceId, SourceTrack};
use crate::transport::Transport;
use crate::wbi;

const NAV_ENDPOINT: &str = "https://api.bilibili.com/x/web-interface/nav";
const SEARCH_ENDPOINT: &str = "https://api.bilibili.com/x/web-interface/wbi/search/type";
const VIEW_ENDPOINT: &str = "https://api.bilibili.com/x/web-interface/view";
const PLAYURL_ENDPOINT: &str = "https://api.bilibili.com/x/player/playurl";
/// B 站搜索单页最多返回 50 条。
const MAX_PAGE_SIZE: usize = 50;
/// wbi key 每日更换，缓存 1 小时。
const MIXIN_KEY_TTL: Duration = Duration::from_secs(60 * 60);

/// B 站音源。
pub struct BilibiliSource {
    transport: Arc<dyn Transport>,
    mixin_key: Mutex<Option<CachedMixinKey>>,
    /// ADR-0004 §4：搜索串行，避免并发触发风控。
    search_lock: Mutex<()>,
}

struct CachedMixinKey {
    value: String,
    fetched_at: Instant,
}

impl BilibiliSource {
    /// 用给定传输层构造。测试注入夹具实现，生产注入 [`crate::ReqwestTransport`]。
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            mixin_key: Mutex::new(None),
            search_lock: Mutex::new(()),
        }
    }

    fn mixin_key(&self) -> Result<String> {
        {
            let guard = self
                .mixin_key
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cached) = guard.as_ref()
                && cached.fetched_at.elapsed() < MIXIN_KEY_TTL
            {
                return Ok(cached.value.clone());
            }
        }

        let body = self.transport.get(NAV_ENDPOINT)?;
        let mixin_key = dto::parse_nav(&body)?;
        let mut guard = self
            .mixin_key
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(CachedMixinKey {
            value: mixin_key.clone(),
            fetched_at: Instant::now(),
        });
        Ok(mixin_key)
    }
}

impl AudioSource for BilibiliSource {
    fn id(&self) -> SourceId {
        SourceId::Bilibili
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SourceTrack>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let _search_guard = self
            .search_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let page_size = limit.min(MAX_PAGE_SIZE);
        let mixin_key = self.mixin_key()?;
        let wts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        let params = vec![
            ("search_type".to_string(), "video".to_string()),
            ("keyword".to_string(), query.to_string()),
            ("page".to_string(), "1".to_string()),
            ("page_size".to_string(), page_size.to_string()),
        ];
        let signed_query = wbi::sign(&params, &mixin_key, wts);
        let url = format!("{SEARCH_ENDPOINT}?{signed_query}");
        let body = self.transport.get(&url)?;
        let tracks = dto::parse_search(&body)?;
        Ok(tracks.into_iter().take(limit).collect())
    }

    fn resolve(&self, track: &SourceTrack) -> Result<ResolvedStream> {
        // 搜索结果不带 cid，固定先调 view 补齐（任务卡 §6.1）；aid 也在这里换成 bvid。
        let video = parse_video_id(&track.source_id)?;
        let view_url = match &video {
            VideoId::Bvid(bvid) => format!("{VIEW_ENDPOINT}?bvid={bvid}"),
            VideoId::Aid(aid) => format!("{VIEW_ENDPOINT}?aid={aid}"),
        };
        let view_body = self.transport.get(&view_url)?;
        let info = dto::parse_view(&view_body)?;

        // fnval=16 才返回 DASH；fourk=1 与 Web 端保持一致。
        let playurl_url = format!(
            "{PLAYURL_ENDPOINT}?bvid={}&cid={}&fnval=16&fourk=1",
            info.bvid, info.cid
        );
        let playurl_body = self.transport.get(&playurl_url)?;
        dto::parse_playurl(&playurl_body)
    }
}

/// `source_id` 的两种合法形态：BV 号或 aid 数字。
enum VideoId {
    Bvid(String),
    Aid(u64),
}

fn parse_video_id(source_id: &str) -> Result<VideoId> {
    if source_id.len() >= 12
        && source_id.starts_with("BV")
        && source_id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
    {
        return Ok(VideoId::Bvid(source_id.to_string()));
    }
    if !source_id.is_empty() && source_id.bytes().all(|byte| byte.is_ascii_digit()) {
        let aid: u64 = source_id
            .parse()
            .map_err(|_| SourceError::Unavailable(format!("B 站 aid 非法: {source_id:?}")))?;
        if aid > 0 {
            return Ok(VideoId::Aid(aid));
        }
    }
    Err(SourceError::Unavailable(format!(
        "B 站 source_id 既不是 BV 号也不是 aid: {source_id:?}"
    )))
}
