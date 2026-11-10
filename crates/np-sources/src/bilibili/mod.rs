//! B 站音源：搜索（T-0034）与取流地址（T-0035）。
//!
//! 下载与缓存不在这里：`BilibiliSource` 只返回 [`ResolvedStream`]，
//! 由 `download` / `cache` 模块落到本地文件。

mod dto;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::AudioSource;
use crate::error::{Result, SourceError};
use crate::model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack, UpUser};
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
/// 用户名 → mid 的进程内缓存（T-0049）。
const USER_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// `(mid, keywords)` 结果短缓存；避免连按两次命令第二次就 412。
const ARCHIVE_CACHE_TTL: Duration = Duration::from_secs(30);

type UserCache = HashMap<String, (Instant, Vec<UpUser>)>;
type ArchiveCache = HashMap<(u64, String), (Instant, Vec<SourceTrack>)>;
/// 空间接口只取一页；B 站默认页大小 20，这里显式要 30。
const UP_ARCHIVE_PAGE_SIZE: usize = 30;

fn rate_limit_error() -> SourceError {
    SourceError::Unavailable("B 站限流，请稍后再试（未自动重试）".to_string())
}

/// B 站音源。
pub struct BilibiliSource {
    transport: Arc<dyn Transport>,
    mixin_key: Mutex<Option<CachedMixinKey>>,
    /// ADR-0004 §4：搜索串行，避免并发触发风控。
    search_lock: Mutex<()>,
    user_cache: Mutex<UserCache>,
    archive_cache: Mutex<ArchiveCache>,
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
            user_cache: Mutex::new(HashMap::new()),
            archive_cache: Mutex::new(HashMap::new()),
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

    /// 搜一个 `duration` 桶；-403 时刷新 wbi key 重试一次。
    fn search_bucket_body(&self, query: &str, page_size: usize, duration: u32) -> Result<Vec<u8>> {
        let body = self.search_body(query, page_size, duration)?;
        if dto::is_code(&body, -403) {
            // wbi key 每日更换；缓存内可能刚过期。清掉重取一次（ADR-0004 §10）。
            self.invalidate_mixin_key();
            self.search_body(query, page_size, duration)
        } else {
            Ok(body)
        }
    }

    fn search_body(&self, query: &str, page_size: usize, duration: u32) -> Result<Vec<u8>> {
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
            ("duration".to_string(), duration.to_string()),
        ];
        let signed_query = wbi::sign(&params, &mixin_key, wts);
        let url = format!("{SEARCH_ENDPOINT}?{signed_query}");
        self.transport.get(&url)
    }

    fn invalidate_mixin_key(&self) {
        *self
            .mixin_key
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl BilibiliSource {
    /// 用户名搜索（T-0049）；进程内缓存 1 小时。
    pub fn search_users(&self, name: &str) -> Result<Vec<UpUser>> {
        let name = name.trim();
        if name.is_empty() {
            return Err(SourceError::Unavailable("UP 用户名不能为空".to_string()));
        }
        {
            let cache = self
                .user_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((cached_at, users)) = cache.get(name)
                && cached_at.elapsed() < USER_CACHE_TTL
            {
                return Ok(users.clone());
            }
        }

        let body = self.user_search_body(name)?;
        let users = dto::parse_users(&body)?;
        let mut cache = self
            .user_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(name.to_owned(), (Instant::now(), users.clone()));
        Ok(users)
    }

    /// 列某个 UP 的投稿；一次请求，不翻页、不重试（T-0049）。
    pub fn user_archives(
        &self,
        mid: u64,
        name: &str,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>> {
        let keyword = keyword.trim().to_lowercase();
        let limit = limit.clamp(1, UP_ARCHIVE_PAGE_SIZE);
        {
            let cache = self
                .archive_cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((cached_at, tracks)) = cache.get(&(mid, keyword.clone()))
                && cached_at.elapsed() < ARCHIVE_CACHE_TTL
            {
                return Ok(tracks.iter().take(limit).cloned().collect());
            }
        }

        let url = format!(
            "https://api.bilibili.com/x/series/recArchivesByKeywords?mid={mid}&keywords={}&ps={UP_ARCHIVE_PAGE_SIZE}&pn=1",
            wbi::encode_component(&keyword)
        );
        // 硬约束：只发一次；412 原样上抛，绝不自动重试。
        let body = match self.transport.get(&url) {
            Ok(body) => body,
            Err(SourceError::Http(412)) => return Err(rate_limit_error()),
            Err(error) => return Err(error),
        };
        let tracks = dto::parse_up_archives(&body, name, UP_ARCHIVE_PAGE_SIZE)?;
        let mut cache = self
            .archive_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert((mid, keyword), (Instant::now(), tracks.clone()));
        Ok(tracks.into_iter().take(limit).collect())
    }

    fn user_search_body(&self, name: &str) -> Result<Vec<u8>> {
        let mixin_key = self.mixin_key()?;
        let wts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        let params = vec![
            ("search_type".to_string(), "bili_user".to_string()),
            ("keyword".to_string(), name.to_string()),
            ("page".to_string(), "1".to_string()),
            ("page_size".to_string(), "50".to_string()),
        ];
        let signed_query = wbi::sign(&params, &mixin_key, wts);
        let url =
            format!("https://api.bilibili.com/x/web-interface/wbi/search/type?{signed_query}");
        match self.transport.get(&url) {
            Ok(body) => Ok(body),
            Err(SourceError::Http(412)) => Err(rate_limit_error()),
            Err(error) => Err(error),
        }
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

        // 服务端 duration 单值只能是 1=<10min / 2=10–30min；
        // 两次请求合并后才等价于“30 分钟内”，客户端仍会兜底过滤。
        let mut ranked = Vec::new();
        for duration in [1u32, 2u32] {
            let body = self.search_bucket_body(query, page_size, duration)?;
            ranked.extend(dto::parse_search(&body)?);
        }
        dto::dedupe(&mut ranked);
        dto::rank_tracks(&mut ranked, query);
        Ok(ranked
            .into_iter()
            .take(limit)
            .map(|ranked| ranked.track)
            .collect())
    }

    fn search_up_users(&self, name: &str) -> Result<Vec<UpUser>> {
        self.search_users(name)
    }

    fn user_archives(
        &self,
        mid: u64,
        name: &str,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>> {
        self.user_archives(mid, name, keyword, limit)
    }

    fn resolve(&self, track: &SourceTrack) -> Result<ResolvedStream> {
        if track.duration_ms > MAX_TRACK_DURATION_MS {
            return Err(SourceError::Unavailable(
                "该曲目时长超过 30 分钟，暂不支持播放（音频文件可能过大）".to_string(),
            ));
        }
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
