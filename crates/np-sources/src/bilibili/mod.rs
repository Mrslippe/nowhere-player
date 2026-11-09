//! B 站音源：搜索已实现，取流地址留待 T-0035。

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
/// B 站搜索单页最多返回 50 条。
const MAX_PAGE_SIZE: usize = 50;
/// wbi key 每日更换，缓存 1 小时。
const MIXIN_KEY_TTL: Duration = Duration::from_secs(60 * 60);

/// B 站音源。
pub struct BilibiliSource {
    transport: Arc<dyn Transport>,
    mixin_key: Mutex<Option<CachedMixinKey>>,
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

    fn resolve(&self, _track: &SourceTrack) -> Result<ResolvedStream> {
        Err(SourceError::NotImplemented(
            "B 站取流地址将在 T-0035 实现".to_string(),
        ))
    }
}
