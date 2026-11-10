//! 命令执行器（T-0018）：把 `np_core` 的解析结果变成播放器调用与终端输出。
//!
//! 这一层是 T-0017（解析）与 T-0019（渲染）之间的胶水：
//! - 只依赖 [`PlayerApi`]，不直接依赖 `PlayerHandle`；单测用 `FakeApi` 记录调用，
//!   因此执行器逻辑不碰真实音频设备（T-0018 §6.1）。
//! - 输出复用 `np_core::terminal` 的六种原语，不自造字符串协议。
//! - `cmd.exec` 同步执行，结果通过 `terminal:block` 事件推回前端。
//!
//! # 约定
//!
//! - **序号对用户一律 1 基**：`play 3` 是队列第 3 首，执行层内部转成 0 基。
//! - `vol ±N` 先读当前音量再算，结果在调用前钳到 0–100。
//! - `mode` 一次下发完整 `{ repeat, shuffle }`，避免两次调用互相覆盖。
//! - `clear` 是视图操作，由前端处理；执行器只留一条防御性错误。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use np_core::command::{Completion, complete, parse, registry};
use np_core::{
    Block, BlockAction, BlockStatus, Output, ParseError, Parsed, PlaybackMode, PlaybackState,
    PlayerError, PlayerErrorCode, QueueView, RepeatMode, SCHEMA_VERSION, SettingValue, Settings,
    SettingsWarning, Span, ThemeApplyReply, ThemeError, ThemeListEntry, ThemeSource,
    validate_setting,
};
use np_library::{
    CoverCacheUsage, Database, LibraryError, NewPlaylistItem, Playlist, PlaylistItem,
    PlaylistSummary, ScanObserver, ScanReport, Scanner, Stats, Track, cover_cache_usage,
};
use np_sources::{
    AudioSource, BilibiliSource, Downloader, HttpDownloader, MatchKind, NeteaseSource,
    ReqwestTransport, SourceError, SourceId, SourceRegistry, SourceTrack, StreamCache, Transport,
    build_query, matches_artist_in_query as match_source_artist_in_query, resolve_up_user,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::player_actor::PlayerHandle;

/// 执行器依赖的播放器能力（T-0018 §6.1）。
///
/// 生产实现是 `HandleApi`（`player_actor` 里的 `PlayerHandle` 适配）；
/// 单测实现是 `FakeApi`。执行器只按 trait 调用，所以命令映射可以整体单测。
pub(crate) trait PlayerApi {
    /// 当前完整快照。
    fn snapshot(&self) -> Result<PlaybackState, CmdError>;
    /// 打开路径并播放（同时入队）；`None` 时继续当前曲目。
    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError>;
    /// 暂停。
    fn pause(&self) -> Result<PlaybackState, CmdError>;
    /// 继续播放（等价于 `play(None)`）。
    fn resume(&self) -> Result<PlaybackState, CmdError>;
    /// 下一首。
    fn next(&self) -> Result<PlaybackState, CmdError>;
    /// 上一首。
    fn prev(&self) -> Result<PlaybackState, CmdError>;
    /// 跳到绝对播放位置（毫秒）。
    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError>;
    /// 设置音量（0.0–1.0）。
    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError>;
    /// 设置完整播放模式。
    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError>;
    /// 队列快照。
    fn queue_list(&self) -> Result<QueueView, CmdError>;
    /// 追加曲目（按给定顺序）。
    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError>;
    /// 移除播放顺序里的第 `index` 首（0 基）。
    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError>;
    /// 清空队列。
    fn queue_clear(&self) -> Result<QueueView, CmdError>;
    /// 跳到播放顺序里的第 `index` 首（0 基）并播放。
    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError>;
}

/// 执行器依赖的媒体库能力（T-0023 §6.3）。
///
/// 生产实现是 `LibraryHandle`；单测实现是 `FakeLibraryApi`。执行器只按 trait
/// 调用，所以命令映射、`E_BUSY` 与会话语义都能在不碰真实文件系统的情况下测试。
pub(crate) trait LibraryApi {
    /// 登记扫描根并返回当前根数量；已存在时回 `E_CMD_ARG`。
    fn add_root(&self, path: &str) -> Result<usize, CmdError>;
    /// 全部扫描根（绝对路径，按添加顺序）。
    fn list_roots(&self) -> Result<Vec<String>, CmdError>;
    /// 受理一次后台扫描并立即返回；进度经 `observer` 回调。
    fn scan(&self, full: bool, observer: Arc<dyn LibraryObserver>) -> Result<(), CmdError>;
    /// 在标题 / 艺人 / 专辑上检索。
    fn search(&self, keyword: &str, limit: usize) -> Result<Vec<Track>, CmdError>;
    /// 最近入库的曲目。
    fn list_recent(&self, limit: usize) -> Result<Vec<Track>, CmdError>;
    /// 曲目 / 专辑 / 艺人 / 时长 / 最近扫描。
    fn stats(&self) -> Result<Stats, CmdError>;
    /// 封面缓存用量（文件数 / 字节数，T-0026 §6.3）。
    fn covers_usage(&self) -> Result<CoverCacheUsage, CmdError>;
    /// 把最近一次 `search` 结果存进会话上下文（最多 100 条）。
    fn remember_search(&self, tracks: Vec<Track>);
    /// 当前会话上下文里的搜索结果。
    fn search_results(&self) -> Vec<Track>;
    /// 把最近一次统一搜索（本地 + 在线）存进会话上下文。
    fn remember_hits(&self, _hits: Vec<SearchHit>) {}
    /// 当前会话上下文里的统一搜索结果；默认把本地结果包成 `Local`。
    fn search_hits(&self) -> Vec<SearchHit> {
        self.search_results()
            .into_iter()
            .map(SearchHit::Local)
            .collect()
    }
    /// 按路径查元数据；`cache clear` 用它判断当前是否在播远程曲目。
    fn track_by_path(&self, _path: &str) -> Result<Option<Track>, CmdError> {
        Ok(None)
    }
    /// 批量按路径查元数据；默认逐条走 `track_by_path`，生产实现只开一次库。
    fn tracks_by_paths(&self, paths: &[String]) -> Result<HashMap<String, Track>, CmdError> {
        let mut tracks = HashMap::new();
        for path in paths {
            if let Some(track) = self.track_by_path(path)? {
                tracks.insert(path.clone(), track);
            }
        }
        Ok(tracks)
    }
    /// 最近一次渲染带序号表格的命令写入（T-0043）。
    fn set_active_listing(&self, _listing: ActiveListing) {}
    /// 当前序号上下文；没有渲染过列表时为 `None`。
    fn active_listing(&self) -> Option<ActiveListing> {
        None
    }
    /// 记住 `library list` 渲染的曲目，供 `play <n>` 解析。
    fn remember_library_listing(&self, _tracks: Vec<Track>) {}
    /// 最近一次 `library list` 的曲目。
    fn library_listing(&self) -> Vec<Track> {
        Vec::new()
    }

    fn playlist_create(&self, _name: &str) -> Result<i64, CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_rename(&self, _id: i64, _name: &str) -> Result<(), CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_delete(&self, _id: i64) -> Result<(), CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_list(&self) -> Result<Vec<PlaylistSummary>, CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_by_name(&self, _name: &str) -> Result<Option<Playlist>, CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_get(&self, _id: i64) -> Result<(Playlist, Vec<PlaylistItem>), CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_add(&self, _id: i64, _item: &NewPlaylistItem) -> Result<PlaylistItem, CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
    fn playlist_remove(&self, _id: i64, _position: i64) -> Result<(), CmdError> {
        Err(CmdError::new("E_CMD_ARG", "歌单功能不可用"))
    }
}

/// 前台观察者：后台扫描通过它把进度与结果送回 UI 线程。
pub(crate) trait LibraryObserver: Send + Sync {
    /// 每处理完一个文件调用；实现方负责节流。
    fn on_progress(&self, scanned: usize, total: usize, current: &Path);
    /// 扫描正常结束（含取消）。
    fn on_finished(&self, report: &ScanReport);
    /// 扫描无法开始或异常结束。
    fn on_failed(&self, error: &CmdError);
}

/// 什么都不做的观察者：仅用于不需要进度的场景与播放器单测。
pub(crate) struct NoopLibraryObserver;

impl LibraryObserver for NoopLibraryObserver {
    fn on_progress(&self, _scanned: usize, _total: usize, _current: &Path) {}

    fn on_finished(&self, _report: &ScanReport) {}

    fn on_failed(&self, _error: &CmdError) {}
}

/// 最近一次“带序号表格”的来源（T-0043）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ListingSource {
    Search,
    Queue,
    Library,
    Playlist(String),
}

impl ListingSource {
    /// 终端文案；越界错误也用它。
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Search => "搜索结果".to_owned(),
            Self::Queue => "播放队列".to_owned(),
            Self::Library => "媒体库".to_owned(),
            Self::Playlist(name) => format!("歌单《{name}》"),
        }
    }
}

/// 会话级序号上下文：来源 + 当时渲染的行数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveListing {
    pub(crate) source: ListingSource,
    pub(crate) len: usize,
}

/// 搜索结果硬上限（T-0023 §6.4）；`--limit` 也不能超过它。
pub(crate) const MAX_SEARCH_RESULTS: usize = 100;

/// 默认搜索结果总数（T-0046 C）。
pub(crate) const DEFAULT_SEARCH_RESULTS: usize = 20;

/// 默认每个在线源取多少条；两个源正好凑成默认总数 20。
pub(crate) const ONLINE_SEARCH_PER_SOURCE: usize = 10;

/// `--up` 无关键词时的最近投稿页大小（T-0049）。
pub(crate) const UP_ARCHIVE_PAGE_SIZE: usize = 30;

/// 一次 `search` 的解析结果（T-0046 D）。
#[derive(Debug, Clone)]
pub(crate) struct SearchOptions {
    /// 实际发给源的词；只给 `--artist` 时等于艺人名。
    pub(crate) keyword: String,
    pub(crate) artist: Option<String>,
    /// `None` = all（含本地 + 全部在线源）。
    pub(crate) source: Option<SourceId>,
    pub(crate) limit: usize,
    pub(crate) local: bool,
    pub(crate) explicit_limit: bool,
    /// 用户是否真的给了关键词（只给 artist 时 keyword 会填 artist，但不算关键词）。
    pub(crate) has_keyword: bool,
    /// `--up <用户名>`；与 `mid` 互斥。
    pub(crate) up: Option<String>,
    /// `--mid <id>`；跳过用户名解析。
    pub(crate) mid: Option<u64>,
}

impl SearchOptions {
    fn parse(parsed: &Parsed) -> Result<Self, CmdError> {
        let local = parsed.flags.contains_key("local");
        let up = match parsed.flags.get("up") {
            Some(raw) if raw.trim().is_empty() => {
                return Err(CmdError::new("E_CMD_ARG", "--up 不能为空"));
            }
            Some(raw) => Some(raw.trim().to_owned()),
            None => None,
        };
        let mid = match parsed.flags.get("mid") {
            Some(raw) => {
                let value = raw
                    .parse::<u64>()
                    .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的 --mid：{raw}")))?;
                if value == 0 {
                    return Err(CmdError::new("E_CMD_ARG", "--mid 必须大于 0"));
                }
                Some(value)
            }
            None => None,
        };
        if up.is_some() && mid.is_some() {
            return Err(CmdError::new("E_CMD_ARG", "search --up 与 --mid 互斥")
                .with_hint("--up 按用户名解析；--mid 直接指定"));
        }
        let source_raw = parsed.flags.get("source");
        if local && source_raw.is_some() {
            return Err(
                CmdError::new("E_CMD_ARG", "search --source 与 --local 互斥")
                    .with_hint("--local 只搜本地库；--source 指定在线源"),
            );
        }
        let source = match source_raw.map(String::as_str) {
            None | Some("all") => None,
            Some("bilibili") => Some(SourceId::Bilibili),
            Some("netease") => Some(SourceId::Netease),
            Some(other) => {
                return Err(CmdError::new("E_CMD_ARG", format!("未知音源：{other}"))
                    .with_hint("可用：all / bilibili / netease"));
            }
        };
        if up.is_some() || mid.is_some() {
            if local {
                return Err(
                    CmdError::new("E_CMD_ARG", "search --up/--mid 与 --local 互斥")
                        .with_hint("UP 检索只支持 B 站在线"),
                );
            }
            if parsed.flags.contains_key("artist") {
                return Err(
                    CmdError::new("E_CMD_ARG", "search --up/--mid 与 --artist 互斥")
                        .with_hint("--up 已在指定人；关键词语义由空间接口处理"),
                );
            }
            if source == Some(SourceId::Netease) {
                return Err(CmdError::new("E_CMD_ARG", "search --up/--mid 只支持 B 站")
                    .with_hint("--source 改为 all 或 bilibili"));
            }
        }

        let explicit_limit = parsed.flags.contains_key("limit");
        let limit = if let Some(raw) = parsed.flags.get("limit") {
            let value = raw.parse::<usize>().map_err(|_| {
                CmdError::new("E_CMD_ARG", format!("无效的 --limit：{raw}"))
                    .with_hint("--limit 取值范围 1-100")
            })?;
            if !(1..=MAX_SEARCH_RESULTS).contains(&value) {
                return Err(
                    CmdError::new("E_CMD_ARG", format!("--limit 超出范围：{value}"))
                        .with_hint("--limit 取值范围 1-100"),
                );
            }
            value
        } else {
            DEFAULT_SEARCH_RESULTS
        };

        let artist = match parsed.flags.get("artist") {
            Some(raw) if raw.trim().is_empty() => {
                return Err(CmdError::new("E_CMD_ARG", "--artist 不能为空"));
            }
            Some(raw) => Some(raw.trim().to_owned()),
            None => None,
        };
        let raw_keyword = parsed
            .positional
            .first()
            .map(|keyword| keyword.trim())
            .unwrap_or_default();
        let has_keyword = !raw_keyword.is_empty();
        let keyword = if raw_keyword.is_empty() {
            artist.clone().unwrap_or_default()
        } else {
            raw_keyword.to_owned()
        };
        if keyword.is_empty() && up.is_none() && mid.is_none() {
            return Err(CmdError::new("E_CMD_ARG", "搜索关键词不能为空")
                .with_hint("search <关键词>、search --artist <名> 或 search --up <用户名>"));
        }

        Ok(Self {
            keyword,
            artist,
            source,
            limit,
            local,
            explicit_limit,
            has_keyword,
            up,
            mid,
        })
    }

    #[cfg(test)]
    fn legacy(keyword: &str, limit: usize) -> Self {
        Self {
            keyword: keyword.to_owned(),
            artist: None,
            source: None,
            limit,
            local: false,
            explicit_limit: true,
            has_keyword: !keyword.trim().is_empty(),
            up: None,
            mid: None,
        }
    }

    fn per_source(&self) -> usize {
        if self.explicit_limit {
            self.limit
        } else {
            ONLINE_SEARCH_PER_SOURCE
        }
    }

    /// 默认配额下本地最多占 10 条，剩下留给在线；显式 `--limit` 不额外压本地。
    fn local_cap(&self) -> usize {
        if self.explicit_limit {
            self.limit
        } else {
            ONLINE_SEARCH_PER_SOURCE
        }
    }

    /// 带 `--artist` 时扩大候选池；无 artist 维持现有配额。
    fn candidate_pool(&self) -> usize {
        if self.artist.is_some() {
            self.limit.saturating_mul(3).clamp(1, 50)
        } else {
            self.per_source()
        }
    }

    /// 带 artist 时本地也扩大抓取，过滤后再截断。
    fn local_fetch_limit(&self) -> usize {
        if self.artist.is_some() {
            self.limit.saturating_mul(3).clamp(1, 50)
        } else {
            self.limit.saturating_add(1)
        }
    }

    /// 发给在线源的 query；构造规则集中在 `np-sources`。
    fn source_query(&self) -> String {
        build_query(&self.keyword, self.artist.as_deref())
    }

    /// 同时给关键词与 artist 时，B 站标题必须含关键词；只给 artist 时返回 None。
    fn keyword_filter(&self) -> Option<&str> {
        if self.artist.is_some() && self.has_keyword {
            Some(self.keyword.as_str())
        } else {
            None
        }
    }

    fn footer(&self, count: usize, with_source_column: bool) -> String {
        let hint = if self.local {
            if self.explicit_limit {
                format!("--limit {}", self.limit)
            } else {
                "--limit 可调".to_owned()
            }
        } else if self.explicit_limit {
            format!("--limit {}", self.limit)
        } else {
            format!("每源 {}，--limit 可调", self.per_source())
        };
        let source = if with_source_column {
            "（来源见「来源」列）"
        } else {
            ""
        };
        format!("共 {count} 条（{hint}） · 输入 play <序号> 播放{source}")
    }
}

/// 会话上下文里的一条搜索结果：本地曲目或在线曲目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchHit {
    /// 本地库曲目。
    Local(Track),
    /// 在线源曲目。
    Online(SourceTrack),
}

impl SearchHit {
    fn title(&self) -> &str {
        match self {
            Self::Local(track) => &track.title,
            Self::Online(track) => &track.title,
        }
    }

    fn artist(&self) -> &str {
        match self {
            Self::Local(track) => &track.artist,
            Self::Online(track) => &track.artist,
        }
    }

    fn duration_ms(&self) -> u64 {
        match self {
            Self::Local(track) => u64::try_from(track.duration_ms).unwrap_or(0),
            Self::Online(track) => track.duration_ms,
        }
    }

    fn source_label(&self) -> &'static str {
        match self {
            Self::Local(_) => "本地",
            Self::Online(track) => source_label(track.source),
        }
    }

    fn artist_match(&self, name: &str, keyword: Option<&str>) -> MatchKind {
        match self {
            Self::Local(track) => local_artist_match(track, name),
            Self::Online(track) => match_source_artist_in_query(track, name, keyword),
        }
    }
}

/// 本地库的 artist 是结构化字段，只比它，不比标题。
fn local_artist_match(track: &Track, name: &str) -> MatchKind {
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        MatchKind::None
    } else if track.artist.to_lowercase().contains(&wanted) {
        MatchKind::Author
    } else {
        MatchKind::None
    }
}

/// 音源在终端里的中文短名。
fn source_label(source: SourceId) -> &'static str {
    match source {
        SourceId::Bilibili => "B站",
        SourceId::Netease => "网易云",
        SourceId::Ytdlp => "yt-dlp",
    }
}

fn source_id_from_enabled_key(key: &str) -> Option<&str> {
    key.strip_prefix("source.")
        .and_then(|rest| rest.strip_suffix(".enabled"))
}

fn known_source_ids(online: &dyn OnlineApi) -> Vec<SourceId> {
    online.known_sources()
}

fn known_source_by_str(known: &[SourceId], id: &str) -> Option<SourceId> {
    known.iter().copied().find(|source| source.as_str() == id)
}

fn target_label(target: CacheTarget) -> &'static str {
    match target {
        CacheTarget::Streams => "streams",
        CacheTarget::Covers => "covers",
        CacheTarget::All => "streams + covers",
    }
}

fn source_description(source: SourceId) -> &'static str {
    match source {
        SourceId::Bilibili => "视频站，匿名搜索 + DASH 音频",
        SourceId::Netease => "音乐站，匿名搜索 + 版权受限时提示",
        SourceId::Ytdlp => "外部二进制（后续接入）",
    }
}

/// 跨源搜索的合并结果。
#[derive(Debug)]
pub(crate) struct OnlineSearch {
    /// 在线源返回的曲目（按注册顺序）。
    pub(crate) tracks: Vec<SourceTrack>,
    /// 是否所有在线源都失败（用于 hint）。
    pub(crate) all_failed: bool,
}

/// 已落盘的在线曲目。
#[derive(Debug, Clone)]
pub(crate) struct PreparedTrack {
    /// 本地缓存路径。
    pub(crate) path: PathBuf,
    /// 文件字节数。
    pub(crate) bytes: u64,
    /// 是否命中已有缓存（没有 resolve / 下载）。
    pub(crate) cached: bool,
}

/// 会话级的远程曲目元数据；不持久化，进程退出即丢。
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteMeta {
    /// 源 id（`bilibili` / `netease`）。
    source: String,
    /// 源内 id。
    source_id: String,
    /// 标题。
    title: String,
    /// 艺人。
    artist: String,
    /// 时长，毫秒。
    duration_ms: u64,
    /// 本地封面缓存 key（`np-library` 的 16 位 hex）；无封面为 `None`。
    cover_key: Option<String>,
}

/// 缓存清理目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheTarget {
    Streams,
    Covers,
    All,
}

impl CacheTarget {
    fn parse(raw: &str) -> Result<Self, CmdError> {
        match raw {
            "streams" => Ok(Self::Streams),
            "covers" => Ok(Self::Covers),
            "all" => Ok(Self::All),
            _ => Err(CmdError::new("E_CMD_ARG", format!("未知缓存目标：{raw}"))
                .with_hint("可用：streams / covers / all")),
        }
    }

    fn includes_streams(self) -> bool {
        matches!(self, Self::Streams | Self::All)
    }

    fn includes_covers(self) -> bool {
        matches!(self, Self::Covers | Self::All)
    }
}

/// 缓存统计（文件数 + 字节数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CacheUsage {
    pub(crate) streams_files: u64,
    pub(crate) streams_bytes: u64,
    pub(crate) covers_files: u64,
    pub(crate) covers_bytes: u64,
}

impl CacheUsage {
    fn total_files(self) -> u64 {
        self.streams_files.saturating_add(self.covers_files)
    }

    fn total_bytes(self) -> u64 {
        self.streams_bytes.saturating_add(self.covers_bytes)
    }
}

/// 用户名解析后的 UP（T-0049）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpResolution {
    pub(crate) mid: u64,
    pub(crate) name: Option<String>,
    pub(crate) fans: u64,
    /// 精确同名候选数；>1 时输出 --mid 提示。
    pub(crate) same_name: usize,
}

/// 命令层依赖的在线音源能力；生产实现是 [`OnlineService`]，单测用假实现。
pub(crate) trait OnlineApi: Send + Sync {
    /// 串行搜索所有在线源；单源失败在实现内隔离。
    fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError>;
    /// 只搜指定源；`None` = 全部启用源。默认实现按 `track.source` 过滤。
    fn search_all_for(
        &self,
        query: &str,
        limit: usize,
        source: Option<SourceId>,
    ) -> Result<OnlineSearch, CmdError> {
        let mut result = self.search_all(query, limit)?;
        if let Some(source) = source {
            result.tracks.retain(|track| track.source == source);
        }
        Ok(result)
    }
    /// resolve → 下载/缓存；进度按已写字节数回调。
    fn prepare(
        &self,
        track: &SourceTrack,
        progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError>;
    /// 用户名 → UP（仅 B 站）；默认不支持。
    fn resolve_up(&self, _name: &str) -> Result<UpResolution, CmdError> {
        Err(CmdError::new("E_SRC_UNAVAILABLE", "当前音源不支持 UP 检索"))
    }

    /// 列某个 UP 的投稿；默认不支持。
    fn up_archives(
        &self,
        _mid: u64,
        _name: &str,
        _keyword: &str,
        _limit: usize,
    ) -> Result<Vec<SourceTrack>, CmdError> {
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "当前音源不支持 UP 投稿检索",
        ))
    }

    /// 已知源 id（含被禁用的），顺序稳定。
    fn known_sources(&self) -> Vec<SourceId> {
        Vec::new()
    }
    /// 进程内启停一个源；下次搜索/prepare 立即生效。
    fn set_source_enabled(&self, _source: SourceId, _enabled: bool) -> Result<(), CmdError> {
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "当前在线服务不支持音源开关",
        ))
    }
    /// streams / covers 的文件数与体积。
    fn cache_usage(&self) -> Result<CacheUsage, CmdError> {
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "当前在线服务不支持缓存统计",
        ))
    }
    /// 删除目标目录下的文件（不删目录），返回释放字节数。
    fn clear_cache(&self, _target: CacheTarget) -> Result<u64, CmdError> {
        Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            "当前在线服务不支持缓存清理",
        ))
    }
}

/// 后台播放线程需要用到的播放器能力；`PlayerHandle` 实现它。
pub(crate) trait PlaybackTarget: Send + Sync {
    /// 播放本地路径。
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError>;
}

/// `theme.export` 的回执（不把 ipc 的内部结构漏给命令层以外）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThemeExportTarget {
    pub(crate) path: String,
    pub(crate) created: bool,
}

/// 命令层依赖的主题能力；生产实现是 [`ThemeHandle`]，单测用假实现。
pub(crate) trait ThemeApi: Send + Sync {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError>;
    fn apply(&self, id: &str) -> Result<ThemeApplyReply, CmdError>;
    fn export(&self, id: &str) -> Result<ThemeExportTarget, CmdError>;
    /// `theme set` 写盘成功后广播给两个窗口。
    fn broadcast(&self, id: &str);
}

/// `theme:changed` 事件名与载荷（前端监听后调用 `theme.apply`）。
pub(crate) const THEME_CHANGED_EVENT: &str = "theme:changed";

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThemeChanged {
    id: String,
}

/// 生产主题服务：复用 ipc 里的用户主题目录/加载/导出逻辑，只额外负责广播。
pub(crate) struct ThemeHandle {
    dir: PathBuf,
    app: AppHandle,
}

impl ThemeHandle {
    pub(crate) fn for_app(app: AppHandle) -> Result<Self, CmdError> {
        let dir = crate::ipc::themes_dir(&app).map_err(theme_error)?;
        Ok(Self { dir, app })
    }
}

impl ThemeApi for ThemeHandle {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError> {
        Ok(crate::ipc::list_themes(&self.dir))
    }

    fn apply(&self, id: &str) -> Result<ThemeApplyReply, CmdError> {
        crate::ipc::apply_theme(&self.dir, id).map_err(theme_error)
    }

    fn export(&self, id: &str) -> Result<ThemeExportTarget, CmdError> {
        crate::ipc::export_builtin(&self.dir, id)
            .map(|reply| ThemeExportTarget {
                path: reply.path,
                created: reply.created,
            })
            .map_err(theme_error)
    }

    fn broadcast(&self, id: &str) {
        if let Err(error) = self
            .app
            .emit(THEME_CHANGED_EVENT, ThemeChanged { id: id.to_owned() })
        {
            eprintln!("[theme] emit {THEME_CHANGED_EVENT} failed: {error}");
        }
    }
}

fn theme_error(error: ThemeError) -> CmdError {
    CmdError {
        code: error.code,
        message: error.message,
        hint: error.hint,
    }
}

/// 扫描进度节流的默认间隔：2 Hz。
pub(crate) const PROGRESS_INTERVAL_MS: u64 = 500;

/// 把 `np-library` 错误映射成稳定的 IPC 码。
fn library_error(error: LibraryError) -> CmdError {
    let code = match &error {
        LibraryError::Sqlite(_) => "E_LIB_DB",
        LibraryError::Io(_) => "E_LIB_IO",
        LibraryError::Tag { .. } => "E_LIB_TAG",
        LibraryError::WorkerPanicked => "E_LIB_SCAN",
        LibraryError::Playlist(_) => "E_CMD_ARG",
    };
    CmdError::new(code, error.to_string())
}

/// 播放器单测用的空库：不碰文件系统，搜索结果恒为空。
#[cfg(test)]
struct EmptyLibraryApi;

#[cfg(test)]
impl LibraryApi for EmptyLibraryApi {
    fn add_root(&self, _path: &str) -> Result<usize, CmdError> {
        Err(CmdError::new("E_LIB_DB", "单测没有媒体库"))
    }

    fn list_roots(&self) -> Result<Vec<String>, CmdError> {
        Ok(Vec::new())
    }

    fn scan(&self, _full: bool, _observer: Arc<dyn LibraryObserver>) -> Result<(), CmdError> {
        Err(CmdError::new("E_LIB_SCAN", "单测没有媒体库"))
    }

    fn search(&self, _keyword: &str, _limit: usize) -> Result<Vec<Track>, CmdError> {
        Ok(Vec::new())
    }

    fn list_recent(&self, _limit: usize) -> Result<Vec<Track>, CmdError> {
        Ok(Vec::new())
    }

    fn stats(&self) -> Result<Stats, CmdError> {
        Ok(Stats::default())
    }

    fn covers_usage(&self) -> Result<CoverCacheUsage, CmdError> {
        Ok(CoverCacheUsage::default())
    }

    fn remember_search(&self, _tracks: Vec<Track>) {}

    fn search_results(&self) -> Vec<Track> {
        Vec::new()
    }
}

/// 执行期错误：稳定的 IPC 码 + 一句原因 + 可选的修复提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CmdError {
    /// IPC 错误码，如 `E_CMD_ARG` / `E_UNSUPPORTED`。
    pub(crate) code: String,
    /// 一句话原因，可直接展示。
    pub(crate) message: String,
    /// 修复提示；解析错误用 `usage` 或拼写建议填充。
    pub(crate) hint: Option<String>,
}

impl CmdError {
    /// 用错误码与原因构造错误。
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
        }
    }

    /// 解析错误 → 执行错误：`hint` 优先用 `usage`，没有则用拼写建议。
    pub(crate) fn from_parse(error: &ParseError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            hint: error
                .usage
                .as_deref()
                .map(|usage| format!("用法：{usage}"))
                .or_else(|| error.suggestion.as_ref().map(|name| suggestion(name))),
        }
    }

    /// 播放器错误 → 执行错误。`hint` 由执行器补命令用法（见 `with_usage`）。
    pub(crate) fn from_player(error: PlayerError) -> Self {
        Self {
            code: player_error_code(error.code).to_owned(),
            message: error.message,
            hint: None,
        }
    }

    /// 补一条提示；已有提示时不覆盖。
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        if self.hint.is_none() {
            self.hint = Some(hint.into());
        }
        self
    }

    /// 失败时至少给一条提示：没有更具体的就用命令用法。
    pub(crate) fn with_usage(mut self, parsed: &Parsed) -> Self {
        if self.hint.is_none() {
            self.hint = Some(format!("用法：{}", parsed.usage()));
        }
        self
    }

    /// 转成终端的 `error` 原语。
    pub(crate) fn to_output(&self) -> np_core::Output {
        np_core::Output::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            hint: self.hint.clone(),
            actions: None,
        }
    }
}

/// 进程内设置单例：`<config_dir>/settings.json` + 一把锁。
///
/// 纯逻辑在 `np_core::settings`；这里只负责路径、缓存与错误码转换。
pub(crate) struct SettingsHandle {
    dir: PathBuf,
    inner: Mutex<Settings>,
    warnings: Vec<SettingsWarning>,
}

impl SettingsHandle {
    pub(crate) fn load(dir: PathBuf) -> Self {
        let (settings, warnings) = Settings::load(&dir);
        Self {
            dir,
            inner: Mutex::new(settings),
            warnings,
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn warnings(&self) -> &[SettingsWarning] {
        &self.warnings
    }

    pub(crate) fn get(&self, key: &str) -> Option<SettingValue> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
    }

    pub(crate) fn get_str(&self, key: &str) -> Option<String> {
        self.get(key)
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    pub(crate) fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|value| value.as_bool())
    }

    pub(crate) fn theme_id(&self) -> Option<String> {
        self.get_str("theme.id")
    }

    pub(crate) fn entries(&self) -> Vec<(String, SettingValue)> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(key, value)| (key.to_owned(), value.clone()))
            .collect()
    }

    pub(crate) fn enabled_source_ids(&self, known: &[SourceId]) -> HashSet<SourceId> {
        known
            .iter()
            .copied()
            .filter(|source| {
                self.get_bool(&format!("source.{source}.enabled"))
                    .unwrap_or(true)
            })
            .collect()
    }

    pub(crate) fn set_and_save(
        &self,
        key: impl Into<String>,
        value: SettingValue,
    ) -> Result<(), CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings.set(key, value);
        save_settings(&self.dir, &settings)
    }

    pub(crate) fn unset_and_save(&self, key: &str) -> Result<Option<SettingValue>, CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = settings.unset(key);
        save_settings(&self.dir, &settings)?;
        Ok(removed)
    }

    pub(crate) fn reset_and_save(&self) -> Result<(), CmdError> {
        let mut settings = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        settings.reset();
        save_settings(&self.dir, &settings)
    }
}

fn save_settings(dir: &Path, settings: &Settings) -> Result<(), CmdError> {
    settings
        .save(dir)
        .map_err(|error| CmdError::new("E_SETTINGS_IO", error.to_string()))
}

/// 当前注册表支持的全部源（`source list` 在禁用状态下也要能列出来）。
pub(crate) const KNOWN_SOURCES: [SourceId; 2] = [SourceId::Bilibili, SourceId::Netease];

/// 拼写建议的展示文案，与 `ParseError` 的 `Display` 保持一致。
fn suggestion(name: &str) -> String {
    format!("did you mean '{name}'?")
}

/// `PlayerErrorCode` 的 IPC 字符串（`np-core` 没有暴露 `as_str`）。
fn player_error_code(code: PlayerErrorCode) -> &'static str {
    match code {
        PlayerErrorCode::Decode => "E_DECODE",
        PlayerErrorCode::DeviceLost => "E_DEVICE_LOST",
        PlayerErrorCode::Unsupported => "E_UNSUPPORTED",
        PlayerErrorCode::Busy => "E_BUSY",
    }
}

/// 一条命令的执行结果（不含块壳）。
struct Outcome {
    outputs: Vec<Output>,
    footer: Option<String>,
}

/// 执行器内部结果：普通命令已完成，`library scan` 则只是“已受理”。
enum Executed {
    /// 命令已执行完，`done` 块可立即推送。
    Finished(Outcome),
    /// 后台任务已受理；调用方只推 `running` 块，后续由观察者更新同一 id。
    Accepted(Outcome),
}

/// `execute` 的结果：空输入不做任何事，否则给出要按顺序推送的块。
pub(crate) enum Execution {
    /// 纯空白输入：不是错误，也不产生块。
    Ignored,
    /// 先推 `running`（空输出），再由 `done` 覆盖同一个 `id`。
    Blocks {
        /// 开始执行时推送的空块。
        running: Box<Block>,
        /// 执行结束的最终块。
        done: Box<Block>,
    },
    /// 后台命令已受理（`library scan`）：只推 `running` 块，最终块由观察者推送。
    Accepted {
        /// 已受理的块，输出里写明“扫描已开始”。
        running: Box<Block>,
    },
}

/// 解析并执行一条终端输入，返回可推送的块。
///
/// 空输入（`Ok(None)`）不产生块；解析失败也走 `running → error` 两个块，
/// 这样前端只要按 `id` 覆盖即可。
///
/// 播放器单测不需要真实媒体库：这条入口用空实现（搜索结果恒为空）。
#[cfg(test)]
pub(crate) fn execute(api: &dyn PlayerApi, block_id: &str, input: &str) -> Execution {
    execute_with_library(
        api,
        &EmptyLibraryApi,
        block_id,
        input,
        Arc::new(NoopLibraryObserver),
    )
}

/// 带媒体库的执行入口；`observer` 只被 `library scan` 使用。
///
/// `library scan` 的后台线程可能很快结束，所以 `cmd_exec` 会在调用本函数
/// **之前**先推 `running` 块，保证进度事件不会先于它到达前端。
#[cfg(test)]
pub(crate) fn execute_with_library(
    api: &dyn PlayerApi,
    library: &dyn LibraryApi,
    block_id: &str,
    input: &str,
    observer: Arc<dyn LibraryObserver>,
) -> Execution {
    execute_with_services(api, library, Services::default(), block_id, input, observer)
}

/// 带设置 / 主题 / 在线源依赖的执行入口（生产 `cmd_exec` 用）。
pub(crate) fn execute_with_services(
    api: &dyn PlayerApi,
    library: &dyn LibraryApi,
    services: Services<'_>,
    block_id: &str,
    input: &str,
    observer: Arc<dyn LibraryObserver>,
) -> Execution {
    let parsed = match parse(input) {
        Ok(None) => return Execution::Ignored,
        Ok(Some(parsed)) => Ok(parsed),
        Err(error) => Err(CmdError::from_parse(&error)),
    };

    let started_at = now_millis();
    let running = Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at,
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: Vec::new(),
        footer: None,
        schema_version: SCHEMA_VERSION,
        actions: None,
    };

    let clock = Instant::now();
    let result = match &parsed {
        Ok(parsed) => Executor {
            api,
            library,
            observer,
            services,
        }
        .run(parsed),
        // 引号未闭合这类错误定位不到命令，没有 usage / 建议，给一句通用提示
        // 保证失败块里的 `hint` 恒不为空（T-0018 §2.5）。
        Err(error) => Err(error.clone().with_hint("输入 help 查看全部命令与用法")),
    };
    let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);

    let done = match result {
        Ok(Executed::Accepted(outcome)) => {
            return Execution::Accepted {
                running: Box::new(Block {
                    outputs: outcome.outputs,
                    footer: outcome.footer,
                    ..running
                }),
            };
        }
        Ok(Executed::Finished(outcome)) => Block {
            duration_ms: Some(duration_ms),
            status: BlockStatus::Done,
            outputs: outcome.outputs,
            footer: outcome.footer,
            ..running.clone()
        },
        Err(error) => Block {
            duration_ms: Some(duration_ms),
            status: BlockStatus::Error,
            outputs: vec![error.to_output()],
            actions: error_actions(input, parsed.as_ref().ok()),
            ..running.clone()
        },
    };

    Execution::Blocks {
        running: Box::new(running),
        done: Box::new(done),
    }
}

/// 错误块动作（T-0045 阶段 3）：只生成真实存在的命令。
///
/// - 所有非空错误块都有「重试」（原命令）。
/// - 只有 `search` 且当前不是本地搜索时，追加「只用本地搜索」；
///   关键词来自 `SearchOptions`，不伪造不存在的「跳过该目录」。
pub(crate) fn error_actions(input: &str, parsed: Option<&Parsed>) -> Option<Vec<BlockAction>> {
    let retry = input.trim();
    if retry.is_empty() {
        return None;
    }

    let mut actions = vec![BlockAction {
        label: "重试".to_owned(),
        input: retry.to_owned(),
    }];

    if let Some(parsed) = parsed
        && parsed.spec.name == "search"
        && let Ok(options) = SearchOptions::parse(parsed)
        && !options.local
        && !options.keyword.trim().is_empty()
    {
        actions.push(BlockAction {
            label: "只用本地搜索".to_owned(),
            input: format!(
                "search {} --local",
                quote_search_keyword(options.keyword.trim())
            ),
        });
    }

    Some(actions)
}

/// 关键词含空白/引号时用双引号包住，保证动作命令仍是一个 search 查询。
fn quote_search_keyword(keyword: &str) -> String {
    if keyword
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '"' || ch == '\\')
    {
        format!("\"{}\"", keyword.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        keyword.to_owned()
    }
}

/// 设置/主题/在线源这几类命令的依赖；旧测试可以全部传 `None`。
#[derive(Clone, Copy, Default)]
pub(crate) struct Services<'a> {
    pub(crate) settings: Option<&'a SettingsHandle>,
    pub(crate) theme: Option<&'a dyn ThemeApi>,
    pub(crate) online: Option<&'a dyn OnlineApi>,
}

/// 把 [`Parsed`] 映射成 [`PlayerApi`] / [`LibraryApi`] 调用并构造输出。
struct Executor<'a> {
    api: &'a dyn PlayerApi,
    library: &'a dyn LibraryApi,
    observer: Arc<dyn LibraryObserver>,
    services: Services<'a>,
}

impl Executor<'_> {
    /// 执行一条已解析的命令；失败时补上命令用法作为提示。
    fn run(&self, parsed: &Parsed) -> Result<Executed, CmdError> {
        if is_library_scan(parsed) {
            return self
                .library_scan(parsed)
                .map(Executed::Accepted)
                .map_err(|error| error.with_usage(parsed));
        }
        self.dispatch(parsed)
            .map(Executed::Finished)
            .map_err(|error| error.with_usage(parsed))
    }

    fn dispatch(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        match parsed.spec.name {
            "play" => self.play(parsed),
            "pause" => self.simple("暂停", self.api.pause()),
            "resume" => self.simple("继续播放", self.api.resume()),
            "next" => self.simple("下一首", self.api.next()),
            "prev" => self.simple("上一首", self.api.prev()),
            "seek" => self.seek(parsed),
            "vol" => self.volume(parsed),
            "queue" => self.queue(parsed),
            "mode" => self.mode(parsed),
            "library" => self.library(parsed),
            "playlist" => self.playlist(parsed),
            "search" => self.search(parsed),
            "theme" => self.theme(parsed),
            "config" => self.config(parsed),
            "source" => self.source(parsed),
            "cache" => self.cache(parsed),
            "help" => self.help(parsed),
            "version" => Ok(Self::version()),
            "clear" => Err(CmdError::new("E_CMD_ARG", "clear 由终端本地处理")
                .with_hint("clear 是视图操作，前端直接清屏，不经过后端")),
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("命令表与执行器不一致：{other}"),
            )),
        }
    }

    /// `play`：无参 = 继续；纯数字 = 按**最近一张带序号表格**解析；其余 = 文件路径。
    ///
    /// 没有列表上下文时不再回退到播放队列（T-0043 统一规则）。
    fn play(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let Some(value) = parsed.positional.first() else {
            return Ok(state_outcome("继续播放", &self.api.resume()?));
        };

        if is_unsigned_integer(value) {
            let index = user_index(value)?;
            let listing = self.library.active_listing().ok_or_else(|| {
                CmdError::new("E_CMD_ARG", "没有可用的序号列表")
                    .with_hint("先执行 search / queue list 等命令，再用序号")
            })?;

            match listing.source {
                ListingSource::Search => {
                    let hits = self.library.search_hits();
                    if index >= listing.len || index >= hits.len() {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 条，没有第 {} 条",
                                listing.source.label(),
                                listing.len.max(hits.len()),
                                index + 1
                            ),
                        ));
                    }
                    let hit = &hits[index];
                    let SearchHit::Local(track) = hit else {
                        // 正常路径由 cmd_exec 的在线分派处理；这里只防御直接调用执行器的场景。
                        return Err(CmdError::new(
                            "E_SRC_UNAVAILABLE",
                            "该结果是云曲目，请重新执行 play <序号> 以走在线下载",
                        ));
                    };
                    let state = self.api.play(Some(track.path.clone()))?;
                    let mut spans = vec![
                        bold("播放"),
                        plain(format!(" · 搜索结果第 {} 首", index + 1)),
                        dim(format!(" · {}", track.title)),
                    ];
                    if !track.artist.is_empty() {
                        spans.push(dim(format!(" · {}", track.artist)));
                    }
                    return Ok(Outcome {
                        outputs: vec![text_of(spans)],
                        footer: Some(format!(
                            "来源：搜索结果（共 {} 条） · {}",
                            listing.len.max(hits.len()),
                            state_footer(&state)
                        )),
                    });
                }
                ListingSource::Queue => {
                    let view = self.api.queue_list()?;
                    if index >= listing.len || view.tracks.get(index).is_none() {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 首，没有第 {} 首",
                                listing.source.label(),
                                listing.len.max(view.tracks.len()),
                                index + 1
                            ),
                        ));
                    }
                    let view = self.api.queue_jump(index)?;
                    return Ok(queue_jump_outcome(index, &view));
                }
                ListingSource::Library => {
                    let tracks = self.library.library_listing();
                    let Some(track) = tracks.get(index) else {
                        return Err(CmdError::new(
                            "E_CMD_ARG",
                            format!(
                                "{}只有 {} 条，没有第 {} 条",
                                listing.source.label(),
                                listing.len.max(tracks.len()),
                                index + 1
                            ),
                        ));
                    };
                    let state = self.api.play(Some(track.path.clone()))?;
                    let mut spans = vec![
                        bold("播放"),
                        plain(format!(" · 媒体库第 {} 首", index + 1)),
                        dim(format!(" · {}", track.title)),
                    ];
                    if !track.artist.is_empty() {
                        spans.push(dim(format!(" · {}", track.artist)));
                    }
                    return Ok(Outcome {
                        outputs: vec![text_of(spans)],
                        footer: Some(format!(
                            "来源：媒体库（共 {} 首） · {}",
                            listing.len.max(tracks.len()),
                            state_footer(&state)
                        )),
                    });
                }
                ListingSource::Playlist(name) => {
                    let playlist = self.playlist_by_name(&name)?;
                    return self.play_playlist_item(&playlist, index);
                }
            }
        }

        Ok(state_outcome("播放", &self.api.play(Some(value.clone()))?))
    }

    /// 播放类命令的公共收尾：一句标题 + 当前曲目 / 位置。
    fn simple(
        &self,
        headline: &str,
        result: Result<PlaybackState, CmdError>,
    ) -> Result<Outcome, CmdError> {
        Ok(state_outcome(headline, &result?))
    }

    /// `seek`：`mm:ss` / 纯毫秒是绝对位置，`±N`（秒）与 `±mm:ss` 相对当前位置。
    fn seek(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "播放位置")?;
        let target = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            parse_relative_seek(raw, state.position_ms, state.duration_ms)?
        } else {
            parse_absolute_seek(raw)?
        };

        Ok(state_outcome("跳转", &self.api.seek(target)?))
    }

    /// `vol`：`±N` 先读当前音量；无论哪种写法都在调用前钳到 0–100。
    fn volume(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let raw = positional_arg(parsed, "音量")?;
        let level = if raw.starts_with('+') || raw.starts_with('-') {
            let state = self.api.snapshot()?;
            relative_volume(raw, state.volume)?
        } else {
            raw.parse::<i64>()
                .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?
        };
        let clamped = level.clamp(0, 100);
        self.api.volume(clamped as f32 / 100.0)?;

        Ok(Outcome {
            outputs: vec![text_of(vec![bold("音量"), plain(format!(" · {clamped}%"))])],
            footer: None,
        })
    }

    /// `mode`：先取当前模式，再一次性下发完整的 `{ repeat, shuffle }`。
    fn mode(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "mode 缺少子命令"))?;
        let raw = positional_arg(parsed, "模式取值")?;
        let current = self.api.snapshot()?.mode;

        let mode = match subcommand.name {
            "repeat" => PlaybackMode {
                repeat: repeat_of(raw),
                shuffle: current.shuffle,
            },
            "shuffle" => PlaybackMode {
                repeat: current.repeat,
                shuffle: raw == "on",
            },
            other => {
                return Err(CmdError::new(
                    "E_CMD_UNKNOWN",
                    format!("mode 没有子命令：{other}"),
                ));
            }
        };

        let state = self.api.mode(mode)?;
        Ok(Outcome {
            outputs: vec![text_of(mode_spans(&state.mode))],
            footer: None,
        })
    }

    /// `queue`：`list` / `add` / `remove` / `clear` / `jump`。
    fn queue(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "queue 缺少子命令"))?;

        match subcommand.name {
            "list" => self.queue_list(),
            "add" => self.add_paths(parsed),
            "remove" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_remove(index)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已移除"),
                        plain(format!(" · 第 {} 首", index + 1)),
                    ])],
                    footer: Some(format!("队列还剩 {} 首", view.tracks.len())),
                })
            }
            "clear" => {
                let view = self.api.queue_clear()?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![bold("队列已清空")])],
                    footer: Some(format!("队列 {} 首", view.tracks.len())),
                })
            }
            "jump" => {
                let index = user_index(positional_arg(parsed, "队列序号")?)?;
                let view = self.api.queue_jump(index)?;
                Ok(jump_outcome("跳转", index, &view))
            }
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("queue 没有子命令：{other}"),
            )),
        }
    }

    /// `queue list`：元数据来自一次批量查库 + 远程会话映射；查不到回退文件名 + —。
    fn queue_list(&self) -> Result<Outcome, CmdError> {
        let view = self.api.queue_list()?;
        let metadata = self.library.tracks_by_paths(&view.tracks)?;
        let mut rows = Vec::with_capacity(view.tracks.len());
        let mut has_source = false;

        for path in &view.tracks {
            let filename = track_display(path);
            let Some(track) = metadata.get(path) else {
                rows.push(QueueRow {
                    title: filename,
                    artist: "—".to_owned(),
                    duration: "—".to_owned(),
                    source: None,
                });
                continue;
            };

            let title = if track.title.trim().is_empty() {
                filename
            } else {
                track.title.clone()
            };
            let artist = if track.artist.trim().is_empty() {
                "—".to_owned()
            } else {
                track.artist.clone()
            };
            let duration = if track.duration_ms > 0 {
                format_track_duration(track.duration_ms)
            } else {
                "—".to_owned()
            };
            let source = queue_track_source(track);
            has_source |= source.is_some();
            rows.push(QueueRow {
                title,
                artist,
                duration,
                source,
            });
        }

        let mut columns = vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ];
        if has_source {
            columns.push("来源".to_owned());
        }
        let table_rows = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let sequence = if view.index == Some(index) {
                    format!("▶ {}", index + 1)
                } else {
                    (index + 1).to_string()
                };
                let mut cells = vec![sequence, row.title, row.artist, row.duration];
                if has_source {
                    cells.push(row.source.unwrap_or_else(|| "—".to_owned()));
                }
                cells
            })
            .collect();

        let footer = if view.tracks.is_empty() {
            "队列为空 · 用 queue add <路径> 加入曲目".to_owned()
        } else {
            format!("{} 首 · 输入 play 1 开始播放", view.tracks.len())
        };
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Queue,
            len: view.tracks.len(),
        });

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns,
                rows: table_rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `queue add <path>` 收窄为真实存在的本地文件；`--from-search <n>` 从最近搜索结果入队。
    fn add_paths(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let from_search = parsed.flags.get("from-search");

        let path = if let Some(raw) = from_search {
            if !parsed.positional.is_empty() {
                return Err(CmdError::new(
                    "E_CMD_ARG",
                    "queue add 只能在 <path> 与 --from-search <n> 中二选一",
                )
                .with_hint("queue add <path> | queue add --from-search <n>"));
            }
            let index = user_index(raw)?;
            let listing = self.library.active_listing().ok_or_else(|| {
                CmdError::new("E_CMD_ARG", "没有可用的序号列表")
                    .with_hint("先执行 search / queue list 等命令，再用序号")
            })?;
            if listing.source != ListingSource::Search {
                return Err(CmdError::new("E_CMD_ARG", "当前序号列表不是搜索结果")
                    .with_hint("先执行 search <关键词>，再用 queue add --from-search <n>"));
            }
            let hits = self.library.search_hits();
            let Some(hit) = hits.get(index) else {
                return Err(CmdError::new(
                    "E_CMD_ARG",
                    format!(
                        "{}只有 {} 条，没有第 {} 条",
                        listing.source.label(),
                        listing.len.max(hits.len()),
                        index + 1
                    ),
                )
                .with_hint("先执行 search <关键词>"));
            };
            match hit {
                SearchHit::Local(track) => track.path.clone(),
                SearchHit::Online(track) => {
                    let online = self.services.online.ok_or_else(|| {
                        CmdError::new("E_SRC_UNAVAILABLE", "在线搜索结果入队需要在线服务")
                    })?;
                    // 队列只存本地路径：先把在线曲目解析/下载到缓存，再入队。
                    let prepared = online.prepare(track, &mut |_| {})?;
                    prepared.path.to_string_lossy().into_owned()
                }
            }
        } else {
            if parsed.positional.len() != 1 {
                return Err(
                    CmdError::new("E_CMD_ARG", "queue add 一次只接受一个文件路径")
                        .with_hint("queue add <path> | queue add --from-search <n>"),
                );
            }
            let path = parsed.positional[0].clone();
            if !Path::new(&path).is_file() {
                return Err(CmdError::new("E_CMD_ARG", format!("文件不存在：{path}"))
                    .with_hint("queue add 只接受本地文件；搜索结果用 --from-search <n>"));
            }
            path
        };

        let view = self.api.queue_add(vec![path])?;
        Ok(Outcome {
            outputs: vec![text_of(vec![bold("已加入 1 首")])],
            footer: Some(format!("队列现有 {} 首", view.tracks.len())),
        })
    }

    /// `playlist`：8 个子命令；所有名字按 COLLATE NOCASE 解析。
    fn playlist(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "playlist 缺少子命令"))?;
        match subcommand.name {
            "list" => {
                let playlists = self.library.playlist_list()?;
                if playlists.is_empty() {
                    return Ok(Outcome {
                        outputs: vec![text_of(vec![
                            bold("还没有歌单"),
                            dim(" · playlist create \"名字\" 新建"),
                        ])],
                        footer: None,
                    });
                }
                let rows = playlists
                    .iter()
                    .map(|playlist| vec![playlist.name.clone(), playlist.item_count.to_string()])
                    .collect();
                Ok(Outcome {
                    outputs: vec![Output::Table {
                        columns: vec!["歌单".to_owned(), "条目".to_owned()],
                        rows,
                        selectable: false,
                    }],
                    footer: Some("playlist show \"名字\" 查看条目".to_owned()),
                })
            }
            "create" => {
                let name = positional_arg(parsed, "名字")?;
                self.library.playlist_create(name)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已创建歌单"),
                        plain(format!(" · {name}")),
                    ])],
                    footer: Some("playlist add \"名字\" <序号> 加入当前列表条目".to_owned()),
                })
            }
            "rename" => {
                let old = positional_arg(parsed, "旧名")?;
                let new = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| {
                        CmdError::new("E_CMD_ARG", "缺少参数：新名")
                            .with_hint("名字含空格时用引号：playlist rename \"旧名\" \"新名\"")
                    })?;
                let playlist = self.playlist_by_name(old)?;
                self.library.playlist_rename(playlist.id, new)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已重命名"),
                        plain(format!(" · {old} → {new}")),
                    ])],
                    footer: None,
                })
            }
            "delete" => {
                let name = positional_arg(parsed, "名字")?;
                let playlist = self.playlist_by_name(name)?;
                self.library.playlist_delete(playlist.id)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已删除歌单"),
                        plain(format!(" · {name}")),
                    ])],
                    footer: None,
                })
            }
            "show" => self.playlist_show(positional_arg(parsed, "名字")?),
            "add" => {
                let name = positional_arg(parsed, "名字")?;
                let raw_index = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少参数：序号"))?;
                self.playlist_add(name, user_index(raw_index)?)
            }
            "remove" => {
                let name = positional_arg(parsed, "名字")?;
                let raw_index = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少参数：序号"))?;
                let index = user_index(raw_index)?;
                let playlist = self.playlist_by_name(name)?;
                self.library
                    .playlist_remove(playlist.id, i64::try_from(index).unwrap_or(i64::MAX))?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已从歌单移除"),
                        plain(format!(" · {name} 第 {} 首", index + 1)),
                    ])],
                    footer: None,
                })
            }
            "play" => {
                let name = positional_arg(parsed, "名字")?;
                let index = match parsed.positional.get(1) {
                    Some(raw) => user_index(raw)?,
                    None => 0,
                };
                let playlist = self.playlist_by_name(name)?;
                self.play_playlist_item(&playlist, index)
            }
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("playlist 没有子命令：{other}"),
            )),
        }
    }

    fn playlist_by_name(&self, name: &str) -> Result<Playlist, CmdError> {
        self.library.playlist_by_name(name)?.ok_or_else(|| {
            CmdError::new("E_CMD_ARG", format!("歌单不存在：{name}"))
                .with_hint("名字含空格时用引号：playlist show \"名字\"")
        })
    }

    fn playlist_show(&self, name: &str) -> Result<Outcome, CmdError> {
        let playlist = self.playlist_by_name(name)?;
        let (_, items) = self.library.playlist_get(playlist.id)?;
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Playlist(playlist.name.clone()),
            len: items.len(),
        });
        if items.is_empty() {
            return Ok(Outcome {
                outputs: vec![text_of(vec![
                    bold(format!("歌单：{}", playlist.name)),
                    dim(" · 0 首"),
                ])],
                footer: Some("playlist add \"名字\" <序号> 加入条目".to_owned()),
            });
        }
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let missing = item.kind == "local" && !Path::new(&item.path).is_file();
                let title = if missing {
                    format!("! {}", item.title)
                } else {
                    item.title.clone()
                };
                vec![
                    (index + 1).to_string(),
                    title,
                    item.artist.clone(),
                    if item.duration_ms > 0 {
                        format_track_duration(item.duration_ms)
                    } else {
                        "—".to_owned()
                    },
                    playlist_item_source(item),
                ]
            })
            .collect();
        Ok(Outcome {
            outputs: vec![
                text_of(vec![
                    bold(format!("歌单：{}", playlist.name)),
                    dim(format!(" · {} 首", items.len())),
                ]),
                Output::Table {
                    columns: vec![
                        "序号".to_owned(),
                        "标题".to_owned(),
                        "艺人".to_owned(),
                        "时长".to_owned(),
                        "来源".to_owned(),
                    ],
                    rows,
                    selectable: true,
                },
            ],
            footer: Some("输入 play <序号> 播放".to_owned()),
        })
    }

    fn playlist_add(&self, name: &str, index: usize) -> Result<Outcome, CmdError> {
        let playlist = self.playlist_by_name(name)?;
        let listing = self.library.active_listing().ok_or_else(|| {
            CmdError::new("E_CMD_ARG", "当前没有可用的列表")
                .with_hint("先 search 或 library list，再用 playlist add")
        })?;
        if index >= listing.len {
            return Err(CmdError::new(
                "E_CMD_ARG",
                format!(
                    "{}只有 {} 条，没有第 {} 条",
                    listing.source.label(),
                    listing.len,
                    index + 1
                ),
            ));
        }
        let draft = self.listing_draft(&listing, index)?;
        let added = self.library.playlist_add(playlist.id, &draft)?;
        Ok(Outcome {
            outputs: vec![text_of(vec![
                bold("已加入歌单"),
                plain(format!(" · {} 第 {} 首", playlist.name, added.position + 1)),
                dim(format!(" · {}", added.title)),
            ])],
            footer: None,
        })
    }

    fn listing_draft(
        &self,
        listing: &ActiveListing,
        index: usize,
    ) -> Result<NewPlaylistItem, CmdError> {
        match &listing.source {
            ListingSource::Search => {
                let hits = self.library.search_hits();
                let hit = hits.get(index).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("搜索结果只有 {} 条", hits.len()))
                })?;
                Ok(match hit {
                    SearchHit::Local(track) => local_draft(track),
                    SearchHit::Online(track) => remote_draft(track),
                })
            }
            ListingSource::Queue => {
                let view = self.api.queue_list()?;
                let path = view.tracks.get(index).ok_or_else(|| {
                    CmdError::new(
                        "E_CMD_ARG",
                        format!("播放队列只有 {} 首", view.tracks.len()),
                    )
                })?;
                let metadata = self.library.tracks_by_paths(std::slice::from_ref(path))?;
                Ok(match metadata.get(path) {
                    Some(track) => local_draft(track),
                    None => NewPlaylistItem {
                        kind: "local".to_owned(),
                        path: path.clone(),
                        source: String::new(),
                        source_id: String::new(),
                        title: track_display(path),
                        artist: "—".to_owned(),
                        album: String::new(),
                        duration_ms: 0,
                    },
                })
            }
            ListingSource::Library => {
                let tracks = self.library.library_listing();
                let track = tracks.get(index).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("媒体库只有 {} 条", tracks.len()))
                })?;
                Ok(local_draft(track))
            }
            ListingSource::Playlist(name) => {
                let playlist = self.playlist_by_name(name)?;
                let (_, items) = self.library.playlist_get(playlist.id)?;
                let item = items.get(index).ok_or_else(|| {
                    CmdError::new(
                        "E_CMD_ARG",
                        format!("歌单《{name}》只有 {} 首", items.len()),
                    )
                })?;
                Ok(playlist_draft(item))
            }
        }
    }

    fn play_playlist_item(&self, playlist: &Playlist, index: usize) -> Result<Outcome, CmdError> {
        let (_, items) = self.library.playlist_get(playlist.id)?;
        let item = items.get(index).ok_or_else(|| {
            CmdError::new(
                "E_CMD_ARG",
                format!("歌单《{}》里只有 {} 首", playlist.name, items.len()),
            )
        })?;
        let state = if item.kind == "local" {
            if !Path::new(&item.path).is_file() {
                return Err(CmdError::new("E_CMD_ARG", "文件不存在，可能已移动"));
            }
            self.api.play(Some(item.path.clone()))?
        } else {
            let source = source_from_str(&item.source)?;
            let track = SourceTrack {
                source,
                source_id: item.source_id.clone(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                duration_ms: u64::try_from(item.duration_ms).unwrap_or(0),
                album: (!item.album.is_empty()).then(|| item.album.clone()),
                cover_url: None,
            };
            let online = self
                .services
                .online
                .ok_or_else(|| CmdError::new("E_SRC_UNAVAILABLE", "在线服务未接入"))?;
            let prepared = online.prepare(&track, &mut |_| {})?;
            self.api
                .play(Some(prepared.path.to_string_lossy().into_owned()))?
        };
        Ok(Outcome {
            outputs: vec![text_of(vec![
                bold("播放"),
                plain(format!(" · {} 第 {} 首", playlist.name, index + 1)),
                dim(format!(" · {}", item.title)),
                dim(format!(" · 来源：{}", playlist_item_source(item))),
            ])],
            footer: Some(state_footer(&state)),
        })
    }

    /// `library`：`add` / `list` / `stat`（`scan` 在 `run` 里提前处理）。
    fn library(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let subcommand = parsed
            .subcommand
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "library 缺少子命令"))?;

        match subcommand.name {
            "add" => {
                let path = positional_arg(parsed, "目录")?;
                let roots = self.library.add_root(path)?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已添加扫描根"),
                        dim(format!(" · {path}")),
                    ])],
                    footer: Some(format!(
                        "当前 {roots} 个扫描根 · 输入 library scan 开始扫描"
                    )),
                })
            }
            "list" => self.library_list(parsed),
            "stat" => self.library_stat(),
            "scan" => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                "library scan 应由后台扫描入口处理",
            )),
            other => Err(CmdError::new(
                "E_CMD_UNKNOWN",
                format!("library 没有子命令：{other}"),
            )),
        }
    }

    /// `library list [--limit=N]`：最近入库的曲目，序号 1 基。
    fn library_list(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let limit = flag_limit(parsed, 20)?;
        let tracks = self.library.list_recent(limit)?;
        let rows = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track.title.clone(),
                    track.artist.clone(),
                    format_track_duration(track.duration_ms),
                ]
            })
            .collect();
        self.library.remember_library_listing(tracks.clone());
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Library,
            len: tracks.len(),
        });

        let footer = if tracks.is_empty() {
            "库里还没有曲目 · 先 library add <path> 再 library scan".to_owned()
        } else {
            format!(
                "最近 {} 首 · 输入 search <关键词> 可按序号播放",
                tracks.len()
            )
        };

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns: vec![
                    "序号".to_owned(),
                    "标题".to_owned(),
                    "艺人".to_owned(),
                    "时长".to_owned(),
                ],
                rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `library stat`：曲目 / 专辑 / 艺人 / 总时长 / 最近扫描 / 封面缓存。
    fn library_stat(&self) -> Result<Outcome, CmdError> {
        let stats = self.library.stats()?;
        let covers = self.library.covers_usage()?;
        let last_scan = if stats.last_scan_at == 0 {
            "从未扫描".to_owned()
        } else {
            format_relative_time(stats.last_scan_at)
        };
        let lines = [
            ("曲目", stats.tracks.to_string()),
            ("专辑", stats.albums.to_string()),
            ("艺人", stats.artists.to_string()),
            ("总时长", format_total_duration(stats.total_duration_ms)),
            ("最近扫描", last_scan),
            ("封面缓存", format_cover_cache(covers)),
        ];

        Ok(Outcome {
            outputs: lines
                .into_iter()
                .map(|(name, value)| text_of(vec![bold(name), plain(format!(" · {value}"))]))
                .collect(),
            footer: Some("输入 library list 查看最近入库".to_owned()),
        })
    }

    /// `search`：本地库搜索（在线分派由 `maybe_start_online` 处理）。
    fn search(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let options = SearchOptions::parse(parsed)?;
        let keyword = &options.keyword;
        let mut tracks = self.library.search(keyword, options.local_fetch_limit())?;
        let before_artist = tracks.len();
        if let Some(artist) = &options.artist {
            tracks.retain(|track| !matches!(local_artist_match(track, artist), MatchKind::None));
        }
        let removed = before_artist.saturating_sub(tracks.len());
        let truncated = tracks.len() > options.limit;
        tracks.truncate(options.limit);
        self.library.remember_search(tracks.clone());

        if tracks.is_empty() {
            // 空搜索也是一张（零行）Search 表：清掉旧上下文，避免 play 播到上一次结果。
            self.library.set_active_listing(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            });
            let mut outputs = vec![text_of(vec![
                bold("没有找到"),
                dim(format!(" · {keyword}")),
            ])];
            if options.artist.is_some() && before_artist > 0 && removed > 0 {
                outputs.push(hint_output(&format!(
                    "过滤前有 {before_artist} 条，被 --artist 过滤掉了 {removed} 条"
                )));
            } else {
                outputs.push(hint_output("换个关键词试试"));
            }
            return Ok(Outcome {
                outputs,
                footer: Some(options.footer(0, false)),
            });
        }
        self.library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: tracks.len(),
        });

        let rows = tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track.title.clone(),
                    track.artist.clone(),
                    format_track_duration(track.duration_ms),
                ]
            })
            .collect();
        let mut outputs = vec![
            text_of(vec![bold("搜索结果"), dim(format!(" · {keyword}"))]),
            Output::Table {
                columns: vec![
                    "序号".to_owned(),
                    "标题".to_owned(),
                    "艺人".to_owned(),
                    "时长".to_owned(),
                ],
                rows,
                selectable: true,
            },
        ];
        if truncated {
            outputs.push(text_of(vec![dim(format!(
                "结果超过 {} 条，只显示前 {} 条",
                options.limit, options.limit
            ))]));
        }

        Ok(Outcome {
            outputs,
            footer: Some(options.footer(tracks.len(), false)),
        })
    }

    /// `library scan [--full]`：受理后台扫描并立即返回（`ok` = 已受理）。
    fn library_scan(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let full = parsed.flags.contains_key("full");
        self.library.scan(full, Arc::clone(&self.observer))?;
        let (outputs, footer) = scan_started_output(full);

        Ok(Outcome { outputs, footer })
    }

    /// `theme`：列出 / 切换 / 导出。切换会写 settings 并广播 `theme:changed`。
    fn theme(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let theme = self
            .services
            .theme
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "主题服务未接入"))?;
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("set") => {
                let id = positional_arg(parsed, "主题 id")?;
                let applied = theme.apply(id)?;
                settings.set_and_save("theme.id", SettingValue::Str(applied.id.clone()))?;
                theme.broadcast(&applied.id);

                let source = match applied.source {
                    ThemeSource::Builtin => "内置",
                    ThemeSource::User => "用户",
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("主题已切换"),
                        plain(format!(" · {}", applied.name)),
                        dim(format!(" · {} · {source}", applied.id)),
                    ])],
                    footer: Some("重启后仍会恢复到这个主题".to_owned()),
                })
            }
            Some("export") => {
                let id = positional_arg(parsed, "主题 id")?;
                let target = theme.export(id)?;
                let detail = if target.created {
                    "已导出"
                } else {
                    "文件已存在，未覆盖"
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("主题导出"),
                        plain(format!(" · {detail}")),
                        dim(format!(" · {}", target.path)),
                    ])],
                    footer: Some("编辑 JSON 后可用 theme set 应用".to_owned()),
                })
            }
            _ => {
                let entries = theme.list()?;
                let current = settings.theme_id().unwrap_or_else(|| "graphite".to_owned());
                let rows = entries
                    .iter()
                    .map(|entry| {
                        vec![
                            if entry.id == current {
                                "●".to_owned()
                            } else {
                                String::new()
                            },
                            entry.id.clone(),
                            entry.name.clone(),
                            match entry.source {
                                ThemeSource::Builtin => "内置".to_owned(),
                                ThemeSource::User => "用户".to_owned(),
                            },
                            entry.skin.as_str().to_owned(),
                        ]
                    })
                    .collect();
                Ok(Outcome {
                    outputs: vec![
                        text_of(vec![bold("主题"), dim(format!(" · 当前：{current}"))]),
                        Output::Table {
                            columns: vec![
                                "当前".to_owned(),
                                "id".to_owned(),
                                "名称".to_owned(),
                                "来源".to_owned(),
                                "skin".to_owned(),
                            ],
                            rows,
                            selectable: true,
                        },
                    ],
                    footer: Some("theme set <id> 切换 · theme export <id> 导出样例".to_owned()),
                })
            }
        }
    }

    /// `config`：list / get / set / unset / reset，全部读写同一份 settings.json。
    fn config(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("get") => {
                let key = positional_arg(parsed, "key")?;
                let value = settings.get(key).ok_or_else(|| {
                    CmdError::new("E_CMD_ARG", format!("未设置：{key}"))
                        .with_hint(format!("config set {key} <value>"))
                })?;
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(key),
                        plain(format!(" = {}", value.display())),
                    ])],
                    footer: None,
                })
            }
            Some("set") => {
                let key = positional_arg(parsed, "key")?;
                let raw = parsed
                    .positional
                    .get(1)
                    .map(String::as_str)
                    .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少参数：value"))?;
                let value = SettingValue::parse(raw);
                let known_themes = self.known_theme_ids();
                let known_sources: Vec<String> = self
                    .services
                    .online
                    .map(|online| {
                        online
                            .known_sources()
                            .into_iter()
                            .map(|source| source.as_str().to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let known_sources: Vec<&str> = known_sources.iter().map(String::as_str).collect();
                validate_setting(key, &value, &known_themes, &known_sources)
                    .map_err(|message| CmdError::new("E_CMD_ARG", message))?;

                if key == "theme.id" {
                    let theme = self
                        .services
                        .theme
                        .ok_or_else(|| CmdError::new("E_CMD_ARG", "主题服务未接入"))?;
                    let id = value.as_str().unwrap_or_default();
                    let applied = theme.apply(id)?;
                    settings.set_and_save(key, value.clone())?;
                    theme.broadcast(&applied.id);
                } else if let Some(source_id) = source_id_from_enabled_key(key) {
                    let online = self
                        .services
                        .online
                        .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;
                    let source = known_source_by_str(&known_source_ids(online), source_id)
                        .ok_or_else(|| {
                            CmdError::new("E_CMD_ARG", format!("未知音源：{source_id}"))
                        })?;
                    settings.set_and_save(key, value.clone())?;
                    if let Err(error) =
                        online.set_source_enabled(source, value.as_bool().unwrap_or(true))
                    {
                        let _ = settings.unset_and_save(key);
                        return Err(error);
                    }
                } else {
                    settings.set_and_save(key, value.clone())?;
                }

                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold("已写入"),
                        plain(format!(" · {key} = {}", value.display())),
                    ])],
                    footer: None,
                })
            }
            Some("unset") => {
                let key = positional_arg(parsed, "key")?;
                let removed = settings.unset_and_save(key)?;
                if removed.is_some() {
                    if key == "theme.id" {
                        if let Some(theme) = self.services.theme {
                            theme.broadcast("graphite");
                        }
                    } else if let Some(source_id) = source_id_from_enabled_key(key)
                        && let Some(online) = self.services.online
                        && let Some(source) =
                            known_source_by_str(&known_source_ids(online), source_id)
                    {
                        online.set_source_enabled(source, true)?;
                    }
                }
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(if removed.is_some() {
                            "已移除"
                        } else {
                            "未设置"
                        }),
                        plain(format!(" · {key}")),
                    ])],
                    footer: None,
                })
            }
            Some("reset") => {
                settings.reset_and_save()?;
                if let Some(online) = self.services.online {
                    for source in known_source_ids(online) {
                        online.set_source_enabled(source, true)?;
                    }
                }
                if let Some(theme) = self.services.theme {
                    theme.broadcast("graphite");
                }
                Ok(Outcome {
                    outputs: vec![text_of(vec![bold("已恢复默认设置")])],
                    footer: Some("settings.json 已清空".to_owned()),
                })
            }
            _ => {
                let entries = settings.entries();
                if entries.is_empty() {
                    return Ok(Outcome {
                        outputs: vec![text_of(vec![
                            bold("暂无设置"),
                            dim(" · 所有项使用内置默认"),
                        ])],
                        footer: Some(format!(
                            "文件：{}",
                            np_core::settings::settings_path(settings.dir()).display()
                        )),
                    });
                }
                let mut outputs: Vec<Output> = entries
                    .iter()
                    .map(|(key, value)| {
                        text_of(vec![plain(format!("{key} = ")), bold(value.display())])
                    })
                    .collect();
                for warning in settings.warnings() {
                    outputs.push(hint_output(&warning.message));
                }
                Ok(Outcome {
                    outputs,
                    footer: Some(format!(
                        "文件：{}",
                        np_core::settings::settings_path(settings.dir()).display()
                    )),
                })
            }
        }
    }

    fn known_theme_ids(&self) -> Vec<String> {
        self.services
            .theme
            .and_then(|theme| theme.list().ok())
            .map(|entries| entries.into_iter().map(|entry| entry.id).collect())
            .unwrap_or_default()
    }

    /// `source`：list / enable / disable；写入 settings 并立即影响搜索。
    fn source(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let settings = self
            .services
            .settings
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "设置服务未接入"))?;
        let online = self
            .services
            .online
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;
        let known = online.known_sources();

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("enable") | Some("disable") => {
                let text = positional_arg(parsed, "音源 id")?;
                let source = known
                    .iter()
                    .copied()
                    .find(|source| source.as_str() == text)
                    .ok_or_else(|| {
                        CmdError::new("E_CMD_ARG", format!("未知音源：{text}")).with_hint(format!(
                            "可用：{}",
                            known
                                .iter()
                                .map(|source| source.as_str())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ))
                    })?;
                let enabled = parsed.subcommand.map(|subcommand| subcommand.name) == Some("enable");
                let key = format!("source.{}.enabled", source.as_str());
                let known_ids: Vec<&str> = known.iter().map(|source| source.as_str()).collect();
                validate_setting(&key, &SettingValue::Bool(enabled), &[], &known_ids)
                    .map_err(|message| CmdError::new("E_CMD_ARG", message))?;

                settings.set_and_save(&key, SettingValue::Bool(enabled))?;
                if let Err(error) = online.set_source_enabled(source, enabled) {
                    let _ = settings.unset_and_save(&key);
                    return Err(error);
                }

                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(if enabled { "已启用" } else { "已禁用" }),
                        plain(format!(" · {} ({})", source_label(source), source.as_str())),
                    ])],
                    footer: Some(if enabled {
                        "下一次搜索会包含该源".to_owned()
                    } else {
                        "搜索会跳过该源；已在播放/下载的曲目不受影响".to_owned()
                    }),
                })
            }
            _ => {
                let rows = known
                    .iter()
                    .map(|source| {
                        let enabled = settings
                            .get_bool(&format!("source.{}.enabled", source.as_str()))
                            .unwrap_or(true);
                        vec![
                            source.as_str().to_owned(),
                            source_label(*source).to_owned(),
                            if enabled {
                                "启用".to_owned()
                            } else {
                                "禁用".to_owned()
                            },
                            source_description(*source).to_owned(),
                        ]
                    })
                    .collect();
                Ok(Outcome {
                    outputs: vec![
                        text_of(vec![bold("在线音源")]),
                        Output::Table {
                            columns: vec![
                                "id".to_owned(),
                                "名称".to_owned(),
                                "状态".to_owned(),
                                "说明".to_owned(),
                            ],
                            rows,
                            selectable: false,
                        },
                    ],
                    footer: Some("source enable/disable <id>".to_owned()),
                })
            }
        }
    }

    /// `cache`：stat / clear；清 streams 前若正在播远程曲目先停播。
    fn cache(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let online = self
            .services
            .online
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "在线音源服务未接入"))?;

        match parsed.subcommand.map(|subcommand| subcommand.name) {
            Some("clear") => {
                let raw = parsed
                    .positional
                    .first()
                    .map(String::as_str)
                    .unwrap_or("all");
                let target = CacheTarget::parse(raw)?;
                let stopped = self.stop_remote_playback()?;
                let released = online.clear_cache(target)?;

                let mut spans = vec![
                    bold("缓存已清理"),
                    plain(format!(" · {}", target_label(target))),
                    dim(format!(" · 释放 {}", format_bytes(released))),
                ];
                if stopped {
                    spans.push(dim(" · 已先停止播放远程曲目"));
                }
                let mut outputs = vec![text_of(spans)];
                if target.includes_covers() {
                    outputs.push(hint_output(
                        "本地库封面缓存已清空，下次扫描会重建（不会丢功能）",
                    ));
                }
                Ok(Outcome {
                    outputs,
                    footer: None,
                })
            }
            _ => {
                let usage = online.cache_usage()?;
                let mut outputs = vec![
                    text_of(vec![
                        bold("streams"),
                        plain(format!(
                            " · {} 个文件 / {}",
                            usage.streams_files,
                            format_bytes(usage.streams_bytes)
                        )),
                    ]),
                    text_of(vec![
                        bold("covers"),
                        plain(format!(
                            " · {} 个文件 / {}",
                            usage.covers_files,
                            format_bytes(usage.covers_bytes)
                        )),
                    ]),
                    text_of(vec![
                        bold("合计"),
                        plain(format!(
                            " · {} 个文件 / {}",
                            usage.total_files(),
                            format_bytes(usage.total_bytes())
                        )),
                    ]),
                ];
                if usage.total_files() == 0 {
                    outputs.push(hint_output("缓存为空；播放在线曲目时会自动创建"));
                }
                Ok(Outcome {
                    outputs,
                    footer: Some("cache clear [streams|covers|all]".to_owned()),
                })
            }
        }
    }

    /// 正在播放的曲目若是远程缓存，先停止并把它从队列移除（保留其它队列项）。
    fn stop_remote_playback(&self) -> Result<bool, CmdError> {
        let state = self.api.snapshot()?;
        let Some(path) = state.track_id.as_deref() else {
            return Ok(false);
        };
        let is_remote = self
            .library
            .track_by_path(path)?
            .is_some_and(|track| track.kind == "remote");
        if !is_remote {
            return Ok(false);
        }

        let view = self.api.queue_list()?;
        if let Some(index) = view.index {
            self.api.queue_remove(index)?;
        } else {
            self.api.queue_clear()?;
        }
        Ok(true)
    }

    fn help(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let registry = registry();

        match parsed.positional.first() {
            Some(name) => {
                let Some(spec) = registry.find(name) else {
                    return Err(CmdError {
                        code: "E_CMD_UNKNOWN".to_owned(),
                        message: format!("未识别的命令：{name}"),
                        hint: registry.suggest(name).map(|spec| suggestion(spec.name)),
                    });
                };
                Ok(Outcome {
                    outputs: vec![text_of(vec![
                        bold(spec.usage),
                        plain(format!("  {}", spec.summary)),
                    ])],
                    footer: None,
                })
            }
            None => {
                let outputs = registry
                    .specs()
                    .iter()
                    .map(|spec| {
                        text_of(vec![bold(format!("{:<8}", spec.name)), plain(spec.summary)])
                    })
                    .collect();
                Ok(Outcome {
                    outputs,
                    footer: Some("输入 help <命令> 查看用法".to_owned()),
                })
            }
        }
    }

    /// `version`：执行 crate 的版本。
    fn version() -> Outcome {
        Outcome {
            outputs: vec![text_of(vec![
                bold("Nowhere Player"),
                plain(format!(" {}", env!("CARGO_PKG_VERSION"))),
            ])],
            footer: None,
        }
    }
}

/// 现在时刻（Unix 毫秒）。
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 这条命令是否是后台扫描（需要先推 `running` 再启动线程）。
fn is_library_scan(parsed: &Parsed) -> bool {
    parsed.spec.name == "library"
        && parsed
            .subcommand
            .is_some_and(|subcommand| subcommand.name == "scan")
}

/// 解析出 `library scan` 的 `--full`；不是扫描命令时返回 `None`。
fn scan_full_flag(input: &str) -> Option<bool> {
    match parse(input) {
        Ok(Some(parsed)) if is_library_scan(&parsed) => Some(parsed.flags.contains_key("full")),
        _ => None,
    }
}

/// `library scan` 的“已受理”输出：文案必须写明“扫描已开始”（T-0023 §11）。
fn scan_started_output(full: bool) -> (Vec<Output>, Option<String>) {
    let kind = if full { "全量扫描" } else { "增量扫描" };
    (
        vec![text_of(vec![bold("扫描已开始"), dim(format!(" · {kind}"))])],
        Some("进度会更新到本块；扫描完成前可继续使用其他命令".to_owned()),
    )
}

/// 读取 `--limit=N`；缺省走 `default`，非数字或 0 回 `E_CMD_ARG`。
fn flag_limit(parsed: &Parsed, default: usize) -> Result<usize, CmdError> {
    let Some(raw) = parsed.flags.get("limit") else {
        return Ok(default);
    };
    let value: usize = raw
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("limit 需要正整数，收到：{raw}")))?;
    if value == 0 {
        return Err(CmdError::new("E_CMD_ARG", "limit 必须大于 0"));
    }
    Ok(value)
}

/// 曲目时长：未知（0）显示 `--:--`。
fn format_track_duration(duration_ms: i64) -> String {
    if duration_ms <= 0 {
        return "--:--".to_owned();
    }
    format_position(u64::try_from(duration_ms).unwrap_or(0))
}

/// 库总时长：`X 小时 Y 分` / `Y 分` / `Z 秒`。
fn format_total_duration(total_ms: u64) -> String {
    let seconds = total_ms / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        format!("{hours} 小时 {minutes} 分")
    } else if minutes > 0 {
        format!("{minutes} 分")
    } else {
        format!("{seconds} 秒")
    }
}

/// 封面缓存用量文案：`N 个文件 / X MB`（1 MB = 1024² 字节，保留一位小数）。
fn format_cover_cache(usage: CoverCacheUsage) -> String {
    let megabytes = usage.bytes as f64 / (1024.0 * 1024.0);
    format!("{} 个文件 / {megabytes:.1} MB", usage.files)
}

/// 最近扫描的粗粒度相对时间；只用于终端文案，精度到天。
fn format_relative_time(timestamp_ms: i64) -> String {
    let now = i64::try_from(now_millis()).unwrap_or(i64::MAX);
    let seconds = now.saturating_sub(timestamp_ms).max(0) / 1_000;
    if seconds < 60 {
        "刚刚".to_owned()
    } else if seconds < 3_600 {
        format!("{} 分钟前", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} 小时前", seconds / 3_600)
    } else {
        format!("{} 天前", seconds / 86_400)
    }
}

struct QueueRow {
    title: String,
    artist: String,
    duration: String,
    source: Option<String>,
}

fn local_draft(track: &Track) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "local".to_owned(),
        path: track.path.clone(),
        source: String::new(),
        source_id: String::new(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
    }
}

fn remote_draft(track: &SourceTrack) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "remote".to_owned(),
        path: String::new(),
        source: track.source.as_str().to_owned(),
        source_id: track.source_id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone().unwrap_or_default(),
        duration_ms: i64::try_from(track.duration_ms).unwrap_or(i64::MAX),
    }
}

fn playlist_draft(item: &PlaylistItem) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: item.kind.clone(),
        path: item.path.clone(),
        source: item.source.clone(),
        source_id: item.source_id.clone(),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms,
    }
}

fn playlist_item_source(item: &PlaylistItem) -> String {
    if item.kind == "local" {
        return "本地".to_owned();
    }
    match item.source.as_str() {
        "bilibili" => "B站".to_owned(),
        "netease" => "网易云".to_owned(),
        other => other.to_owned(),
    }
}

fn source_from_str(raw: &str) -> Result<SourceId, CmdError> {
    match raw {
        "bilibili" => Ok(SourceId::Bilibili),
        "netease" => Ok(SourceId::Netease),
        "ytdlp" => Ok(SourceId::Ytdlp),
        other => Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            format!("歌单条目来源未知：{other}"),
        )),
    }
}

/// 队列行可显示的来源；本地库的 `local` 不单独占一列。
fn queue_track_source(track: &Track) -> Option<String> {
    match track.source.as_str() {
        "" | "local" => None,
        "bilibili" => Some("B站".to_owned()),
        "netease" => Some("网易云".to_owned()),
        other => Some(other.to_owned()),
    }
}

/// `play <n>` 命中队列时的输出：写明来源是播放队列。
fn queue_jump_outcome(index: usize, view: &QueueView) -> Outcome {
    let mut outcome = jump_outcome("播放", index, view);
    outcome.footer = Some(format!("来源：播放队列（共 {} 首）", view.tracks.len()));
    outcome
}

/// 播放类结果的简短尾部；`queue_length` 未知时给一句通用文案。
fn state_footer(state: &PlaybackState) -> String {
    state.queue_length.map_or_else(
        || "已开始播放".to_owned(),
        |length| format!("队列 {length} 首"),
    )
}

/// 取第一个位置参数。
fn positional_arg<'a>(parsed: &'a Parsed, name: &str) -> Result<&'a str, CmdError> {
    parsed
        .positional
        .first()
        .map(String::as_str)
        .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("缺少参数：{name}")))
}

/// 纯 ASCII 数字（无符号）——只有这种位置参数才是序号，其余按路径处理。
fn is_unsigned_integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 用户输入的 1 基序号 → 队列的 0 基下标（T-0018 §6.2 定死）。
fn user_index(value: &str) -> Result<usize, CmdError> {
    let number: u64 = value
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))?;
    if number == 0 {
        return Err(CmdError::new("E_CMD_ARG", "序号从 1 开始"));
    }
    usize::try_from(number - 1)
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))
}

/// `mm:ss` → 毫秒。
fn parse_mm_ss(text: &str) -> Option<u64> {
    let (minutes, seconds) = text.split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds.checked_mul(1_000)?)
}

/// 绝对位置：带冒号按 `mm:ss`，否则按毫秒。
fn parse_absolute_seek(raw: &str) -> Result<u64, CmdError> {
    if raw.contains(':') {
        parse_mm_ss(raw).ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    } else {
        raw.parse::<u64>()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    }
}

/// 相对位置：`±N` 是秒，`±mm:ss` 是分钟秒；结果钳在 `[0, duration]`。
fn parse_relative_seek(
    raw: &str,
    current_ms: u64,
    duration_ms: Option<u64>,
) -> Result<u64, CmdError> {
    let (negative, body) = match raw.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };

    let delta_ms: i128 = if body.contains(':') {
        i128::from(
            parse_mm_ss(body)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    } else {
        let seconds: u64 = body
            .parse()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?;
        i128::from(
            seconds
                .checked_mul(1_000)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    };

    let target = i128::from(current_ms) + if negative { -delta_ms } else { delta_ms };
    let upper = duration_ms.map_or(i128::MAX, i128::from);

    Ok(u64::try_from(target.clamp(0, upper)).unwrap_or(0))
}

/// 相对音量：当前百分比（0–100）+ 增量，钳制留给调用方。
fn relative_volume(raw: &str, current: f32) -> Result<i64, CmdError> {
    let delta: i64 = raw
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?;
    let current_percent = (current * 100.0).round().clamp(0.0, 100.0) as i64;

    Ok(current_percent + delta)
}

/// 枚举值 → 循环模式（解析层已归一化，未知值按 `off` 兜底）。
fn repeat_of(value: &str) -> RepeatMode {
    match value {
        "one" => RepeatMode::One,
        "all" => RepeatMode::All,
        _ => RepeatMode::Off,
    }
}

fn plain(text: impl Into<String>) -> Span {
    Span::plain(text)
}

fn bold(text: impl Into<String>) -> Span {
    Span {
        bold: true,
        ..Span::plain(text)
    }
}

fn dim(text: impl Into<String>) -> Span {
    Span {
        dim: true,
        ..Span::plain(text)
    }
}

fn text_of(spans: Vec<Span>) -> Output {
    Output::Text { spans, tone: None }
}

/// 播放类命令的输出：标题 + 曲目名 + 位置/时长。
fn state_outcome(headline: &str, state: &PlaybackState) -> Outcome {
    let mut spans = vec![bold(headline)];

    if let Some(track) = state.track_id.as_deref() {
        spans.push(dim(format!(" · {}", track_display(track))));
        let position = format_position(state.position_ms);
        let duration = state
            .duration_ms
            .map_or_else(|| "--:--".to_owned(), format_position);
        spans.push(dim(format!(" · {position} / {duration}")));
    }

    let footer = match state.queue_length {
        Some(length) if length > 0 => Some(format!("队列 {length} 首")),
        _ => None,
    };

    Outcome {
        outputs: vec![text_of(spans)],
        footer,
    }
}

/// 队列跳转类命令的输出；序号对用户按 1 基显示。
fn jump_outcome(headline: &str, index: usize, view: &QueueView) -> Outcome {
    let mut spans = vec![bold(headline), plain(format!(" · 第 {} 首", index + 1))];
    if let Some(track) = view.tracks.get(index) {
        spans.push(dim(format!(" · {}", track_display(track))));
    }

    Outcome {
        outputs: vec![text_of(spans)],
        footer: Some(format!("队列 {} 首", view.tracks.len())),
    }
}

fn mode_spans(mode: &PlaybackMode) -> Vec<Span> {
    let repeat = match mode.repeat {
        RepeatMode::Off => "关",
        RepeatMode::One => "单曲",
        RepeatMode::All => "列表",
    };
    let shuffle = if mode.shuffle { "开" } else { "关" };

    vec![
        bold("模式"),
        plain(format!(" · 循环：{repeat} · 随机：{shuffle}")),
    ]
}

/// 曲目 id（本地文件为绝对路径）在终端里只显示文件名。
fn track_display(track_id: &str) -> String {
    std::path::Path::new(track_id)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| track_id.to_owned())
}

/// 毫秒 → `mm:ss`（超过一小时时分钟继续累加）。
fn format_position(position_ms: u64) -> String {
    let total_seconds = position_ms / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// `terminal:block` 事件名。
///
/// Tauri 2 的事件名只允许字母数字与 `- / : _`，**点号非法**，
/// 所以这里用冒号（`cmd.exec` 是命令名，不受同一限制）。
pub(crate) const EVENT_TERMINAL_BLOCK: &str = "terminal:block";

/// 扫描进度节流器：按**时间**而不是文件数放行（T-0023 §6.2）。
///
/// 时间由调用方注入，单测用合成时钟，不依赖真实睡眠。
struct TimeThrottle {
    interval_ms: u64,
    last_emitted_ms: Option<u64>,
}

impl TimeThrottle {
    const fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_emitted_ms: None,
        }
    }

    /// 第一次调用立即放行，之后每满 `interval_ms` 放行一次。
    fn allow(&mut self, now_ms: u64) -> bool {
        match self.last_emitted_ms {
            None => {
                self.last_emitted_ms = Some(now_ms);
                true
            }
            Some(last) if now_ms.saturating_sub(last) >= self.interval_ms => {
                self.last_emitted_ms = Some(now_ms);
                true
            }
            Some(_) => false,
        }
    }
}

/// 进度事件的发射函数；单测用闭包捕获推送的块。
type EmitFn = dyn Fn(&Block) + Send + Sync;
/// 时钟注入点（毫秒）。
type ClockFn = dyn Fn() -> u64 + Send + Sync;

/// 把后台扫描的进度写到 `terminal:block`，同一个 `blockId` 原地更新。
pub(crate) struct EventObserver {
    block_id: String,
    input: String,
    started_at: u64,
    origin_ms: u64,
    emit: Arc<EmitFn>,
    clock: Arc<ClockFn>,
    throttle: Mutex<TimeThrottle>,
}

impl EventObserver {
    fn with_port(
        emit: Arc<EmitFn>,
        clock: Arc<ClockFn>,
        block_id: String,
        input: String,
        started_at: u64,
    ) -> Self {
        let origin_ms = clock();
        Self {
            block_id,
            input,
            started_at,
            origin_ms,
            emit,
            clock,
            throttle: Mutex::new(TimeThrottle::new(PROGRESS_INTERVAL_MS)),
        }
    }

    /// 生产实现：通过 Tauri 事件推块。
    pub(crate) fn for_app(app: AppHandle, block_id: String, input: String) -> Self {
        let started_at = now_millis();
        let emitter: Arc<EmitFn> = Arc::new(move |block| emit_block(&app, block));
        Self::with_port(emitter, Arc::new(now_millis), block_id, input, started_at)
    }

    fn block(&self, status: BlockStatus, outputs: Vec<Output>, footer: Option<String>) -> Block {
        let now = (self.clock)();
        Block {
            id: self.block_id.clone(),
            input: self.input.clone(),
            started_at: self.started_at,
            duration_ms: Some(now.saturating_sub(self.origin_ms)),
            status,
            outputs,
            footer,
            schema_version: SCHEMA_VERSION,
            actions: None,
        }
    }
}

impl LibraryObserver for EventObserver {
    fn on_progress(&self, scanned: usize, total: usize, current: &Path) {
        let now = (self.clock)().saturating_sub(self.origin_ms);
        {
            let mut throttle = self
                .throttle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !throttle.allow(now) {
                return;
            }
        }

        let file = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty());
        let block = Block {
            id: self.block_id.clone(),
            input: self.input.clone(),
            started_at: self.started_at,
            duration_ms: None,
            status: BlockStatus::Running,
            outputs: vec![
                text_of(vec![bold("扫描中"), plain(format!(" · {scanned}/{total}"))]),
                Output::Progress {
                    label: "扫描媒体库".to_owned(),
                    current: u64::try_from(scanned).unwrap_or(u64::MAX),
                    total: u64::try_from(total).unwrap_or(u64::MAX),
                    unit: "首".to_owned(),
                },
            ],
            footer: file.map(|name| format!("当前：{name}")),
            schema_version: SCHEMA_VERSION,
            actions: None,
        };
        (self.emit)(&block);
    }

    fn on_finished(&self, report: &ScanReport) {
        let status = if report.cancelled {
            BlockStatus::Cancelled
        } else {
            BlockStatus::Done
        };
        let headline = if report.cancelled {
            "扫描已取消"
        } else {
            "扫描完成"
        };
        let outputs = vec![
            text_of(vec![
                bold(headline),
                plain(format!(" · 扫描 {}", report.scanned)),
            ]),
            text_of(vec![
                plain(format!("入库 {}", report.indexed)),
                dim(format!(" · 跳过 {}", report.skipped)),
                dim(format!(" · 失败 {}", report.failed)),
                dim(format!(" · 缺失 {}", report.missing)),
            ]),
        ];
        let footer = if report.cancelled {
            "已入库部分保留；再次 library scan 可继续增量".to_owned()
        } else {
            "输入 library list 查看最近入库".to_owned()
        };
        (self.emit)(&self.block(status, outputs, Some(footer)));
    }

    fn on_failed(&self, error: &CmdError) {
        (self.emit)(&self.block(BlockStatus::Error, vec![error.to_output()], None));
    }
}

/// 后台扫描线程与前台之间的桥：把 `np-library` 的观察者接口转成
/// [`LibraryObserver`]，并向上暴露取消标志。
struct ScanBridge {
    sink: Arc<dyn LibraryObserver>,
    cancel: Arc<AtomicBool>,
}

impl ScanObserver for ScanBridge {
    fn on_progress(&self, scanned: usize, total: usize, current_path: &Path) {
        self.sink.on_progress(scanned, total, current_path);
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// 扫描线程退出时（含 panic）把 `active` 复位，避免永久 `E_BUSY`。
struct ActiveGuard(Arc<AtomicBool>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 正在跑的扫描。
struct ScanJob {
    cancel: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

struct LibraryInner {
    db_path: PathBuf,
    db: Mutex<Option<Database>>,
    scan: Mutex<Option<ScanJob>>,
    hits: Mutex<Vec<SearchHit>>,
    active_listing: Mutex<Option<ActiveListing>>,
    library_listing: Mutex<Vec<Track>>,
}

/// 生产版 [`LibraryApi`]：持有库路径、主线程连接、扫描任务与会话搜索结果。
///
/// `online` 是进程内唯一的在线音源服务；`LibraryHandle` 是 setup 里已经
/// managed 的状态，把它挂在这里可以避免新增 Tauri state 注册点。
pub(crate) struct LibraryHandle {
    inner: Arc<LibraryInner>,
    online: Arc<OnlineService>,
    settings: Arc<SettingsHandle>,
}

impl LibraryHandle {
    #[cfg(test)]
    pub(crate) fn new(db_path: PathBuf) -> Self {
        let cache_root = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_cache_root(db_path, cache_root)
    }

    /// 显式注入缓存根（生产用 `app_cache_dir()`，测试用临时目录）。
    #[cfg(test)]
    pub(crate) fn with_cache_root(db_path: PathBuf, cache_root: PathBuf) -> Self {
        let config_dir = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_config_dir(db_path, cache_root, config_dir)
    }

    /// 生产入口：缓存根 + 设置目录都由 setup 注入。
    pub(crate) fn with_config_dir(
        db_path: PathBuf,
        cache_root: PathBuf,
        config_dir: PathBuf,
    ) -> Self {
        let covers_dir = db_path
            .parent()
            .map(|parent| parent.join("covers"))
            .unwrap_or_else(|| PathBuf::from("covers"));
        let settings = Arc::new(SettingsHandle::load(config_dir));
        let enabled = settings.enabled_source_ids(&KNOWN_SOURCES);
        Self {
            inner: Arc::new(LibraryInner {
                db_path,
                db: Mutex::new(None),
                scan: Mutex::new(None),
                hits: Mutex::new(Vec::new()),
                active_listing: Mutex::new(None),
                library_listing: Mutex::new(Vec::new()),
            }),
            online: Arc::new(OnlineService::new(cache_root, covers_dir, enabled)),
            settings,
        }
    }

    /// 命令层持有的设置 store。
    pub(crate) fn settings(&self) -> &SettingsHandle {
        &self.settings
    }

    /// 命令层持有的在线音源服务（与媒体库同生命周期）。
    pub(crate) fn online(&self) -> Arc<dyn OnlineApi> {
        Arc::clone(&self.online) as Arc<dyn OnlineApi>
    }

    /// 退出时取消进行中的下载，并打断后台扫描、等它提交已入库部分。
    pub(crate) fn shutdown(&self) {
        self.online.cancel_all();
        if let Ok(mut slot) = self.inner.scan.lock()
            && let Some(job) = slot.take()
        {
            job.cancel.store(true, Ordering::SeqCst);
            let _ = job.join.join();
        }
    }

    /// 封面缓存目录，与 [`Database::open`] 的默认约定一致：
    /// `<db 所在目录>/covers`（ARCH-003 §3）。不打开数据库也能算。
    pub(crate) fn covers_dir(&self) -> PathBuf {
        self.inner
            .db_path
            .parent()
            .map(|parent| parent.join("covers"))
            .unwrap_or_else(|| PathBuf::from("covers"))
    }

    /// 按绝对路径精确查一首曲目；本地库未命中时回退到在线会话映射。
    pub(crate) fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
        let local = self.with_db(|db| db.track_by_path(path))?;
        Ok(lookup_track(local, &self.online, Path::new(path)))
    }

    /// 懒打开（必要时建库）主线程连接；单测不经过这里。
    fn with_db<T>(
        &self,
        action: impl FnOnce(&Database) -> np_library::Result<T>,
    ) -> Result<T, CmdError> {
        let mut guard = self
            .inner
            .db
            .lock()
            .map_err(|_| CmdError::new("E_LIB_DB", "媒体库连接锁已损坏"))?;
        if guard.is_none() {
            *guard = Some(Database::open(&self.inner.db_path).map_err(library_error)?);
        }
        let db = guard
            .as_ref()
            .ok_or_else(|| CmdError::new("E_LIB_DB", "媒体库连接未初始化"))?;
        action(db).map_err(library_error)
    }
}

impl LibraryApi for LibraryHandle {
    fn add_root(&self, path: &str) -> Result<usize, CmdError> {
        let path = path.trim();
        if path.is_empty() {
            return Err(CmdError::new("E_CMD_ARG", "扫描根路径不能为空"));
        }
        let root = std::path::absolute(path)
            .map_err(|error| CmdError::new("E_LIB_IO", format!("无效路径：{error}")))?;
        if !root.is_dir() {
            return Err(CmdError::new(
                "E_CMD_ARG",
                format!("目录不存在：{}", root.display()),
            ));
        }
        if !self.with_db(|db| db.add_root(&root))? {
            return Err(CmdError::new(
                "E_CMD_ARG",
                format!("扫描根已存在：{}", root.display()),
            ));
        }
        let roots = self.with_db(|db| db.list_roots())?;
        Ok(roots.len())
    }

    fn list_roots(&self) -> Result<Vec<String>, CmdError> {
        self.with_db(|db| db.list_roots())
    }

    fn scan(&self, full: bool, observer: Arc<dyn LibraryObserver>) -> Result<(), CmdError> {
        let mut slot = self
            .inner
            .scan
            .lock()
            .map_err(|_| CmdError::new("E_LIB_SCAN", "扫描状态锁已损坏"))?;
        if slot
            .as_ref()
            .is_some_and(|job| job.active.load(Ordering::SeqCst))
        {
            return Err(CmdError::new("E_BUSY", "已有扫描在进行中"));
        }
        if let Some(finished) = slot.take() {
            // 上一轮已经结束（active 为 false），join 只回收线程。
            let _ = finished.join.join();
        }

        let roots = self.list_roots()?;
        if roots.is_empty() {
            return Err(CmdError::new("E_CMD_ARG", "还没有扫描根")
                .with_hint("先 library add <path> 添加目录"));
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));
        let bridge = Arc::new(ScanBridge {
            sink: observer,
            cancel: Arc::clone(&cancel),
        });
        let db_path = self.inner.db_path.clone();
        let thread_active = Arc::clone(&active);
        let join = std::thread::Builder::new()
            .name("np-library-scan".to_owned())
            .spawn(move || {
                let _guard = ActiveGuard(thread_active);
                run_scan(&db_path, full, bridge);
            })
            .map_err(|error| CmdError::new("E_LIB_SCAN", format!("无法启动扫描线程：{error}")))?;

        *slot = Some(ScanJob {
            cancel,
            active,
            join,
        });
        Ok(())
    }

    fn search(&self, keyword: &str, limit: usize) -> Result<Vec<Track>, CmdError> {
        self.with_db(|db| db.search(keyword, limit))
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<Track>, CmdError> {
        self.with_db(|db| db.list_recent(limit))
    }

    fn stats(&self) -> Result<Stats, CmdError> {
        self.with_db(|db| db.stats())
    }

    fn covers_usage(&self) -> Result<CoverCacheUsage, CmdError> {
        // 只算 `<db 所在目录>/covers` 下的 jpg；目录不存在返回 0。
        Ok(cover_cache_usage(&self.covers_dir()))
    }

    fn remember_search(&self, tracks: Vec<Track>) {
        let mut hits: Vec<SearchHit> = tracks.into_iter().map(SearchHit::Local).collect();
        hits.truncate(MAX_SEARCH_RESULTS);
        if let Ok(mut slot) = self.inner.hits.lock() {
            *slot = hits;
        }
    }

    fn search_results(&self) -> Vec<Track> {
        self.inner
            .hits
            .lock()
            .map(|hits| {
                hits.iter()
                    .filter_map(|hit| match hit {
                        SearchHit::Local(track) => Some(track.clone()),
                        SearchHit::Online(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remember_hits(&self, hits: Vec<SearchHit>) {
        let mut hits = hits;
        hits.truncate(MAX_SEARCH_RESULTS);
        if let Ok(mut slot) = self.inner.hits.lock() {
            *slot = hits;
        }
    }

    fn search_hits(&self) -> Vec<SearchHit> {
        self.inner
            .hits
            .lock()
            .map(|hits| hits.clone())
            .unwrap_or_default()
    }

    fn set_active_listing(&self, listing: ActiveListing) {
        if let Ok(mut slot) = self.inner.active_listing.lock() {
            *slot = Some(listing);
        }
    }

    fn active_listing(&self) -> Option<ActiveListing> {
        self.inner
            .active_listing
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    fn remember_library_listing(&self, tracks: Vec<Track>) {
        if let Ok(mut slot) = self.inner.library_listing.lock() {
            *slot = tracks;
        }
    }

    fn library_listing(&self) -> Vec<Track> {
        self.inner
            .library_listing
            .lock()
            .map(|tracks| tracks.clone())
            .unwrap_or_default()
    }

    fn playlist_create(&self, name: &str) -> Result<i64, CmdError> {
        self.with_db(|db| db.create_playlist(name))
    }

    fn playlist_rename(&self, id: i64, name: &str) -> Result<(), CmdError> {
        self.with_db(|db| db.rename_playlist(id, name))
    }

    fn playlist_delete(&self, id: i64) -> Result<(), CmdError> {
        self.with_db(|db| db.delete_playlist(id))
    }

    fn playlist_list(&self) -> Result<Vec<PlaylistSummary>, CmdError> {
        self.with_db(Database::list_playlists)
    }

    fn playlist_by_name(&self, name: &str) -> Result<Option<Playlist>, CmdError> {
        self.with_db(|db| db.playlist_by_name(name))
    }

    fn playlist_get(&self, id: i64) -> Result<(Playlist, Vec<PlaylistItem>), CmdError> {
        self.with_db(|db| db.get_playlist(id))
    }

    fn playlist_add(&self, id: i64, item: &NewPlaylistItem) -> Result<PlaylistItem, CmdError> {
        self.with_db(|db| db.add_playlist_item(id, item))
    }

    fn playlist_remove(&self, id: i64, position: i64) -> Result<(), CmdError> {
        self.with_db(|db| db.remove_playlist_item(id, position))
    }

    fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
        LibraryHandle::track_by_path(self, path)
    }

    fn tracks_by_paths(&self, paths: &[String]) -> Result<HashMap<String, Track>, CmdError> {
        // 一次 open，循环查库；未命中的再查在线会话映射。
        let mut tracks = self.with_db(|db| {
            let mut found = HashMap::new();
            for path in paths {
                if let Some(track) = db.track_by_path(path)? {
                    found.insert(path.clone(), track);
                }
            }
            Ok(found)
        })?;
        for path in paths {
            if tracks.contains_key(path) {
                continue;
            }
            if let Some(track) = self.online.track_by_path(Path::new(path)) {
                tracks.insert(path.clone(), track);
            }
        }
        Ok(tracks)
    }
}

/// 后台线程：打开独立的库连接、读根、跑扫描，把结果交给观察者。
fn run_scan(db_path: &Path, full: bool, observer: Arc<ScanBridge>) {
    let db = match Database::open(db_path) {
        Ok(db) => db,
        Err(error) => {
            observer.sink.on_failed(&library_error(error));
            return;
        }
    };
    let roots = match db.list_roots() {
        Ok(roots) => roots,
        Err(error) => {
            observer.sink.on_failed(&library_error(error));
            return;
        }
    };
    if roots.is_empty() {
        observer
            .sink
            .on_failed(&CmdError::new("E_CMD_ARG", "还没有扫描根"));
        return;
    }

    let scanner = Scanner::new(&db, observer.as_ref());
    let result = if full {
        scanner.full().scan(&roots)
    } else {
        scanner.scan(&roots)
    };
    match result {
        Ok(report) => observer.sink.on_finished(&report),
        Err(error) => observer.sink.on_failed(&library_error(error)),
    }
}

/// `cmd.exec` / `library.scan` 的“已受理”回执。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScanAcceptedReply {
    pub(crate) accepted: bool,
}

/// IPC 错误载荷（`{ code, message, hint? }`），与终端 `error` 原语同形。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CmdErrorPayload {
    pub(crate) code: String,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hint: Option<String>,
}

impl From<CmdError> for CmdErrorPayload {
    fn from(error: CmdError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            hint: error.hint,
        }
    }
}

type DedupKey = (SourceId, String);
type DedupLock = Arc<Mutex<()>>;

/// 把远端封面字节编码并落盘；生产实现只调用 `np_library::store_cover`。
trait CoverStore: Send + Sync {
    fn store(&self, dir: &Path, bytes: &[u8]) -> Option<String>;
}

struct NpCoverStore;

impl CoverStore for NpCoverStore {
    fn store(&self, dir: &Path, bytes: &[u8]) -> Option<String> {
        np_library::store_cover(dir, bytes)
    }
}

struct OnlineRuntime {
    registry: SourceRegistry,
    cache: StreamCache,
    transport: Arc<dyn Transport>,
}

/// 生产版 [`OnlineApi`]：持有注册表、下载缓存与进程内去重锁。
///
/// 传输层在首次使用时惰性创建：`LibraryHandle::new` 是 infallible 的，
/// 客户端构造失败会变成一次可读的 `E_SRC_UNAVAILABLE`，不影响本地命令。
pub(crate) struct OnlineService {
    cache_root: PathBuf,
    covers_dir: PathBuf,
    cover_store: Arc<dyn CoverStore>,
    runtime: OnceLock<Result<OnlineRuntime, String>>,
    /// 单测注入的现成 runtime；生产恒为 `None`。
    fixed: Option<OnlineRuntime>,
    locks: Mutex<HashMap<DedupKey, DedupLock>>,
    /// 会话级远程元数据：缓存路径 → 源信息。进程退出即丢。
    remote_meta: Mutex<HashMap<PathBuf, RemoteMeta>>,
    /// 仅用于测试断言“本地命中时没有查映射”。
    track_lookups: AtomicUsize,
    /// 退出时置位；下载器每个 chunk 检查。
    cancel: Arc<AtomicBool>,
    /// 已知源 id（含被禁用的），`source list` 用。
    all_ids: Vec<SourceId>,
    /// 当前启用的源；搜索与 prepare 立即按它过滤。
    enabled: Mutex<HashSet<SourceId>>,
}

impl OnlineService {
    pub(crate) fn new(
        cache_root: PathBuf,
        covers_dir: PathBuf,
        enabled: HashSet<SourceId>,
    ) -> Self {
        Self {
            cache_root,
            covers_dir,
            cover_store: Arc::new(NpCoverStore),
            runtime: OnceLock::new(),
            fixed: None,
            locks: Mutex::new(HashMap::new()),
            remote_meta: Mutex::new(HashMap::new()),
            track_lookups: AtomicUsize::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
            all_ids: KNOWN_SOURCES.to_vec(),
            enabled: Mutex::new(enabled),
        }
    }

    #[cfg(test)]
    fn from_parts(
        registry: SourceRegistry,
        cache: StreamCache,
        transport: Arc<dyn Transport>,
        covers_dir: PathBuf,
        cover_store: Arc<dyn CoverStore>,
    ) -> Self {
        let cache_root = cache
            .dir()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Self {
            cache_root,
            covers_dir,
            cover_store,
            runtime: OnceLock::new(),
            fixed: Some(OnlineRuntime {
                registry: SourceRegistry::new(registry.sources().to_vec()),
                cache,
                transport,
            }),
            locks: Mutex::new(HashMap::new()),
            remote_meta: Mutex::new(HashMap::new()),
            track_lookups: AtomicUsize::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
            all_ids: registry.ids(),
            enabled: Mutex::new(registry.ids().into_iter().collect()),
        }
    }

    fn runtime(&self) -> Result<&OnlineRuntime, CmdError> {
        if let Some(fixed) = self.fixed.as_ref() {
            return Ok(fixed);
        }
        self.runtime
            .get_or_init(|| self.build())
            .as_ref()
            .map_err(|message| CmdError::new("E_SRC_UNAVAILABLE", message.clone()))
    }

    fn build(&self) -> Result<OnlineRuntime, String> {
        let transport: Arc<dyn Transport> = Arc::new(
            ReqwestTransport::new().map_err(|error| format!("无法创建网络客户端：{error}"))?,
        );
        let registry = SourceRegistry::new(vec![
            Arc::new(BilibiliSource::new(Arc::clone(&transport))) as Arc<dyn AudioSource>,
            Arc::new(NeteaseSource::new(Arc::clone(&transport))) as Arc<dyn AudioSource>,
        ]);
        let downloader: Arc<dyn Downloader> = Arc::new(HttpDownloader::new(Arc::clone(&transport)));
        let cache = StreamCache::new(self.cache_root.clone(), downloader);
        Ok(OnlineRuntime {
            registry,
            cache,
            transport,
        })
    }

    fn lock_for(&self, key: &DedupKey) -> DedupLock {
        self.locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 下载/命中缓存后登记远程元数据；会话级，不持久化。
    fn remember_remote(&self, path: &Path, track: &SourceTrack, cover_key: Option<String>) {
        let meta = RemoteMeta {
            source: track.source.as_str().to_owned(),
            source_id: track.source_id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            duration_ms: track.duration_ms,
            cover_key,
        };
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path.to_path_buf(), meta);
    }

    /// 取封面字节并交给 `np-library` 编码落盘；任何一步失败都只丢图。
    fn fetch_cover(&self, runtime: &OnlineRuntime, track: &SourceTrack) -> Option<String> {
        if self.cancel.load(Ordering::SeqCst) {
            return None;
        }
        let url = track.cover_url.as_deref()?;
        let headers: &[(&str, &str)] = if track.source == SourceId::Bilibili {
            &[("Referer", "https://www.bilibili.com")]
        } else {
            &[]
        };
        let mut bytes = Vec::new();
        runtime.transport.get_to(url, headers, &mut bytes).ok()?;
        if bytes.is_empty() {
            return None;
        }
        self.cover_store.store(&self.covers_dir, &bytes)
    }

    /// 缓存清理/文件消失后同步移除映射，避免“元数据有、文件没了”。
    fn prune_remote_meta(&self) {
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|path, _| {
                fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
            });
    }

    /// 按缓存路径查远程元数据；文件不存在时顺手清掉映射。
    pub(crate) fn track_by_path(&self, path: &Path) -> Option<Track> {
        self.track_lookups.fetch_add(1, Ordering::SeqCst);
        let meta = self
            .remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(path)
            .cloned()?;
        let Ok(metadata) = fs::metadata(path) else {
            self.remove_remote(path);
            return None;
        };
        if !metadata.is_file() || metadata.len() == 0 {
            self.remove_remote(path);
            return None;
        }
        Some(remote_track(path, &meta, &metadata))
    }

    fn remove_remote(&self, path: &Path) {
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(path);
    }

    /// 准备成功后登记元数据并清理已消失的缓存映射。
    fn finish_prepared(
        &self,
        runtime: &OnlineRuntime,
        track: &SourceTrack,
        path: PathBuf,
        bytes: u64,
        cached: bool,
    ) -> PreparedTrack {
        // 已登记的 cover_key 直接复用；否则在这一步取图（失败只丢图）。
        let existing_cover = {
            self.remote_meta
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&path)
                .and_then(|meta| meta.cover_key.clone())
        };
        let cover_key = existing_cover.or_else(|| self.fetch_cover(runtime, track));
        self.remember_remote(&path, track, cover_key);
        self.prune_remote_meta();
        PreparedTrack {
            path,
            bytes,
            cached,
        }
    }

    /// 已知源 id（含被禁用的）。
    pub(crate) fn known_sources(&self) -> Vec<SourceId> {
        self.all_ids.clone()
    }

    /// 进程内启停一个源；不影响已在进行的下载。
    pub(crate) fn set_source_enabled(
        &self,
        source: SourceId,
        enabled: bool,
    ) -> Result<(), CmdError> {
        if !self.all_ids.contains(&source) {
            return Err(CmdError::new(
                "E_SRC_UNAVAILABLE",
                format!("未知音源：{source}"),
            ));
        }
        let mut current = self
            .enabled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if enabled {
            current.insert(source);
        } else {
            current.remove(&source);
        }
        Ok(())
    }

    pub(crate) fn cache_usage(&self) -> Result<CacheUsage, CmdError> {
        let (streams_files, streams_bytes) = dir_usage(&self.cache_root.join("streams"));
        let (covers_files, covers_bytes) = dir_usage(&self.covers_dir);
        Ok(CacheUsage {
            streams_files,
            streams_bytes,
            covers_files,
            covers_bytes,
        })
    }

    pub(crate) fn clear_cache(&self, target: CacheTarget) -> Result<u64, CmdError> {
        let mut released = 0u64;
        if target.includes_streams() {
            released = released.saturating_add(clear_dir_files(&self.cache_root.join("streams")));
        }
        if target.includes_covers() {
            released = released.saturating_add(clear_dir_files(&self.covers_dir));
        }
        Ok(released)
    }

    #[cfg(test)]
    fn remote_meta_len(&self) -> usize {
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// 退出时置位；正在写的下载会在下一个 chunk 失败并清理 `.part`。
    pub(crate) fn cancel_all(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    #[cfg(test)]
    fn track_lookups(&self) -> usize {
        self.track_lookups.load(Ordering::SeqCst)
    }

    /// 搜启用源；`source` 限定单个源（T-0046 D）。
    fn search_enabled(
        &self,
        query: &str,
        limit: usize,
        source: Option<SourceId>,
    ) -> Result<OnlineSearch, CmdError> {
        let runtime = self.runtime()?;
        let enabled = self
            .enabled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let sources: Vec<_> = runtime
            .registry
            .sources()
            .iter()
            .filter(|candidate| {
                enabled.contains(&candidate.id())
                    && source.is_none_or(|wanted| candidate.id() == wanted)
            })
            .cloned()
            .collect();
        let registry = SourceRegistry::new(sources);
        let attempted = registry.sources().len();
        let merged = registry.search_all(query, limit);
        let all_failed = attempted > 0 && merged.failures.len() == attempted;
        Ok(OnlineSearch {
            tracks: merged.tracks,
            all_failed,
        })
    }
}

impl OnlineApi for OnlineService {
    fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError> {
        self.search_enabled(query, limit, None)
    }

    fn search_all_for(
        &self,
        query: &str,
        limit: usize,
        source: Option<SourceId>,
    ) -> Result<OnlineSearch, CmdError> {
        self.search_enabled(query, limit, source)
    }

    fn resolve_up(&self, name: &str) -> Result<UpResolution, CmdError> {
        let runtime = self.runtime()?;
        let source = runtime
            .registry
            .source(SourceId::Bilibili)
            .ok_or_else(|| CmdError::new("E_SRC_UNAVAILABLE", "B 站音源未注册"))?;
        let users = source.search_up_users(name).map_err(source_error_to_cmd)?;
        let resolved = resolve_up_user(&users, name).ok_or_else(|| {
            CmdError::new("E_SRC_UNAVAILABLE", format!("没有找到 UP：{}", name.trim()))
        })?;
        Ok(UpResolution {
            mid: resolved.user.mid,
            name: Some(resolved.user.name),
            fans: resolved.user.fans,
            same_name: resolved.same_name,
        })
    }

    fn up_archives(
        &self,
        mid: u64,
        name: &str,
        keyword: &str,
        limit: usize,
    ) -> Result<Vec<SourceTrack>, CmdError> {
        if !self
            .enabled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&SourceId::Bilibili)
        {
            return Err(CmdError::new(
                "E_SRC_UNAVAILABLE",
                "B 站音源已禁用（source enable bilibili）",
            ));
        }
        let runtime = self.runtime()?;
        let source = runtime
            .registry
            .source(SourceId::Bilibili)
            .ok_or_else(|| CmdError::new("E_SRC_UNAVAILABLE", "B 站音源未注册"))?;
        source
            .user_archives(mid, name, keyword, limit)
            .map_err(source_error_to_cmd)
    }

    fn prepare(
        &self,
        track: &SourceTrack,
        progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        if !self
            .enabled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&track.source)
        {
            return Err(CmdError::new(
                "E_SRC_UNAVAILABLE",
                format!(
                    "该音源已被禁用：{}（source enable {} 可重新启用）",
                    track.source, track.source
                ),
            ));
        }
        let runtime = self.runtime()?;
        let key = (track.source, track.source_id.clone());
        let lock = self.lock_for(&key);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        // 命中缓存时连 resolve 都不做（网易云 URL 只有 20 分钟有效期）。
        if let Some(path) = runtime.cache.lookup(track.source, &track.source_id) {
            let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            return Ok(self.finish_prepared(runtime, track, path, bytes, true));
        }

        let source = runtime.registry.source(track.source).ok_or_else(|| {
            CmdError::new("E_SRC_UNAVAILABLE", format!("音源未注册：{}", track.source))
        })?;

        for attempt in 0..2 {
            let outcome = match source.resolve(track) {
                Ok(stream) => runtime.cache.fetch_with_progress(
                    track.source,
                    &track.source_id,
                    &stream,
                    progress,
                ),
                Err(error) => Err(error),
            };

            match outcome {
                Ok(path) => {
                    let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                    return Ok(self.finish_prepared(runtime, track, path, bytes, false));
                }
                Err(error) if is_expired_url_error(&error) && attempt == 0 => continue,
                Err(error) if is_expired_url_error(&error) => return Err(expired_url_error()),
                Err(error) => return Err(source_error_to_cmd(error)),
            }
        }

        Err(expired_url_error())
    }

    fn known_sources(&self) -> Vec<SourceId> {
        OnlineService::known_sources(self)
    }

    fn set_source_enabled(&self, source: SourceId, enabled: bool) -> Result<(), CmdError> {
        OnlineService::set_source_enabled(self, source, enabled)
    }

    fn cache_usage(&self) -> Result<CacheUsage, CmdError> {
        OnlineService::cache_usage(self)
    }

    fn clear_cache(&self, target: CacheTarget) -> Result<u64, CmdError> {
        OnlineService::clear_cache(self, target)
    }
}

/// 统计目录下普通文件的个数与字节数（不递归、不跟随链接）。
fn dir_usage(dir: &Path) -> (u64, u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if let Ok(metadata) = entry.metadata() {
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    (files, bytes)
}

/// 删除目录下所有普通文件（不删目录），返回释放字节数。
fn clear_dir_files(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut released = 0u64;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let bytes = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        if fs::remove_file(&path).is_ok() {
            released = released.saturating_add(bytes);
        }
    }
    released
}

/// 本地命中优先；未命中才查在线会话映射。
fn lookup_track(local: Option<Track>, online: &OnlineService, path: &Path) -> Option<Track> {
    local.or_else(|| online.track_by_path(path))
}

/// 把会话元数据包成前端已经在消费的 `Track` 形状（`kind: "remote"`）。
fn remote_track(path: &Path, meta: &RemoteMeta, metadata: &fs::Metadata) -> Track {
    Track {
        id: 0,
        kind: "remote".to_owned(),
        path: path.to_string_lossy().into_owned(),
        source: meta.source.clone(),
        source_id: meta.source_id.clone(),
        title: meta.title.clone(),
        album_id: None,
        album: String::new(),
        artist: meta.artist.clone(),
        album_artist: String::new(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration_ms: i64::try_from(meta.duration_ms).unwrap_or(i64::MAX),
        sample_rate: 0,
        channels: 0,
        bitrate: 0,
        codec: String::new(),
        cover_key: meta.cover_key.clone(),
        size_bytes: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
        mtime_ms: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0),
        added_at: 0,
        last_seen_at: 0,
    }
}

impl PlaybackTarget for PlayerHandle {
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError> {
        self.play(Some(path.to_owned()))
            .map_err(CmdError::from_player)
    }
}

/// 离线 URL 过期（B 站 403 / 410）；部分错误被 transport 折成 `Unavailable`。
fn is_expired_url_error(error: &SourceError) -> bool {
    match error {
        SourceError::Http(status) => matches!(status, 403 | 410),
        SourceError::Unavailable(message) => {
            message.contains("403")
                || message.contains("410")
                || message.contains("过期")
                || message.contains("expired")
        }
        _ => false,
    }
}

fn expired_url_error() -> CmdError {
    CmdError::new("E_SRC_UNAVAILABLE", "链接已过期，请重新搜索")
}

/// 源错误 → 用户可读的命令错误。
fn source_error_to_cmd(error: SourceError) -> CmdError {
    match error {
        SourceError::Download(_) => CmdError::new("E_SRC_DOWNLOAD", "下载失败（已清理临时文件）"),
        SourceError::Network(_) => CmdError::new("E_NET_TIMEOUT", "网络超时，请检查网络或稍后重试"),
        SourceError::Unavailable(message) => CmdError::new("E_SRC_UNAVAILABLE", message),
        SourceError::Http(status) => {
            CmdError::new("E_SRC_UNAVAILABLE", format!("音源返回 HTTP {status}"))
        }
        SourceError::Parse(message) => {
            CmdError::new("E_SRC_UNAVAILABLE", format!("音源响应异常：{message}"))
        }
        SourceError::NotImplemented(message) => CmdError::new("E_SRC_UNAVAILABLE", message),
    }
}

/// 合并搜索结果的终端输出：序号 1 基、跨源连续，来源列写明本地 / B站 / 网易云。
#[cfg(test)]
fn build_search_outcome(keyword: &str, hits: &[SearchHit], all_online_failed: bool) -> Outcome {
    let options = SearchOptions::legacy(keyword, hits.len().max(1));
    build_search_outcome_for(&options, hits, all_online_failed, hits.len(), 0)
}

fn build_search_outcome_for(
    options: &SearchOptions,
    hits: &[SearchHit],
    all_online_failed: bool,
    before_artist: usize,
    removed: usize,
) -> Outcome {
    let keyword = &options.keyword;
    if hits.is_empty() {
        let mut outputs = vec![
            text_of(vec![bold("没有找到"), dim(format!(" · {keyword}"))]),
            Output::Table {
                columns: search_columns(),
                rows: Vec::new(),
                selectable: true,
            },
        ];
        if options.artist.is_some() && before_artist > 0 && removed > 0 {
            outputs.push(hint_output(&format!(
                "过滤前有 {before_artist} 条，被 --artist 过滤掉了 {removed} 条"
            )));
        } else {
            outputs.push(hint_output(if all_online_failed {
                "在线源暂时不可用"
            } else {
                "换个关键词试试"
            }));
        }
        return Outcome {
            outputs,
            footer: Some(options.footer(0, true)),
        };
    }

    let rows = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| {
            vec![
                (index + 1).to_string(),
                hit.source_label().to_owned(),
                hit.title().to_owned(),
                hit.artist().to_owned(),
                format_hit_duration(hit.duration_ms()),
            ]
        })
        .collect();

    let mut outputs = vec![
        text_of(vec![bold("搜索结果"), dim(format!(" · {keyword}"))]),
        Output::Table {
            columns: search_columns(),
            rows,
            selectable: true,
        },
    ];
    if all_online_failed {
        outputs.push(hint_output("在线源暂时不可用"));
    }

    Outcome {
        outputs,
        footer: Some(options.footer(hits.len(), true)),
    }
}

fn search_columns() -> Vec<String> {
    ["序号", "来源", "标题", "艺人", "时长"]
        .iter()
        .map(|column| (*column).to_owned())
        .collect()
}

fn hint_output(text: &str) -> Output {
    Output::Hint {
        text: text.to_owned(),
        suggestion: None,
    }
}

fn format_hit_duration(duration_ms: u64) -> String {
    if duration_ms == 0 {
        "--:--".to_owned()
    } else {
        format_position(duration_ms)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn finish_block(
    template: &Block,
    status: BlockStatus,
    outputs: Vec<Output>,
    footer: Option<String>,
    clock: &dyn Fn() -> u64,
) -> Block {
    Block {
        id: template.id.clone(),
        input: template.input.clone(),
        started_at: template.started_at,
        duration_ms: Some(clock().saturating_sub(template.started_at)),
        status,
        outputs,
        footer,
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

/// 后台执行 `search`：本地库 + 在线源，合并后写入会话上下文并推最终块。
#[cfg(test)]
pub(crate) fn run_online_search(
    keyword: &str,
    limit: usize,
    online: &dyn OnlineApi,
    library: &dyn LibraryApi,
    emit: &dyn Fn(&Block),
    template: &Block,
    clock: &dyn Fn() -> u64,
) {
    let options = SearchOptions::legacy(keyword, limit);
    run_online_search_with_options(&options, online, library, emit, template, clock);
}

pub(crate) fn run_online_search_with_options(
    options: &SearchOptions,
    online: &dyn OnlineApi,
    library: &dyn LibraryApi,
    emit: &dyn Fn(&Block),
    template: &Block,
    clock: &dyn Fn() -> u64,
) {
    // `--source <具体源>` 只搜该在线源，不混本地库。
    let mut local = if options.source.is_none() {
        match library.search(&options.keyword, options.local_fetch_limit()) {
            Ok(tracks) => tracks,
            Err(error) => {
                emit(&finish_block(
                    template,
                    BlockStatus::Error,
                    vec![error.to_output()],
                    None,
                    clock,
                ));
                return;
            }
        }
    } else {
        Vec::new()
    };
    let local_before = local.len();
    if let Some(artist) = &options.artist {
        local.retain(|track| !matches!(local_artist_match(track, artist), MatchKind::None));
    }
    let local_removed = local_before.saturating_sub(local.len());
    // 默认视图给本地留 10 条上限，剩余名额给在线；显式 --limit 不压本地。
    local.truncate(options.local_cap());

    let (mut online_tracks, all_failed) = match online.search_all_for(
        &options.source_query(),
        options.candidate_pool(),
        options.source,
    ) {
        Ok(result) => (result.tracks, result.all_failed),
        // 在线客户端初始化失败也不能吞掉本地结果。
        Err(_) => (Vec::new(), true),
    };
    let online_before = online_tracks.len();
    if let Some(artist) = &options.artist {
        let keyword = options.keyword_filter();
        online_tracks.retain(|track| {
            !matches!(
                match_source_artist_in_query(track, artist, keyword),
                MatchKind::None
            )
        });
    }
    let online_removed = online_before.saturating_sub(online_tracks.len());

    let before_artist = local_before.saturating_add(online_before);
    let removed = local_removed.saturating_add(online_removed);
    let mut hits: Vec<SearchHit> = local.into_iter().map(SearchHit::Local).collect();
    hits.extend(online_tracks.into_iter().map(SearchHit::Online));
    if let Some(artist) = &options.artist {
        // Author 排前、Title 排后；同档保持“本地在前 + 各源返回顺序”。
        let keyword = options.keyword_filter();
        let mut ranked: Vec<(MatchKind, SearchHit)> = hits
            .into_iter()
            .map(|hit| (hit.artist_match(artist, keyword), hit))
            .collect();
        ranked.sort_by_key(|(kind, _)| *kind);
        hits = ranked.into_iter().map(|(_, hit)| hit).collect();
    }
    hits.truncate(options.limit);
    library.remember_hits(hits.clone());
    // 即使 0 条，也把上下文切成“空的 Search”，不保留上一张旧表。
    library.set_active_listing(ActiveListing {
        source: ListingSource::Search,
        len: hits.len(),
    });

    let outcome = build_search_outcome_for(options, &hits, all_failed, before_artist, removed);
    emit(&finish_block(
        template,
        BlockStatus::Done,
        outcome.outputs,
        outcome.footer,
        clock,
    ));
}

/// 先于后台线程推给前端的 `running` 块（UP 检索）。
fn up_started_block(block_id: &str, input: &str, target: &str) -> Block {
    Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at: now_millis(),
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: vec![text_of(vec![
            bold("查找 UP"),
            plain(format!(" · {target}")),
        ])],
        footer: Some("正在解析用户名并读取投稿…".to_owned()),
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

/// `--up` / `--mid` 的后台执行：最多两次请求（解析 + 一次空间列表）。
pub(crate) fn run_up_search(
    options: &SearchOptions,
    online: &dyn OnlineApi,
    library: &dyn LibraryApi,
    emit: &dyn Fn(&Block),
    template: &Block,
    clock: &dyn Fn() -> u64,
) {
    let result = (|| -> Result<Outcome, CmdError> {
        let resolution = if let Some(mid) = options.mid {
            UpResolution {
                mid,
                name: None,
                fans: 0,
                same_name: 0,
            }
        } else {
            let name = options
                .up
                .as_deref()
                .ok_or_else(|| CmdError::new("E_CMD_ARG", "缺少 --up 或 --mid"))?;
            online.resolve_up(name)?
        };

        let display_name = resolution
            .name
            .clone()
            .unwrap_or_else(|| format!("UP {}", resolution.mid));
        let keyword = if options.has_keyword {
            options.keyword.as_str()
        } else {
            ""
        };
        let limit = if options.has_keyword || options.explicit_limit {
            options.limit
        } else {
            UP_ARCHIVE_PAGE_SIZE
        };
        let tracks = online.up_archives(resolution.mid, &display_name, keyword, limit)?;
        let hits: Vec<SearchHit> = tracks.into_iter().map(SearchHit::Online).collect();
        library.remember_hits(hits.clone());
        library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: hits.len(),
        });

        let match_line = match &resolution.name {
            Some(name) => format!(
                "已匹配 UP：{name}（mid={}，粉丝 {}）",
                resolution.mid,
                format_fans(resolution.fans)
            ),
            None => format!("已指定 UP：mid={}", resolution.mid),
        };
        let mut outputs = vec![text_of(vec![bold(match_line)])];
        if resolution.same_name > 1 {
            outputs.push(hint_output(&format!(
                "另有 {} 个同名用户；用 --mid <id> 指定",
                resolution.same_name - 1
            )));
        }
        if hits.is_empty() {
            outputs.push(text_of(vec![
                bold("没有找到"),
                dim(if keyword.is_empty() {
                    " · 该 UP 最近没有可播放投稿"
                } else {
                    " · 该 UP 的投稿里没有匹配关键词"
                }),
            ]));
            return Ok(Outcome {
                outputs,
                footer: None,
            });
        }

        let rows = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                vec![
                    (index + 1).to_string(),
                    hit.source_label().to_owned(),
                    hit.title().to_owned(),
                    hit.artist().to_owned(),
                    format_hit_duration(hit.duration_ms()),
                ]
            })
            .collect();
        outputs.push(Output::Table {
            columns: search_columns(),
            rows,
            selectable: true,
        });
        let footer = if keyword.is_empty() {
            format!(
                "最近 {} 条投稿 · 只显示最近 30 条，可用关键词过滤",
                hits.len()
            )
        } else {
            format!("共 {} 条 · 输入 play <序号> 播放", hits.len())
        };
        Ok(Outcome {
            outputs,
            footer: Some(footer),
        })
    })();

    match result {
        Ok(outcome) => emit(&finish_block(
            template,
            BlockStatus::Done,
            outcome.outputs,
            outcome.footer,
            clock,
        )),
        Err(error) => emit(&finish_block(
            template,
            BlockStatus::Error,
            vec![error.to_output()],
            None,
            clock,
        )),
    }
}

fn format_fans(fans: u64) -> String {
    if fans >= 10_000 {
        format!("{:.1} 万", fans as f64 / 10_000.0)
    } else {
        fans.to_string()
    }
}

/// 后台执行 `play <在线序号>`：resolve → 下载（进度按时间节流）→ 播放本地路径。
pub(crate) fn run_online_play(
    track: &SourceTrack,
    index: usize,
    online: &dyn OnlineApi,
    playback: &dyn PlaybackTarget,
    emit: &dyn Fn(&Block),
    template: &Block,
    clock: &dyn Fn() -> u64,
) {
    let mut throttle = TimeThrottle::new(PROGRESS_INTERVAL_MS);
    let mut progress = |written: u64| {
        if !throttle.allow(clock()) {
            return;
        }
        let mut block = template.clone();
        block.status = BlockStatus::Running;
        block.duration_ms = None;
        block.outputs = vec![
            text_of(vec![
                bold("下载中"),
                dim(format!(" · 搜索结果第 {} 首", index + 1)),
                dim(format!(" · {}", track.title)),
            ]),
            Output::Progress {
                label: format!("下载 {}", track.title),
                current: written,
                total: 0,
                unit: "bytes".to_owned(),
            },
        ];
        block.footer = Some(format!(
            "来源：{} · 已下载 {}",
            source_label(track.source),
            format_bytes(written)
        ));
        emit(&block);
    };

    let prepared = match online.prepare(track, &mut progress) {
        Ok(prepared) => prepared,
        Err(error) => {
            emit(&finish_block(
                template,
                BlockStatus::Error,
                vec![error.to_output()],
                None,
                clock,
            ));
            return;
        }
    };

    if let Err(error) = playback.play_path(&prepared.path.to_string_lossy()) {
        emit(&finish_block(
            template,
            BlockStatus::Error,
            vec![error.to_output()],
            None,
            clock,
        ));
        return;
    }

    let cache_state = if prepared.cached {
        "命中缓存".to_owned()
    } else {
        format!("已缓存 {}", format_bytes(prepared.bytes))
    };
    let mut spans = vec![
        bold("播放"),
        plain(format!(" · 搜索结果第 {} 首", index + 1)),
        dim(format!(" · {}", track.title)),
    ];
    if !track.artist.is_empty() {
        spans.push(dim(format!(" · {}", track.artist)));
    }

    emit(&finish_block(
        template,
        BlockStatus::Done,
        vec![text_of(spans)],
        Some(format!(
            "来源：{} · {cache_state}",
            source_label(track.source)
        )),
        clock,
    ));
}

/// 先于后台线程推给前端的 `running` 块（避免进度事件抢跑）。
pub(crate) fn scan_started_block(block_id: &str, input: &str, full: bool) -> Block {
    let (outputs, footer) = scan_started_output(full);
    Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at: now_millis(),
        duration_ms: None,
        status: BlockStatus::Running,
        outputs,
        footer,
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

/// `cmd.exec` 的立即回执：`{ blockId, ok }`（camelCase）。
///
/// 字段名经 `serde` 的 `rename_all = "camelCase"` 序列化，与 T-0018 契约一致；
/// Tauri 对 `T: Serialize` 的返回值使用 [`tauri::ipc::IpcResponse`] 的默认实现，
/// 不再手写 JSON（T-0021 §2.1）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CmdExecReply {
    block_id: String,
    ok: bool,
}

/// `PlayerHandle` 的 [`PlayerApi`] 适配（生产实现）。
pub(crate) struct HandleApi<'a> {
    handle: &'a PlayerHandle,
}

impl<'a> HandleApi<'a> {
    pub(crate) fn new(handle: &'a PlayerHandle) -> Self {
        Self { handle }
    }
}

impl PlayerApi for HandleApi<'_> {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        self.handle.snapshot().map_err(CmdError::from_player)
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        self.handle.play(path).map_err(CmdError::from_player)
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        self.handle.pause().map_err(CmdError::from_player)
    }

    /// `PlayerHandle` 没有独立的 `resume`：`play(None)` 在 actor 里就是继续当前曲目。
    fn resume(&self) -> Result<PlaybackState, CmdError> {
        self.handle.play(None).map_err(CmdError::from_player)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.handle.next().map_err(CmdError::from_player)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.handle.prev().map_err(CmdError::from_player)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        self.handle.seek(position_ms).map_err(CmdError::from_player)
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        self.handle.volume(volume).map_err(CmdError::from_player)
    }

    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.handle.mode(mode).map_err(CmdError::from_player)
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        self.handle.queue_list().map_err(CmdError::from_player)
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        self.handle.queue_add(paths).map_err(CmdError::from_player)
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        self.handle
            .queue_remove(index)
            .map_err(CmdError::from_player)
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        self.handle.queue_clear().map_err(CmdError::from_player)
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        self.handle.queue_jump(index).map_err(CmdError::from_player)
    }
}

/// `search` 的 running 块：先让用户看到“搜索中”，在线请求结束后再覆盖。
fn search_started_block(block_id: &str, input: &str, keyword: &str) -> Block {
    Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at: now_millis(),
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: vec![text_of(vec![bold("搜索中"), dim(format!(" · {keyword}"))])],
        footer: Some("本地结果与在线源串行搜索，请稍候".to_owned()),
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

/// `play <在线序号>` 的 running 块：resolve/下载都在后台，命令立即返回。
fn online_play_started_block(
    block_id: &str,
    input: &str,
    index: usize,
    track: &SourceTrack,
) -> Block {
    Block {
        id: block_id.to_owned(),
        input: input.to_owned(),
        started_at: now_millis(),
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: vec![text_of(vec![
            bold("准备播放"),
            plain(format!(" · 搜索结果第 {} 首", index + 1)),
            dim(format!(
                " · {} · {}",
                source_label(track.source),
                track.title
            )),
        ])],
        footer: Some("正在解析地址并下载到本地缓存…".to_owned()),
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

/// `search` 是否走“本地 + 在线”；`--local` 是唯一的例外。
#[cfg(test)]
fn search_is_online(parsed: &Parsed) -> bool {
    parsed.spec.name == "search" && !parsed.flags.contains_key("local")
}

/// 解析 `play <n>`，只命中**搜索结果**上下文里的在线结果（T-0043）。
fn online_play_hit(
    parsed: &Parsed,
    listing: Option<&ActiveListing>,
    hits: &[SearchHit],
) -> Option<(usize, SourceTrack)> {
    if parsed.spec.name != "play" {
        return None;
    }
    let listing = listing?;
    if listing.source != ListingSource::Search {
        return None;
    }
    let value = parsed.positional.first()?;
    if !is_unsigned_integer(value) {
        return None;
    }
    let index = user_index(value).ok()?;
    if index >= listing.len {
        return None;
    }
    match hits.get(index)? {
        SearchHit::Online(track) => Some((index, track.clone())),
        SearchHit::Local(_) => None,
    }
}

/// 在线命令的快速分派：命中时立即推 running 并起后台线程，返回回执。
///
/// 非在线命令返回 `None`，交回原来的同步执行器。`search --local` 与队列优先
/// 的 `play <n>` 都不会被这里截走。
fn maybe_start_online(
    app: &AppHandle,
    _handle: &PlayerHandle,
    library: &LibraryHandle,
    input: &str,
    block_id: &str,
) -> Option<CmdExecReply> {
    let parsed = match parse(input) {
        Ok(Some(parsed)) => parsed,
        _ => return None,
    };

    if parsed.spec.name == "search" {
        // 非法参数不在这里吞掉：交回同步执行器，错误文案与 hint 才统一。
        let Ok(options) = SearchOptions::parse(&parsed) else {
            return None;
        };
        if options.local {
            return None;
        }

        if options.up.is_some() || options.mid.is_some() {
            let target = options
                .up
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("mid={}", options.mid.unwrap_or(0)));
            let template = up_started_block(block_id, input, &target);
            emit_block(app, &template);

            let app = app.clone();
            let online = library.online();
            std::thread::spawn(move || {
                let library = app.state::<LibraryHandle>();
                let emit_app = app.clone();
                let emit = move |block: &Block| emit_block(&emit_app, block);
                run_up_search(&options, &*online, &*library, &emit, &template, &now_millis);
            });
            return Some(CmdExecReply {
                block_id: block_id.to_owned(),
                ok: true,
            });
        }

        let template = search_started_block(block_id, input, &options.keyword);
        emit_block(app, &template);

        let app = app.clone();
        let online = library.online();
        std::thread::spawn(move || {
            let library = app.state::<LibraryHandle>();
            let emit_app = app.clone();
            let emit = move |block: &Block| emit_block(&emit_app, block);
            run_online_search_with_options(
                &options,
                &*online,
                &*library,
                &emit,
                &template,
                &now_millis,
            );
        });
        return Some(CmdExecReply {
            block_id: block_id.to_owned(),
            ok: true,
        });
    }

    if parsed.spec.name == "play"
        && parsed
            .positional
            .first()
            .is_some_and(|value| is_unsigned_integer(value))
    {
        let hits = library.search_hits();
        let listing = library.active_listing();
        if let Some((index, track)) = online_play_hit(&parsed, listing.as_ref(), &hits) {
            let template = online_play_started_block(block_id, input, index, &track);
            emit_block(app, &template);

            let app = app.clone();
            let online = library.online();
            std::thread::spawn(move || {
                let handle = app.state::<PlayerHandle>();
                let emit_app = app.clone();
                let emit = move |block: &Block| emit_block(&emit_app, block);
                run_online_play(
                    &track,
                    index,
                    &*online,
                    &*handle,
                    &emit,
                    &template,
                    &now_millis,
                );
            });
            return Some(CmdExecReply {
                block_id: block_id.to_owned(),
                ok: true,
            });
        }
    }

    None
}

/// `cmd.exec`：执行一条终端输入。
///
/// 立即返回 `{ blockId, ok }`，输出经 `terminal:block` 事件推送：
/// 先发 `running` 空块，再发同一 `id` 的最终块（前端按 `id` 覆盖）。
/// `ok` 表示该命令最终是否成功；空输入视为成功且不产生块。
#[tauri::command(rename = "cmd.exec")]
pub(crate) fn cmd_exec(
    app: AppHandle,
    handle: State<'_, PlayerHandle>,
    library: State<'_, LibraryHandle>,
    input: String,
    block_id: String,
) -> CmdExecReply {
    if let Some(reply) = maybe_start_online(&app, &handle, &library, &input, &block_id) {
        return reply;
    }

    let api = HandleApi::new(&handle);
    let online = library.online();
    let theme = ThemeHandle::for_app(app.clone()).ok();
    let services = Services {
        settings: Some(library.settings()),
        theme: theme.as_ref().map(|theme| theme as &dyn ThemeApi),
        online: Some(&*online),
    };

    // 后台扫描可能瞬间结束：先推 running，再启动线程，避免 done 抢在 running 前。
    let full = scan_full_flag(&input);
    if let Some(full) = full {
        emit_block(&app, &scan_started_block(&block_id, &input, full));
    }
    let observer: Arc<dyn LibraryObserver> = if full.is_some() {
        Arc::new(EventObserver::for_app(
            app.clone(),
            block_id.clone(),
            input.clone(),
        ))
    } else {
        Arc::new(NoopLibraryObserver)
    };

    match execute_with_services(&api, &*library, services, &block_id, &input, observer) {
        Execution::Ignored => CmdExecReply { block_id, ok: true },
        Execution::Blocks { running, done } => {
            emit_block(&app, &running);
            let ok = done.status != BlockStatus::Error;
            emit_block(&app, &done);
            CmdExecReply { block_id, ok }
        }
        // 已受理：running 已在启动后台线程前推送，最终块由观察者推送。
        Execution::Accepted { running } => CmdExecReply {
            block_id: running.id,
            ok: true,
        },
    }
}

pub(crate) fn emit_block(app: &AppHandle, block: &Block) {
    if let Err(error) = app.emit(EVENT_TERMINAL_BLOCK, block) {
        eprintln!("[cmd] emit {EVENT_TERMINAL_BLOCK} failed: {error}");
    }
}

/// `cmd.complete` 的纯逻辑：算候选，动态候选按需取一次播放器快照。
///
/// 快照失败（`E_BUSY` 等）只让动态候选降级为空列表，命令名 / 子命令 / 枚举值
/// 照常返回——补全不能被播放器状态卡住（T-0020 §6.1、§11）。
/// `cursor` 与返回的 `replaceStart` / `replaceEnd` 都是**字符偏移**。
pub(crate) fn complete_for(api: &dyn PlayerApi, input: &str, cursor: usize) -> Completion {
    complete(input, cursor, || {
        api.snapshot().ok().and_then(|state| state.queue_length)
    })
}

/// `cmd.complete`：一次补全请求，返回 `{ replaceStart, replaceEnd, items }`。
///
/// 区间单位是字符（码点），不是字节也不是 UTF-16；前端负责与
/// `selectionStart`（UTF-16）互转。无候选时 `items` 为空，调用不报错。
#[tauri::command(rename = "cmd.complete")]
pub(crate) fn cmd_complete(
    handle: State<'_, PlayerHandle>,
    input: String,
    cursor: usize,
) -> Completion {
    complete_for(&HandleApi::new(&handle), &input, cursor)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use np_core::command::parse;
    use np_core::{PlaybackMode, PlaybackStatus, RepeatMode};
    use np_sources::{DownloadOutcome, ResolvedStream};

    use super::*;

    /// `FakeApi` 记录的一次调用。
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Snapshot,
        Play(Option<String>),
        Pause,
        Resume,
        Next,
        Prev,
        Seek(u64),
        Volume(f32),
        Mode(PlaybackMode),
        QueueList,
        QueueAdd(Vec<String>),
        QueueRemove(usize),
        QueueClear,
        QueueJump(usize),
    }

    /// 单测用播放器：记录调用、返回构造值，不碰真实设备。
    struct FakeApi {
        calls: Mutex<Vec<Call>>,
        state: Mutex<PlaybackState>,
        queue: Mutex<QueueView>,
        reject_paths: Mutex<Vec<String>>,
        playback_error: Mutex<Option<CmdError>>,
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                state: Mutex::new(PlaybackState {
                    volume: 0.5,
                    ..PlaybackState::default()
                }),
                queue: Mutex::new(QueueView {
                    tracks: Vec::new(),
                    index: None,
                }),
                reject_paths: Mutex::new(Vec::new()),
                playback_error: Mutex::new(None),
            }
        }

        fn with_state(self, state: PlaybackState) -> Self {
            *self.state.lock().expect("state mutex") = state;
            self
        }

        fn with_queue(self, queue: QueueView) -> Self {
            *self.queue.lock().expect("queue mutex") = queue;
            self
        }

        fn with_playback_error(self, error: CmdError) -> Self {
            *self.playback_error.lock().expect("error mutex") = Some(error);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("calls mutex").clone()
        }

        fn record(&self, call: Call) {
            self.calls.lock().expect("calls mutex").push(call);
        }

        /// 播放类命令的公共返回：配置了错误就回错误，否则回当前快照。
        fn playback(&self) -> Result<PlaybackState, CmdError> {
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            Ok(self.state.lock().expect("state mutex").clone())
        }
    }

    impl PlayerApi for FakeApi {
        fn snapshot(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Snapshot);
            self.playback()
        }

        fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
            self.record(Call::Play(path.clone()));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            {
                let mut state = self.state.lock().expect("state mutex");
                if path.is_some() {
                    state.track_id = path;
                }
                state.status = PlaybackStatus::Playing;
            }
            self.playback()
        }

        fn pause(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Pause);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Paused;
            self.playback()
        }

        fn resume(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Resume);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Playing;
            self.playback()
        }

        fn next(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Next);
            self.playback()
        }

        fn prev(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Prev);
            self.playback()
        }

        fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
            self.record(Call::Seek(position_ms));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").position_ms = position_ms;
            self.playback()
        }

        fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
            self.record(Call::Volume(volume));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").volume = volume;
            self.playback()
        }

        fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
            self.record(Call::Mode(mode));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").mode = mode;
            self.playback()
        }

        fn queue_list(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueList);
            Ok(self.queue.lock().expect("queue mutex").clone())
        }

        fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
            self.record(Call::QueueAdd(paths.clone()));
            {
                let reject = self.reject_paths.lock().expect("reject mutex");
                if let Some(path) = paths.iter().find(|path| reject.contains(path)) {
                    return Err(CmdError::new("E_DECODE", format!("无法加入：{path}")));
                }
            }
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.extend(paths);
            if queue.index.is_none() {
                queue.index = Some(0);
            }
            Ok(queue.clone())
        }

        fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueRemove(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.tracks.remove(index);
            if queue.tracks.is_empty() {
                queue.index = None;
            } else if let Some(current) = queue.index {
                if index < current {
                    queue.index = Some(current - 1);
                } else if index == current {
                    queue.index = Some(current.min(queue.tracks.len() - 1));
                }
            }
            Ok(queue.clone())
        }

        fn queue_clear(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueClear);
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.clear();
            queue.index = None;
            Ok(queue.clone())
        }

        fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueJump(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.index = Some(index);
            Ok(queue.clone())
        }
    }

    fn queue_view(tracks: &[&str], index: Option<usize>) -> QueueView {
        QueueView {
            tracks: tracks.iter().map(|track| (*track).to_owned()).collect(),
            index,
        }
    }

    /// 构造一首假曲目；只填命令层用得到的字段。
    fn track(id: i64, title: &str, artist: &str, path: &str, duration_ms: i64) -> Track {
        Track {
            id,
            kind: "local".to_owned(),
            path: path.to_owned(),
            source: "local".to_owned(),
            source_id: String::new(),
            title: title.to_owned(),
            album_id: None,
            album: String::new(),
            artist: artist.to_owned(),
            album_artist: String::new(),
            track_no: 0,
            disc_no: 0,
            year: 0,
            duration_ms,
            sample_rate: 44_100,
            channels: 2,
            bitrate: 320,
            codec: "flac".to_owned(),
            cover_key: None,
            size_bytes: 0,
            mtime_ms: 0,
            added_at: 0,
            last_seen_at: 0,
        }
    }

    /// `FakeLibraryApi` 记录的一次调用。
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum LibraryCall {
        AddRoot(String),
        ListRoots,
        Scan(bool),
        Search(String, usize),
        ListRecent(usize),
        Stats,
        Covers,
    }

    struct FakeLibraryState {
        roots: Vec<String>,
        list_tracks: Vec<Track>,
        search_tracks: Vec<Track>,
        stats: Stats,
        covers: CoverCacheUsage,
        busy: bool,
        session: Vec<SearchHit>,
        active_listing: Option<ActiveListing>,
        library_listing: Vec<Track>,
        tracks_by_path: std::collections::HashMap<String, Track>,
        playlists: Vec<Playlist>,
        playlist_items: HashMap<i64, Vec<PlaylistItem>>,
        next_playlist_id: i64,
        next_item_id: i64,
        calls: Vec<LibraryCall>,
    }

    /// 单测用媒体库：不碰文件系统，行为完全由构造值决定。
    struct FakeLibraryApi {
        state: Mutex<FakeLibraryState>,
    }

    impl FakeLibraryApi {
        fn new() -> Self {
            Self {
                state: Mutex::new(FakeLibraryState {
                    roots: Vec::new(),
                    list_tracks: Vec::new(),
                    search_tracks: Vec::new(),
                    stats: Stats::default(),
                    covers: CoverCacheUsage::default(),
                    busy: false,
                    session: Vec::new(),
                    active_listing: None,
                    library_listing: Vec::new(),
                    tracks_by_path: std::collections::HashMap::new(),
                    playlists: Vec::new(),
                    playlist_items: HashMap::new(),
                    next_playlist_id: 1,
                    next_item_id: 1,
                    calls: Vec::new(),
                }),
            }
        }

        fn with_roots(self, roots: &[&str]) -> Self {
            self.state.lock().expect("state mutex").roots =
                roots.iter().map(|root| (*root).to_owned()).collect();
            self
        }

        fn with_list_tracks(self, tracks: Vec<Track>) -> Self {
            self.state.lock().expect("state mutex").list_tracks = tracks;
            self
        }

        fn with_search_tracks(self, tracks: Vec<Track>) -> Self {
            self.state.lock().expect("state mutex").search_tracks = tracks;
            self
        }

        fn with_track_by_path(self, path: &str, track: Track) -> Self {
            self.state
                .lock()
                .expect("state mutex")
                .tracks_by_path
                .insert(path.to_owned(), track);
            self
        }

        fn with_stats(self, stats: Stats) -> Self {
            self.state.lock().expect("state mutex").stats = stats;
            self
        }

        fn with_covers(self, files: u64, bytes: u64) -> Self {
            self.state.lock().expect("state mutex").covers = CoverCacheUsage { files, bytes };
            self
        }

        fn busy(self) -> Self {
            self.state.lock().expect("state mutex").busy = true;
            self
        }

        fn calls(&self) -> Vec<LibraryCall> {
            self.state.lock().expect("state mutex").calls.clone()
        }

        fn session(&self) -> Vec<Track> {
            self.state
                .lock()
                .expect("state mutex")
                .session
                .iter()
                .filter_map(|hit| match hit {
                    SearchHit::Local(track) => Some(track.clone()),
                    SearchHit::Online(_) => None,
                })
                .collect()
        }

        fn hits(&self) -> Vec<SearchHit> {
            self.state.lock().expect("state mutex").session.clone()
        }

        fn record(&self, call: LibraryCall) {
            self.state.lock().expect("state mutex").calls.push(call);
        }
    }

    impl LibraryApi for FakeLibraryApi {
        fn add_root(&self, path: &str) -> Result<usize, CmdError> {
            self.record(LibraryCall::AddRoot(path.to_owned()));
            let mut state = self.state.lock().expect("state mutex");
            if state.roots.iter().any(|root| root == path) {
                return Err(CmdError::new("E_CMD_ARG", format!("扫描根已存在：{path}")));
            }
            state.roots.push(path.to_owned());
            Ok(state.roots.len())
        }

        fn list_roots(&self) -> Result<Vec<String>, CmdError> {
            self.record(LibraryCall::ListRoots);
            Ok(self.state.lock().expect("state mutex").roots.clone())
        }

        fn scan(&self, full: bool, _observer: Arc<dyn LibraryObserver>) -> Result<(), CmdError> {
            self.record(LibraryCall::Scan(full));
            let state = self.state.lock().expect("state mutex");
            if state.busy {
                return Err(CmdError::new("E_BUSY", "已有扫描在进行中"));
            }
            if state.roots.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "还没有扫描根"));
            }
            Ok(())
        }

        fn search(&self, keyword: &str, limit: usize) -> Result<Vec<Track>, CmdError> {
            self.record(LibraryCall::Search(keyword.to_owned(), limit));
            let state = self.state.lock().expect("state mutex");
            Ok(state.search_tracks.iter().take(limit).cloned().collect())
        }

        fn list_recent(&self, limit: usize) -> Result<Vec<Track>, CmdError> {
            self.record(LibraryCall::ListRecent(limit));
            let state = self.state.lock().expect("state mutex");
            Ok(state.list_tracks.iter().take(limit).cloned().collect())
        }

        fn stats(&self) -> Result<Stats, CmdError> {
            self.record(LibraryCall::Stats);
            Ok(self.state.lock().expect("state mutex").stats)
        }

        fn covers_usage(&self) -> Result<CoverCacheUsage, CmdError> {
            self.record(LibraryCall::Covers);
            Ok(self.state.lock().expect("state mutex").covers)
        }

        fn remember_search(&self, tracks: Vec<Track>) {
            self.state.lock().expect("state mutex").session =
                tracks.into_iter().map(SearchHit::Local).collect();
        }

        fn search_results(&self) -> Vec<Track> {
            self.state
                .lock()
                .expect("state mutex")
                .session
                .iter()
                .filter_map(|hit| match hit {
                    SearchHit::Local(track) => Some(track.clone()),
                    SearchHit::Online(_) => None,
                })
                .collect()
        }

        fn remember_hits(&self, hits: Vec<SearchHit>) {
            self.state.lock().expect("state mutex").session = hits;
        }

        fn search_hits(&self) -> Vec<SearchHit> {
            self.state.lock().expect("state mutex").session.clone()
        }

        fn set_active_listing(&self, listing: ActiveListing) {
            self.state.lock().expect("state mutex").active_listing = Some(listing);
        }

        fn active_listing(&self) -> Option<ActiveListing> {
            self.state
                .lock()
                .expect("state mutex")
                .active_listing
                .clone()
        }

        fn remember_library_listing(&self, tracks: Vec<Track>) {
            self.state.lock().expect("state mutex").library_listing = tracks;
        }

        fn library_listing(&self) -> Vec<Track> {
            self.state
                .lock()
                .expect("state mutex")
                .library_listing
                .clone()
        }

        fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
            Ok(self
                .state
                .lock()
                .expect("state mutex")
                .tracks_by_path
                .get(path)
                .cloned())
        }

        fn playlist_create(&self, name: &str) -> Result<i64, CmdError> {
            let name = np_library::validate_playlist_name(name)
                .map_err(|error| CmdError::new("E_CMD_ARG", error.to_string()))?;
            let mut state = self.state.lock().expect("state mutex");
            if state
                .playlists
                .iter()
                .any(|playlist| playlist.name.eq_ignore_ascii_case(&name))
            {
                return Err(CmdError::new("E_CMD_ARG", format!("歌单已存在：{name}")));
            }
            let id = state.next_playlist_id;
            state.next_playlist_id += 1;
            state.playlists.push(Playlist {
                id,
                name,
                created_at: 0,
                updated_at: 0,
            });
            state.playlist_items.insert(id, Vec::new());
            Ok(id)
        }

        fn playlist_rename(&self, id: i64, name: &str) -> Result<(), CmdError> {
            let name = np_library::validate_playlist_name(name)
                .map_err(|error| CmdError::new("E_CMD_ARG", error.to_string()))?;
            let mut state = self.state.lock().expect("state mutex");
            if state
                .playlists
                .iter()
                .any(|playlist| playlist.id != id && playlist.name.eq_ignore_ascii_case(&name))
            {
                return Err(CmdError::new("E_CMD_ARG", format!("歌单已存在：{name}")));
            }
            let playlist = state
                .playlists
                .iter_mut()
                .find(|playlist| playlist.id == id)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
            playlist.name = name;
            Ok(())
        }

        fn playlist_delete(&self, id: i64) -> Result<(), CmdError> {
            let mut state = self.state.lock().expect("state mutex");
            let before = state.playlists.len();
            state.playlists.retain(|playlist| playlist.id != id);
            if state.playlists.len() == before {
                return Err(CmdError::new("E_CMD_ARG", "歌单不存在"));
            }
            state.playlist_items.remove(&id);
            Ok(())
        }

        fn playlist_list(&self) -> Result<Vec<PlaylistSummary>, CmdError> {
            let state = self.state.lock().expect("state mutex");
            Ok(state
                .playlists
                .iter()
                .map(|playlist| PlaylistSummary {
                    id: playlist.id,
                    name: playlist.name.clone(),
                    item_count: state
                        .playlist_items
                        .get(&playlist.id)
                        .map_or(0, |items| items.len() as u64),
                    created_at: playlist.created_at,
                    updated_at: playlist.updated_at,
                })
                .collect())
        }

        fn playlist_by_name(&self, name: &str) -> Result<Option<Playlist>, CmdError> {
            let state = self.state.lock().expect("state mutex");
            Ok(state
                .playlists
                .iter()
                .find(|playlist| playlist.name.eq_ignore_ascii_case(name))
                .cloned())
        }

        fn playlist_get(&self, id: i64) -> Result<(Playlist, Vec<PlaylistItem>), CmdError> {
            let state = self.state.lock().expect("state mutex");
            let playlist = state
                .playlists
                .iter()
                .find(|playlist| playlist.id == id)
                .cloned()
                .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
            let items = state.playlist_items.get(&id).cloned().unwrap_or_default();
            Ok((playlist, items))
        }

        fn playlist_add(&self, id: i64, item: &NewPlaylistItem) -> Result<PlaylistItem, CmdError> {
            let mut state = self.state.lock().expect("state mutex");
            if !state.playlists.iter().any(|playlist| playlist.id == id) {
                return Err(CmdError::new("E_CMD_ARG", "歌单不存在"));
            }
            let item_id = state.next_item_id;
            state.next_item_id += 1;
            let items = state.playlist_items.entry(id).or_default();
            let position = items.len() as i64;
            let added = PlaylistItem {
                id: item_id,
                playlist_id: id,
                position,
                kind: item.kind.clone(),
                path: item.path.clone(),
                source: item.source.clone(),
                source_id: item.source_id.clone(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                duration_ms: item.duration_ms,
                added_at: 0,
            };
            items.push(added.clone());
            Ok(added)
        }

        fn playlist_remove(&self, id: i64, position: i64) -> Result<(), CmdError> {
            let mut state = self.state.lock().expect("state mutex");
            let items = state
                .playlist_items
                .get_mut(&id)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", "歌单不存在"))?;
            if position < 0 || position as usize >= items.len() {
                return Err(CmdError::new(
                    "E_CMD_ARG",
                    format!("歌单里没有第 {} 首", position + 1),
                ));
            }
            items.remove(position as usize);
            for (index, item) in items.iter_mut().enumerate() {
                item.position = index as i64;
            }
            Ok(())
        }
    }

    #[test]
    fn fake_api_records_calls() {
        let api = FakeApi::new();
        api.pause().expect("pause succeeds");
        api.seek(1_500).expect("seek succeeds");

        assert_eq!(api.calls(), vec![Call::Pause, Call::Seek(1_500)]);
    }

    #[test]
    fn parse_error_keeps_code_and_usage_hint() {
        let error = parse("plya").expect_err("unknown command must fail");
        let error = CmdError::from_parse(&error);

        assert_eq!(error.code, "E_CMD_UNKNOWN");
        assert_eq!(error.hint.as_deref(), Some("did you mean 'play'?"));
    }

    #[test]
    fn player_error_maps_to_ipc_code() {
        let error = CmdError::from_player(PlayerError::unsupported("没有可暂停的曲目"));

        assert_eq!(error.code, "E_UNSUPPORTED");
        assert_eq!(error.message, "没有可暂停的曲目");
        assert_eq!(error.hint, None);
    }

    #[test]
    fn usage_hint_is_filled_when_missing() {
        let parsed = parse("vol 5")
            .expect("valid input")
            .expect("non-empty input");
        let error = CmdError::new("E_DECODE", "设备丢失").with_usage(&parsed);

        assert_eq!(error.hint.as_deref(), Some("用法：vol <0-100|±N>"));
    }

    #[test]
    fn repeat_mode_defaults_to_off_for_unknown_values() {
        // 解析层已把枚举归一化；这里只锁住“未知值绝不 panic”。
        let mode = PlaybackMode {
            repeat: RepeatMode::Off,
            shuffle: false,
        };
        assert_eq!(mode, PlaybackMode::default());
    }

    // ---- 执行器：命令映射 ----

    fn done_block(api: &FakeApi, input: &str) -> Block {
        match execute(api, "block-1", input) {
            Execution::Blocks { done, .. } => *done,
            Execution::Accepted { .. } => panic!("unexpected accepted block for input: {input}"),
            Execution::Ignored => panic!("expected a block for input: {input}"),
        }
    }

    fn text_of_block(block: &Block) -> String {
        block
            .outputs
            .iter()
            .filter_map(|output| match output {
                Output::Text { spans, .. } => Some(
                    spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn error_fields(block: &Block) -> (&str, &str, Option<&str>) {
        block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Error {
                    code,
                    message,
                    hint,
                    ..
                } => Some((code.as_str(), message.as_str(), hint.as_deref())),
                _ => None,
            })
            .expect("block must contain an error output")
    }

    #[test]
    fn play_without_argument_resumes() {
        let api = FakeApi::new();
        let block = done_block(&api, "play");

        assert_eq!(api.calls(), vec![Call::Resume]);
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("继续播放"));
    }

    #[test]
    fn play_number_is_one_based_and_jumps_after_queue_list() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], None));
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, "queue list");
        let block = done_with_library(&api, &library, "play 3");

        assert_eq!(
            api.calls(),
            vec![Call::QueueList, Call::QueueList, Call::QueueJump(2)]
        );
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("第 3 首"));
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：播放队列"))
        );
    }

    #[test]
    fn play_zero_is_rejected_before_any_call() {
        let api = FakeApi::new();
        let block = done_block(&api, "play 0");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn play_path_is_passed_through() {
        let api = FakeApi::new();
        let block = done_block(&api, r"play C:\Music\a.flac");

        assert_eq!(
            api.calls(),
            vec![Call::Play(Some(r"C:\Music\a.flac".to_owned()))]
        );
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("a.flac"));
    }

    #[test]
    fn pause_resume_next_prev_map_to_same_named_calls() {
        let api = FakeApi::new();

        for input in ["pause", "resume", "next", "prev"] {
            assert_eq!(done_block(&api, input).status, BlockStatus::Done);
        }

        assert_eq!(
            api.calls(),
            vec![Call::Pause, Call::Resume, Call::Next, Call::Prev]
        );
    }

    #[test]
    fn vol_absolute_sets_level_and_clamps_high() {
        let api = FakeApi::new();
        let block = done_block(&api, "vol 150");

        assert_eq!(api.calls(), vec![Call::Volume(1.0)]);
        assert!(text_of_block(&block).contains("100%"));
    }

    #[test]
    fn vol_absolute_zero_is_allowed() {
        let api = FakeApi::new();
        let block = done_block(&api, "vol 0");

        assert_eq!(api.calls(), vec![Call::Volume(0.0)]);
        assert!(text_of_block(&block).contains("0%"));
    }

    #[test]
    fn vol_relative_reads_current_level_first() {
        let api = FakeApi::new().with_state(PlaybackState {
            volume: 0.42,
            ..PlaybackState::default()
        });
        let block = done_block(&api, "vol +8");

        let calls = api.calls();
        assert_eq!(calls.first(), Some(&Call::Snapshot));
        match calls.get(1) {
            Some(Call::Volume(volume)) => assert!((volume - 0.5).abs() < 1e-6),
            other => panic!("expected volume call, got {other:?}"),
        }
        assert!(text_of_block(&block).contains("50%"));
    }

    #[test]
    fn vol_relative_clamps_low() {
        let api = FakeApi::new().with_state(PlaybackState {
            volume: 0.5,
            ..PlaybackState::default()
        });
        let block = done_block(&api, "vol -200");

        assert_eq!(api.calls(), vec![Call::Snapshot, Call::Volume(0.0)]);
        assert!(text_of_block(&block).contains("0%"));
    }

    #[test]
    fn seek_plain_number_is_milliseconds() {
        let api = FakeApi::new();
        done_block(&api, "seek 90000");

        assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
    }

    #[test]
    fn seek_mm_ss_is_absolute() {
        let api = FakeApi::new();
        done_block(&api, "seek 1:30");

        assert_eq!(api.calls(), vec![Call::Seek(90_000)]);
    }

    #[test]
    fn seek_relative_seconds_uses_current_position() {
        let api = FakeApi::new().with_state(PlaybackState {
            position_ms: 30_000,
            duration_ms: Some(180_000),
            ..PlaybackState::default()
        });
        done_block(&api, "seek +15");

        assert_eq!(api.calls(), vec![Call::Snapshot, Call::Seek(45_000)]);
    }

    #[test]
    fn seek_relative_clamps_to_track_bounds() {
        let api = FakeApi::new().with_state(PlaybackState {
            position_ms: 30_000,
            duration_ms: Some(180_000),
            ..PlaybackState::default()
        });

        done_block(&api, "seek -40");
        done_block(&api, "seek +999");

        assert_eq!(
            api.calls(),
            vec![
                Call::Snapshot,
                Call::Seek(0),
                Call::Snapshot,
                Call::Seek(180_000)
            ]
        );
    }

    #[test]
    fn mode_repeat_sends_complete_mode_once() {
        let api = FakeApi::new().with_state(PlaybackState {
            mode: PlaybackMode {
                repeat: RepeatMode::Off,
                shuffle: true,
            },
            ..PlaybackState::default()
        });
        let block = done_block(&api, "mode repeat all");

        let calls = api.calls();
        assert_eq!(calls.first(), Some(&Call::Snapshot));
        let modes: Vec<&Call> = calls
            .iter()
            .filter(|call| matches!(call, Call::Mode(_)))
            .collect();
        assert_eq!(modes.len(), 1, "mode 必须一次下发完整对象");
        assert_eq!(
            modes[0],
            &Call::Mode(PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: true,
            })
        );
        assert!(text_of_block(&block).contains("循环：列表"));
    }

    #[test]
    fn mode_shuffle_keeps_repeat_from_snapshot() {
        let api = FakeApi::new().with_state(PlaybackState {
            mode: PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: false,
            },
            ..PlaybackState::default()
        });
        let block = done_block(&api, "mode shuffle on");

        assert_eq!(
            api.calls(),
            vec![
                Call::Snapshot,
                Call::Mode(PlaybackMode {
                    repeat: RepeatMode::All,
                    shuffle: true,
                })
            ]
        );
        assert!(text_of_block(&block).contains("随机：开"));
    }

    #[test]
    fn queue_list_shows_local_metadata_in_four_columns() {
        let api =
            FakeApi::new().with_queue(queue_view(&["/music/a.flac", "/music/b.flac"], Some(1)));
        let library = FakeLibraryApi::new()
            .with_track_by_path(
                "/music/a.flac",
                track(1, "第一首", "甲", "/music/a.flac", 65_000),
            )
            .with_track_by_path(
                "/music/b.flac",
                track(2, "第二首", "乙", "/music/b.flac", 125_000),
            );
        let block = done_with_library(&api, &library, "queue list");

        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns, ["序号", "标题", "艺人", "时长"]);
        assert_eq!(rows[0], ["1", "第一首", "甲", "01:05"]);
        assert_eq!(rows[1], ["▶ 2", "第二首", "乙", "02:05"]);
    }

    #[test]
    fn queue_list_shows_remote_source_column() {
        let api = FakeApi::new().with_queue(queue_view(&["/cache/bili.m4a"], Some(0)));
        let mut remote = track(0, "远程歌", "上传者", "/cache/bili.m4a", 200_000);
        remote.kind = "remote".to_owned();
        remote.source = "bilibili".to_owned();
        let library = FakeLibraryApi::new().with_track_by_path("/cache/bili.m4a", remote);
        let block = done_with_library(&api, &library, "queue list");

        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns, ["序号", "标题", "艺人", "时长", "来源"]);
        assert_eq!(rows[0], ["▶ 1", "远程歌", "上传者", "03:20", "B站"]);
    }

    #[test]
    fn queue_list_falls_back_to_filename_and_dashes() {
        let api = FakeApi::new().with_queue(queue_view(&["/music/missing.flac"], None));
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "queue list");

        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns.len(), 4);
        assert_eq!(rows[0], ["1", "missing.flac", "—", "—"]);
    }

    #[test]
    fn queue_list_mixed_queue_uses_source_column_with_local_dash() {
        let api = FakeApi::new().with_queue(queue_view(&["/music/a.flac", "/cache/net.m4a"], None));
        let mut remote = track(0, "云歌", "云艺人", "/cache/net.m4a", 90_000);
        remote.kind = "remote".to_owned();
        remote.source = "netease".to_owned();
        let library = FakeLibraryApi::new()
            .with_track_by_path(
                "/music/a.flac",
                track(1, "本地歌", "本地艺人", "/music/a.flac", 60_000),
            )
            .with_track_by_path("/cache/net.m4a", remote);
        let block = done_with_library(&api, &library, "queue list");

        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns.last().copied(), Some("来源"));
        assert_eq!(rows[0][4], "—");
        assert_eq!(rows[1][4], "网易云");
    }

    #[test]
    fn queue_list_empty_still_has_metadata_columns() {
        let api = FakeApi::new().with_queue(queue_view(&[], None));
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "queue list");

        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns.len(), 4);
        assert!(rows.is_empty());
    }

    #[test]
    fn queue_add_accepts_existing_file() {
        let dir = TestDir::new("queue-add-file");
        let path = dir.path().join("a b.flac");
        fs::write(&path, b"fake").expect("write");
        let input = format!("queue add \"{}\"", path.display());
        let api = FakeApi::new();
        let block = done_block(&api, &input);

        assert_eq!(
            api.calls(),
            vec![Call::QueueAdd(vec![path.to_string_lossy().into_owned()])]
        );
        assert!(text_of_block(&block).contains("已加入 1 首"));
        assert_eq!(block.footer.as_deref(), Some("队列现有 1 首"));
    }

    #[test]
    fn queue_add_missing_path_reports_file_not_found() {
        let api = FakeApi::new();
        let block = done_block(&api, "queue add 1");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert_eq!(error_fields(&block).1, "文件不存在：1");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn queue_add_requires_a_path_or_from_search() {
        let api = FakeApi::new();
        let block = done_block(&api, "queue add");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn queue_add_from_search_adds_local_hit() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "First", "Artist", "/music/first.flac", 1_000),
            track(2, "Second", "Artist", "/music/second.flac", 1_000),
        ]);
        done_with_library(&api, &library, "search artist");
        let block = done_with_library(&api, &library, "queue add --from-search 2");

        assert_eq!(
            api.calls(),
            vec![Call::QueueAdd(vec!["/music/second.flac".to_owned()])]
        );
        assert!(text_of_block(&block).contains("已加入 1 首"));
    }

    #[test]
    fn queue_add_rejects_path_and_from_search_together() {
        let dir = TestDir::new("queue-add-both");
        let path = dir.path().join("a.flac");
        fs::write(&path, b"fake").expect("write");
        let input = format!("queue add \"{}\" --from-search 1", path.display());
        let api = FakeApi::new();
        let block = done_block(&api, &input);

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn queue_add_from_search_out_of_range_fails() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Only",
            "Artist",
            "/music/only.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search only");
        let block = done_with_library(&api, &library, "queue add --from-search 2");

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("搜索结果只有 1 条"));
        assert!(api.calls().is_empty());
    }

    #[test]
    fn queue_add_from_search_online_prepares_then_queues_local_path() {
        let dir = TestDir::new("queue-add-online");
        let cached = dir.path().join("cached.m4a");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        library.remember_hits(vec![SearchHit::Online(online_track(
            SourceId::Bilibili,
            "BV1xx",
            "在线",
        ))]);
        library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: 1,
        });
        let online = FakeQueueOnline {
            path: cached.clone(),
        };
        let services = Services {
            settings: None,
            theme: None,
            online: Some(&online),
        };

        let block = match execute_with_services(
            &api,
            &library,
            services,
            "queue-add-online-block",
            "queue add --from-search 1",
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Blocks { done, .. } => *done,
            _ => panic!("expected blocks"),
        };

        assert_eq!(
            api.calls(),
            vec![Call::QueueAdd(vec![cached.to_string_lossy().into_owned()])]
        );
        assert!(text_of_block(&block).contains("已加入 1 首"));
    }

    #[test]
    fn queue_add_from_search_without_number_fails() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "queue add --from-search");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(api.calls().is_empty());
    }

    #[test]
    fn queue_add_success_does_not_replace_active_search_listing() {
        let api = FakeApi::new().with_queue(queue_view(&["queued.flac"], None));
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Search",
            "Artist",
            "/music/search.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search song");
        done_with_library(&api, &library, "queue add --from-search 1");
        let block = done_with_library(&api, &library, "play 1");

        assert_eq!(
            api.calls(),
            vec![
                Call::QueueAdd(vec!["/music/search.flac".to_owned()]),
                Call::Play(Some("/music/search.flac".to_owned()))
            ]
        );
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：搜索结果"))
        );
    }

    #[test]
    fn queue_remove_uses_one_based_index() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
        let block = done_block(&api, "queue remove 2");

        assert_eq!(api.calls(), vec![Call::QueueRemove(1)]);
        assert!(text_of_block(&block).contains("第 2 首"));
        assert_eq!(block.footer.as_deref(), Some("队列还剩 2 首"));
    }

    #[test]
    fn queue_jump_uses_one_based_index() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(0)));
        let block = done_block(&api, "queue jump 3");

        assert_eq!(api.calls(), vec![Call::QueueJump(2)]);
        assert!(text_of_block(&block).contains("第 3 首"));
    }

    #[test]
    fn queue_clear_empties_the_queue() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac"], Some(0)));
        let block = done_block(&api, "queue clear");

        assert_eq!(api.calls(), vec![Call::QueueClear]);
        assert!(text_of_block(&block).contains("清空"));
    }

    #[test]
    fn parse_error_becomes_error_block_with_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "plya");

        assert_eq!(block.status, BlockStatus::Error);
        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_UNKNOWN");
        assert_eq!(hint, Some("did you mean 'play'?"));
        assert!(api.calls().is_empty());
    }

    #[test]
    fn argument_error_carries_usage_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "seek 1:2:3");

        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        let hint = hint.expect("hint must not be empty");
        assert!(
            hint.contains("用法：seek <ms|mm:ss|±s>"),
            "unexpected hint: {hint}"
        );
    }

    #[test]
    fn unclosed_quote_error_still_has_a_hint() {
        let api = FakeApi::new();
        let block = done_block(&api, "play \"unterminated");

        let (code, _, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        assert!(hint.is_some_and(|hint| !hint.is_empty()));
    }

    #[test]
    fn api_error_becomes_error_block_with_usage_hint() {
        let api =
            FakeApi::new().with_playback_error(CmdError::new("E_UNSUPPORTED", "没有可暂停的曲目"));
        let block = done_block(&api, "pause");

        assert_eq!(block.status, BlockStatus::Error);
        let (code, message, hint) = error_fields(&block);
        assert_eq!(code, "E_UNSUPPORTED");
        assert_eq!(message, "没有可暂停的曲目");
        assert_eq!(hint, Some("用法：pause"));
    }

    #[test]
    fn help_lists_every_registered_command() {
        let api = FakeApi::new();
        let block = done_block(&api, "help");
        let text = text_of_block(&block);

        for spec in registry().specs() {
            assert!(text.contains(spec.name), "help 缺少命令：{}", spec.name);
        }
    }

    #[test]
    fn help_with_argument_shows_usage() {
        let api = FakeApi::new();
        let block = done_block(&api, "help seek");

        assert!(text_of_block(&block).contains("seek <ms|mm:ss|±s>"));
    }

    #[test]
    fn version_reports_crate_version() {
        let api = FakeApi::new();
        let block = done_block(&api, "version");

        assert!(text_of_block(&block).contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn clear_is_rejected_with_explanation() {
        let api = FakeApi::new();
        let block = done_block(&api, "clear");

        let (code, message, hint) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        assert!(message.contains("clear 由终端本地处理"));
        assert!(hint.is_some_and(|hint| !hint.is_empty()));
    }

    #[test]
    fn blank_input_produces_no_block() {
        let api = FakeApi::new();

        assert!(matches!(
            execute(&api, "block-1", "   "),
            Execution::Ignored
        ));
    }

    #[test]
    fn block_is_running_then_done_with_the_same_id() {
        let api = FakeApi::new();
        let Execution::Blocks { running, done } = execute(&api, "id-1", "pause") else {
            panic!("expected blocks");
        };

        assert_eq!(running.id, "id-1");
        assert_eq!(running.status, BlockStatus::Running);
        assert!(running.outputs.is_empty());
        assert_eq!(running.duration_ms, None);

        assert_eq!(done.id, running.id);
        assert_eq!(done.input, "pause");
        assert_eq!(done.status, BlockStatus::Done);
        assert!(done.duration_ms.is_some());
        assert!(done.started_at > 0);
        assert_eq!(done.schema_version, SCHEMA_VERSION);
    }

    // ---- 媒体库命令（T-0023） ----

    fn done_with_playlist_online(
        api: &FakeApi,
        library: &FakeLibraryApi,
        online: &dyn OnlineApi,
        input: &str,
    ) -> Block {
        let services = Services {
            settings: None,
            theme: None,
            online: Some(online),
        };
        match execute_with_services(
            api,
            library,
            services,
            "block-playlist-online",
            input,
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Blocks { done, .. } => *done,
            _ => panic!("expected a finished block for {input}"),
        }
    }

    fn done_with_library(api: &FakeApi, library: &FakeLibraryApi, input: &str) -> Block {
        match execute_with_library(
            api,
            library,
            "block-lib",
            input,
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Blocks { done, .. } => *done,
            Execution::Accepted { .. } => panic!("expected a finished block for: {input}"),
            Execution::Ignored => panic!("expected a block for: {input}"),
        }
    }

    fn accepted_with_library(api: &FakeApi, library: &FakeLibraryApi, input: &str) -> Block {
        match execute_with_library(
            api,
            library,
            "block-lib",
            input,
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Accepted { running } => *running,
            Execution::Blocks { done, .. } => {
                panic!("expected accepted block for {input}, got {:?}", done.status)
            }
            Execution::Ignored => panic!("expected a block for: {input}"),
        }
    }

    #[test]
    fn library_add_reports_the_root_count() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, r"library add C:\Music");

        assert_eq!(
            library.calls(),
            vec![LibraryCall::AddRoot(r"C:\Music".to_owned())]
        );
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("已添加扫描根"));
        assert_eq!(
            block.footer.as_deref(),
            Some("当前 1 个扫描根 · 输入 library scan 开始扫描")
        );
    }

    #[test]
    fn library_add_rejects_a_duplicate_root() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
        let block = done_with_library(&api, &library, r"library add C:\Music");

        assert_eq!(block.status, BlockStatus::Error);
        let (code, message, _) = error_fields(&block);
        assert_eq!(code, "E_CMD_ARG");
        assert!(message.contains("已存在"), "{message}");
    }

    #[test]
    fn library_scan_is_accepted_immediately() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
        let block = accepted_with_library(&api, &library, "library scan");

        assert_eq!(block.status, BlockStatus::Running);
        assert!(text_of_block(&block).contains("扫描已开始"));
        assert_eq!(library.calls(), vec![LibraryCall::Scan(false)]);
    }

    #[test]
    fn library_scan_full_sets_the_flag() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]);
        let block = accepted_with_library(&api, &library, "library scan --full");

        assert!(text_of_block(&block).contains("全量扫描"));
        assert_eq!(library.calls(), vec![LibraryCall::Scan(true)]);
    }

    #[test]
    fn library_scan_while_running_returns_busy() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_roots(&["C:\\Music"]).busy();
        let block = done_with_library(&api, &library, "library scan");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_BUSY");
    }

    #[test]
    fn library_scan_without_roots_returns_arg_error() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "library scan");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
    }

    #[test]
    fn library_list_defaults_to_twenty_and_renders_rows() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_list_tracks(vec![
            track(1, "Kid A", "Radiohead", "/music/a.flac", 90_000),
            track(2, "Untitled", "", "/music/b.flac", 0),
        ]);
        let block = done_with_library(&api, &library, "library list");

        assert_eq!(library.calls(), vec![LibraryCall::ListRecent(20)]);
        let (columns, rows) = table_of_block(&block);
        assert_eq!(columns, vec!["序号", "标题", "艺人", "时长"]);
        assert_eq!(rows[0], vec!["1", "Kid A", "Radiohead", "01:30"]);
        assert_eq!(rows[1][3], "--:--");
    }

    #[test]
    fn library_list_honors_limit_and_rejects_bad_values() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, "library list --limit=5");
        assert_eq!(library.calls(), vec![LibraryCall::ListRecent(5)]);

        let block = done_with_library(&api, &library, "library list --limit=abc");
        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");

        let block = done_with_library(&api, &library, "library list --limit=0");
        assert_eq!(block.status, BlockStatus::Error);
    }

    #[test]
    fn library_stat_renders_counts_scan_time_and_cover_cache() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new()
            .with_stats(Stats {
                tracks: 3,
                albums: 1,
                artists: 2,
                total_duration_ms: 3_723_000,
                last_scan_at: 0,
            })
            .with_covers(3, 1_572_864);
        let block = done_with_library(&api, &library, "library stat");
        let text = text_of_block(&block);

        assert!(text.contains("曲目 · 3"), "{text}");
        assert!(text.contains("专辑 · 1"), "{text}");
        assert!(text.contains("艺人 · 2"), "{text}");
        assert!(text.contains("总时长 · 1 小时 2 分"), "{text}");
        assert!(text.contains("最近扫描 · 从未扫描"), "{text}");
        // 3 个文件 / 1.5 MB：字节数按 1024² 换算，保留一位小数。
        assert!(text.contains("封面缓存 · 3 个文件 / 1.5 MB"), "{text}");
        assert_eq!(
            library.calls(),
            vec![LibraryCall::Stats, LibraryCall::Covers]
        );
    }

    #[test]
    fn library_stat_reports_zero_cover_cache() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "library stat");
        let text = text_of_block(&block);

        assert!(text.contains("封面缓存 · 0 个文件 / 0.0 MB"), "{text}");
    }

    #[test]
    fn search_renders_rows_and_remembers_results() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "Kid A", "Radiohead", "/music/a.flac", 90_000),
            track(2, "Kid B", "Radiohead", "/music/b.flac", 0),
        ]);
        let block = done_with_library(&api, &library, "search kid");

        assert_eq!(
            library.calls(),
            vec![LibraryCall::Search("kid".to_owned(), 21)]
        );
        assert_eq!(library.session().len(), 2);
        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "1");
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("共 2 条"))
        );
    }

    #[test]
    fn search_artist_only_uses_artist_as_keyword() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "晴天",
            "周杰伦",
            "/music/qingtian.flac",
            200_000,
        )]);
        let block = done_with_library(&api, &library, "search --artist 周杰伦 --local");

        assert_eq!(
            library.calls(),
            vec![LibraryCall::Search("周杰伦".to_owned(), 50)]
        );
        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][2], "周杰伦");
    }

    #[test]
    fn search_artist_filters_local_results() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "A", "周杰伦", "/music/a.flac", 1_000),
            track(2, "B", "别人", "/music/b.flac", 1_000),
        ]);
        let block = done_with_library(&api, &library, "search 歌 --artist 周杰伦 --local");

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "A");
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("共 1 条"))
        );
    }

    #[test]
    fn search_artist_filter_empty_explains_before_and_removed() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "A", "甲", "/music/a.flac", 1_000),
            track(2, "B", "乙", "/music/b.flac", 1_000),
            track(3, "C", "丙", "/music/c.flac", 1_000),
        ]);
        let block = done_with_library(&api, &library, "search 歌 --artist 周杰伦 --local");

        assert_eq!(block.status, BlockStatus::Done);
        assert!(block.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. }
                if text.contains("过滤前有 3 条，被 --artist 过滤掉了 3 条")
        )));
        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            })
        );
    }

    #[test]
    fn search_local_and_source_are_mutually_exclusive() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "search 晴天 --local --source netease");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(error_fields(&block).1.contains("互斥"));
        assert!(library.calls().is_empty());
    }

    #[test]
    fn search_rejects_invalid_source() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "search 晴天 --source youtube");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(error_fields(&block).1.contains("未知音源"));
    }

    #[test]
    fn search_rejects_out_of_range_limit() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        for input in ["search 晴天 --limit 0", "search 晴天 --limit 101"] {
            let block = done_with_library(&api, &library, input);
            assert_eq!(block.status, BlockStatus::Error, "{input}");
            assert!(error_fields(&block).1.contains("limit"), "{input}");
        }
    }

    #[test]
    fn search_limit_applies_to_local_results() {
        let api = FakeApi::new();
        let tracks: Vec<Track> = (0..10)
            .map(|index| {
                track(
                    index,
                    &format!("Track {index}"),
                    "Artist",
                    &format!("/music/{index}.flac"),
                    1_000,
                )
            })
            .collect();
        let library = FakeLibraryApi::new().with_search_tracks(tracks);
        let block = done_with_library(&api, &library, "search track --local --limit 5");

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 5);
        assert_eq!(
            library.calls(),
            vec![LibraryCall::Search("track".to_owned(), 6)]
        );
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("--limit 5"))
        );
    }

    #[test]
    fn search_without_hits_explains_itself() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "search nothing");

        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("没有找到"));
        assert!(library.session().is_empty());
    }

    #[test]
    fn search_truncates_results_at_default_twenty() {
        let api = FakeApi::new();
        let tracks: Vec<Track> = (0..21)
            .map(|index| {
                track(
                    index,
                    &format!("Track {index}"),
                    "Artist",
                    &format!("/music/{index}.flac"),
                    1_000,
                )
            })
            .collect();
        let library = FakeLibraryApi::new().with_search_tracks(tracks);
        let block = done_with_library(&api, &library, "search track");

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 20);
        assert_eq!(library.session().len(), 20);
        assert!(text_of_block(&block).contains("只显示前 20 条"));
    }

    #[test]
    fn local_search_sets_active_listing_with_row_count() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "First", "Artist", "/music/first.flac", 1_000),
            track(2, "Second", "Artist", "/music/second.flac", 1_000),
        ]);
        done_with_library(&api, &library, "search artist --local");

        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 2,
            })
        );
    }

    #[test]
    fn empty_local_search_clears_old_context_to_zero() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: 1,
        });

        let empty = done_with_library(&api, &library, "search nothing");
        assert_eq!(empty.status, BlockStatus::Done);
        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            })
        );

        let play = done_with_library(&api, &library, "play 1");
        assert_eq!(play.status, BlockStatus::Error);
        assert!(error_fields(&play).1.contains("搜索结果只有 0 条"));
    }

    #[test]
    fn search_requires_a_keyword() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "search");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(library.calls().is_empty());
    }

    #[test]
    fn queue_list_replaces_search_as_active_listing() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac"], None));
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Search Song",
            "Artist",
            "/music/search.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search song");
        done_with_library(&api, &library, "queue list");
        let block = done_with_library(&api, &library, "play 1");

        assert_eq!(
            api.calls(),
            vec![Call::QueueList, Call::QueueList, Call::QueueJump(0)]
        );
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：播放队列"))
        );
    }

    #[test]
    fn play_uses_search_results_after_search() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "First", "Artist", "/music/first.flac", 1_000),
            track(2, "Second", "Artist", "/music/second.flac", 1_000),
        ]);
        done_with_library(&api, &library, "search artist");
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(
            api.calls(),
            vec![Call::Play(Some("/music/second.flac".to_owned()))]
        );
        assert!(text_of_block(&block).contains("搜索结果第 2 首"));
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：搜索结果"))
        );
    }

    #[test]
    fn play_number_without_any_listing_fails_with_teaching_hint() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(error_fields(&block).1.contains("没有可用的序号列表"));
        assert!(
            error_fields(&block)
                .2
                .is_some_and(|hint| hint.contains("search / queue list"))
        );
        assert!(api.calls().is_empty(), "不能偷偷回退到队列");
    }

    #[test]
    fn play_search_index_out_of_range_fails() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Only",
            "Artist",
            "/music/only.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search only");
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("只有 1 条"));
        assert!(api.calls().is_empty());
    }

    #[test]
    fn library_list_sets_active_listing_for_play_number() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_list_tracks(vec![
            track(1, "First", "Artist", "/music/first.flac", 1_000),
            track(2, "Second", "Artist", "/music/second.flac", 1_000),
        ]);
        done_with_library(&api, &library, "library list");
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(
            api.calls(),
            vec![Call::Play(Some("/music/second.flac".to_owned()))]
        );
        assert!(text_of_block(&block).contains("媒体库第 2 首"));
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：媒体库"))
        );
    }

    #[test]
    fn search_replaces_queue_as_active_listing() {
        let api = FakeApi::new().with_queue(queue_view(&["queued.flac"], None));
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Search",
            "Artist",
            "/music/search.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "queue list");
        done_with_library(&api, &library, "search song");
        let block = done_with_library(&api, &library, "play 1");

        assert_eq!(
            api.calls(),
            vec![
                Call::QueueList,
                Call::Play(Some("/music/search.flac".to_owned()))
            ]
        );
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：搜索结果"))
        );
    }

    #[test]
    fn empty_queue_listing_gives_zero_count_error() {
        let api = FakeApi::new().with_queue(queue_view(&[], None));
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, "queue list");
        let block = done_with_library(&api, &library, "play 1");

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("播放队列只有 0 首"));
        assert!(!api.calls().contains(&Call::QueueJump(0)));
    }

    #[test]
    fn queue_listing_out_of_range_fails_without_jump() {
        let api = FakeApi::new().with_queue(queue_view(&["only.flac"], None));
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, "queue list");
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("播放队列只有 1 首"));
        assert!(!api.calls().contains(&Call::QueueJump(1)));
    }

    fn playlist_with_items(library: &FakeLibraryApi, name: &str) -> i64 {
        library.playlist_create(name).expect("create playlist")
    }

    #[test]
    fn playlist_create_list_and_show_set_active_playlist() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let created = done_with_library(&api, &library, r#"playlist create "周杰伦 情歌""#);
        assert_eq!(created.status, BlockStatus::Done);

        let list = done_with_library(&api, &library, "playlist list");
        assert!(
            table_of_block(&list)
                .1
                .iter()
                .any(|row| row[0] == "周杰伦 情歌")
        );

        let show = done_with_library(&api, &library, r#"playlist show "周杰伦 情歌""#);
        assert_eq!(show.status, BlockStatus::Done, "{}", text_of_block(&show));
        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Playlist("周杰伦 情歌".to_owned()),
                len: 0,
            })
        );
    }

    #[test]
    fn playlist_add_from_search_and_show_lists_remote_metadata() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        library.remember_hits(vec![SearchHit::Online(up_track())]);
        library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: 1,
        });
        done_with_library(&api, &library, r#"playlist create "远程""#);
        let add = done_with_library(&api, &library, r#"playlist add "远程" 1"#);
        assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));

        let show = done_with_library(&api, &library, r#"playlist show "远程""#);
        let (_, rows) = table_of_block(&show);
        assert_eq!(rows[0][1], "最长的电影");
        assert_eq!(rows[0][2], "明前奶绿");
        assert_eq!(rows[0][4], "B站");
    }

    #[test]
    fn playlist_add_without_active_listing_teaches_user() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, r#"playlist create "A""#);
        let block = done_with_library(&api, &library, r#"playlist add "A" 1"#);
        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("当前没有可用的列表"));
        assert!(error_fields(&block).2.unwrap().contains("search"));
    }

    #[test]
    fn playlist_rename_and_delete_commands() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, r#"playlist create "旧""#);
        let rename = done_with_library(&api, &library, r#"playlist rename "旧" "新""#);
        assert_eq!(
            rename.status,
            BlockStatus::Done,
            "{}",
            text_of_block(&rename)
        );
        let show = done_with_library(&api, &library, r#"playlist show "新""#);
        assert_eq!(show.status, BlockStatus::Done);
        let delete = done_with_library(&api, &library, r#"playlist delete "新""#);
        assert_eq!(delete.status, BlockStatus::Done);
        assert!(library.playlist_list().expect("list").is_empty());
    }

    #[test]
    fn playlist_add_from_library_listing() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_list_tracks(vec![track(
            1,
            "库里的歌",
            "艺人",
            "/music/lib.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "library list");
        done_with_library(&api, &library, r#"playlist create "A""#);
        let add = done_with_library(&api, &library, r#"playlist add "A" 1"#);
        assert_eq!(add.status, BlockStatus::Done, "{}", text_of_block(&add));
        let show = done_with_library(&api, &library, r#"playlist show "A""#);
        assert_eq!(table_of_block(&show).1[0][1], "! 库里的歌");
    }

    #[test]
    fn playlist_add_out_of_range_is_human_error() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "一",
            "艺人",
            "/music/one.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search 歌");
        done_with_library(&api, &library, r#"playlist create "A""#);
        let add = done_with_library(&api, &library, r#"playlist add "A" 2"#);
        assert_eq!(add.status, BlockStatus::Error);
        assert!(error_fields(&add).1.contains("搜索结果只有 1 条"));
    }

    #[test]
    fn playlist_play_defaults_to_first_item() {
        let dir = TestDir::new("playlist-default-play");
        let path = dir.path().join("first.flac");
        fs::write(&path, b"fake").expect("audio");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let id = playlist_with_items(&library, "A");
        library
            .playlist_add(
                id,
                &NewPlaylistItem {
                    kind: "local".to_owned(),
                    path: path.to_string_lossy().into_owned(),
                    source: String::new(),
                    source_id: String::new(),
                    title: "第一".to_owned(),
                    artist: String::new(),
                    album: String::new(),
                    duration_ms: 1_000,
                },
            )
            .expect("add");
        let block = done_with_library(&api, &library, r#"playlist play "A""#);
        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("第 1 首"));
    }

    #[test]
    fn playlist_remove_keeps_one_based_positions_contiguous() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let id = playlist_with_items(&library, "A");
        for title in ["一", "二", "三"] {
            library
                .playlist_add(
                    id,
                    &NewPlaylistItem {
                        kind: "local".to_owned(),
                        path: format!("/music/{title}.flac"),
                        source: String::new(),
                        source_id: String::new(),
                        title: title.to_owned(),
                        artist: String::new(),
                        album: String::new(),
                        duration_ms: 1_000,
                    },
                )
                .expect("add");
        }
        let removed = done_with_library(&api, &library, r#"playlist remove "A" 1"#);
        assert_eq!(removed.status, BlockStatus::Done);
        let show = done_with_library(&api, &library, r#"playlist show "A""#);
        let (_, rows) = table_of_block(&show);
        assert_eq!(rows[0][0], "1");
        assert_eq!(rows[0][1], "! 二");
        assert_eq!(rows[1][0], "2");
        assert_eq!(rows[1][1], "! 三");
    }

    #[test]
    fn playlist_play_local_missing_file_is_human_error() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let id = playlist_with_items(&library, "A");
        library
            .playlist_add(
                id,
                &NewPlaylistItem {
                    kind: "local".to_owned(),
                    path: "/does/not/exist.flac".to_owned(),
                    source: String::new(),
                    source_id: String::new(),
                    title: "不存在".to_owned(),
                    artist: String::new(),
                    album: String::new(),
                    duration_ms: 1_000,
                },
            )
            .expect("add");
        let block = done_with_library(&api, &library, r#"playlist play "A""#);
        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("文件不存在"));
    }

    #[test]
    fn playlist_play_local_and_play_number_after_show() {
        let dir = TestDir::new("playlist-local-play");
        let path = dir.path().join("song.flac");
        fs::write(&path, b"fake").expect("audio");
        let path_text = path.to_string_lossy().into_owned();
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let id = playlist_with_items(&library, "A");
        library
            .playlist_add(
                id,
                &NewPlaylistItem {
                    kind: "local".to_owned(),
                    path: path_text.clone(),
                    source: String::new(),
                    source_id: String::new(),
                    title: "本地".to_owned(),
                    artist: "艺人".to_owned(),
                    album: String::new(),
                    duration_ms: 1_000,
                },
            )
            .expect("add");
        let direct = done_with_library(&api, &library, r#"playlist play "A" 1"#);
        assert_eq!(direct.status, BlockStatus::Done);
        assert!(text_of_block(&direct).contains("第 1 首"));
        done_with_library(&api, &library, r#"playlist show "A""#);
        let by_number = done_with_library(&api, &library, "play 1");
        assert_eq!(by_number.status, BlockStatus::Done);
        assert!(
            api.calls()
                .iter()
                .filter(|call| matches!(call, Call::Play(Some(path)) if path == &path_text))
                .count()
                >= 2
        );
    }

    #[test]
    fn playlist_play_remote_uses_prepare() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        library.remember_hits(vec![SearchHit::Online(up_track())]);
        library.set_active_listing(ActiveListing {
            source: ListingSource::Search,
            len: 1,
        });
        done_with_library(&api, &library, r#"playlist create "远程""#);
        done_with_library(&api, &library, r#"playlist add "远程" 1"#);
        let cached = PathBuf::from("/cache/remote.m4a");
        let online = FakeQueueOnline {
            path: cached.clone(),
        };
        let block = done_with_playlist_online(&api, &library, &online, r#"playlist play "远程""#);
        assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
        assert!(api.calls().iter().any(
            |call| matches!(call, Call::Play(Some(path)) if path == &cached.to_string_lossy())
        ));
        assert!(text_of_block(&block).contains("来源：B站"));
    }

    #[test]
    fn playlist_play_out_of_range_is_human_error() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        playlist_with_items(&library, "A");
        let block = done_with_library(&api, &library, r#"playlist play "A" 1"#);
        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("只有 0 首"));
    }

    #[test]
    fn playlist_unknown_name_and_duplicate_name_errors() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        done_with_library(&api, &library, r#"playlist create "A""#);
        let duplicate = done_with_library(&api, &library, r#"playlist create "a""#);
        assert_eq!(duplicate.status, BlockStatus::Error);
        assert!(error_fields(&duplicate).1.contains("已存在"));
        let missing = done_with_library(&api, &library, r#"playlist show "没有""#);
        assert_eq!(missing.status, BlockStatus::Error);
        assert!(error_fields(&missing).1.contains("不存在"));
    }

    #[test]
    fn library_alias_runs_the_same_subcommand() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "lib stat");

        assert_eq!(block.status, BlockStatus::Done);
        assert_eq!(
            library.calls(),
            vec![LibraryCall::Stats, LibraryCall::Covers]
        );
    }

    // ---- 扫描进度（节流 + 块状态） ----

    fn event_observer(now: Arc<AtomicU64>, emitted: Arc<Mutex<Vec<Block>>>) -> EventObserver {
        let emit: Arc<EmitFn> = Arc::new(move |block: &Block| {
            emitted.lock().expect("emitted mutex").push(block.clone());
        });
        let clock: Arc<ClockFn> = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        EventObserver::with_port(
            emit,
            clock,
            "scan-block".to_owned(),
            "library scan".to_owned(),
            1_000,
        )
    }

    fn progress_current(block: &Block) -> u64 {
        block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Progress { current, .. } => Some(*current),
                _ => None,
            })
            .expect("progress output")
    }

    #[test]
    fn progress_is_throttled_by_time_not_by_file_count() {
        let now = Arc::new(AtomicU64::new(1_000));
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

        for (at, scanned) in [
            (1_000, 1),
            (1_100, 2),
            (1_400, 3),
            (1_500, 4),
            (1_900, 5),
            (2_000, 6),
        ] {
            now.store(at, Ordering::SeqCst);
            observer.on_progress(scanned, 10, Path::new("song.flac"));
        }

        let blocks = emitted.lock().expect("emitted mutex");
        let currents: Vec<u64> = blocks.iter().map(progress_current).collect();
        // 1 Hz 下限：只在 0ms、500ms、1000ms 三个时间点放行。
        assert_eq!(currents, vec![1, 4, 6]);
        assert!(blocks.iter().all(|block| block.id == "scan-block"));
        assert!(
            blocks
                .iter()
                .all(|block| block.status == BlockStatus::Running)
        );
    }

    #[test]
    fn scan_finished_block_summarizes_the_report() {
        let now = Arc::new(AtomicU64::new(1_000));
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

        now.store(1_750, Ordering::SeqCst);
        observer.on_finished(&ScanReport {
            scanned: 10,
            indexed: 8,
            skipped: 1,
            failed: 1,
            missing: 2,
            cancelled: false,
        });

        let blocks = emitted.lock().expect("emitted mutex");
        let block = blocks.last().expect("finished block");
        assert_eq!(block.id, "scan-block");
        assert_eq!(block.status, BlockStatus::Done);
        assert_eq!(block.duration_ms, Some(750));
        let text = block
            .outputs
            .iter()
            .filter_map(|output| match output {
                Output::Text { spans, .. } => Some(
                    spans
                        .iter()
                        .map(|span| span.text.as_str())
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("扫描完成"), "{text}");
        assert!(text.contains("扫描 10"), "{text}");
        assert!(text.contains("入库 8"), "{text}");
        assert!(text.contains("缺失 2"), "{text}");
    }

    #[test]
    fn cancelled_scan_finishes_as_cancelled() {
        let now = Arc::new(AtomicU64::new(1_000));
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

        observer.on_finished(&ScanReport {
            scanned: 3,
            indexed: 1,
            cancelled: true,
            ..ScanReport::default()
        });

        let blocks = emitted.lock().expect("emitted mutex");
        let block = blocks.last().expect("cancelled block");
        assert_eq!(block.status, BlockStatus::Cancelled);
    }

    #[test]
    fn scan_failure_block_carries_the_error_code() {
        let now = Arc::new(AtomicU64::new(1_000));
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let observer = event_observer(Arc::clone(&now), Arc::clone(&emitted));

        observer.on_failed(&CmdError::new("E_LIB_IO", "无法读取目录"));

        let blocks = emitted.lock().expect("emitted mutex");
        let block = blocks.last().expect("failure block");
        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(block).0, "E_LIB_IO");
    }

    #[test]
    fn scan_started_block_is_running_with_the_expected_id() {
        let block = scan_started_block("blk-scan", "library scan --full", true);

        assert_eq!(block.id, "blk-scan");
        assert_eq!(block.input, "library scan --full");
        assert_eq!(block.status, BlockStatus::Running);
        assert!(text_of_block(&block).contains("扫描已开始"));
        assert!(text_of_block(&block).contains("全量扫描"));
    }

    #[test]
    fn cmd_error_payload_omits_a_missing_hint() {
        let reply = CmdErrorPayload::from(CmdError::new("E_BUSY", "已有扫描在进行中"));

        match tauri::ipc::IpcResponse::body(reply).expect("reply body") {
            tauri::ipc::InvokeResponseBody::Json(json) => {
                assert_eq!(json, r#"{"code":"E_BUSY","message":"已有扫描在进行中"}"#);
            }
            tauri::ipc::InvokeResponseBody::Raw(_) => panic!("expected a JSON body"),
        }
    }

    fn table_of_block(block: &Block) -> (Vec<&str>, Vec<Vec<&str>>) {
        block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { columns, rows, .. } => Some((
                    columns.iter().map(String::as_str).collect(),
                    rows.iter()
                        .map(|row| row.iter().map(String::as_str).collect())
                        .collect(),
                )),
                _ => None,
            })
            .expect("table output")
    }

    // ---- IPC 与事件 ----

    #[test]
    fn terminal_event_channel_uses_a_colon_not_a_dot() {
        assert_eq!(EVENT_TERMINAL_BLOCK, "terminal:block");
        assert!(!EVENT_TERMINAL_BLOCK.contains('.'));
    }

    #[test]
    fn cmd_exec_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_exec!(), "cmd.exec");
    }

    // ---- 补全（T-0020） ----

    #[test]
    fn cmd_complete_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_complete!(), "cmd.complete");
    }

    #[test]
    fn complete_maps_dynamic_queue_indices_and_polls_snapshot_once() {
        let api = FakeApi::new().with_state(PlaybackState {
            queue_length: Some(3),
            ..PlaybackState::default()
        });

        let completion = complete_for(&api, "play ", 5);

        assert_eq!(api.calls(), vec![Call::Snapshot]);
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
        let texts: Vec<&str> = completion
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(texts, ["1", "2", "3"]);
    }

    #[test]
    fn complete_degrades_to_empty_dynamic_candidates_when_snapshot_fails() {
        let api = FakeApi::new().with_playback_error(CmdError::new("E_BUSY", "播放器线程不可用"));

        let completion = complete_for(&api, "play ", 5);

        // 降级：不报错、不阻塞，只返回空列表；区间仍然给出。
        assert!(completion.items.is_empty());
        assert_eq!((completion.replace_start, completion.replace_end), (5, 5));
    }

    #[test]
    fn complete_with_unknown_queue_length_returns_no_dynamic_candidates() {
        // FakeApi 默认 `queue_length: None`：快照成功但长度未知，同样降级为空。
        let api = FakeApi::new();

        let completion = complete_for(&api, "play ", 5);

        assert!(completion.items.is_empty());
    }

    #[test]
    fn complete_returns_static_candidates_without_polling_the_player() {
        let api = FakeApi::new();

        let completion = complete_for(&api, "pl", 2);

        assert_eq!(api.calls(), Vec::new());
        let play = completion
            .items
            .iter()
            .find(|item| item.text == "play")
            .expect("play candidate");
        assert_eq!(play.kind, np_core::command::CompletionKind::Command);
    }

    #[test]
    fn covers_dir_is_the_database_sibling() {
        // 与 `Database::open` 的默认约定一致；asset scope 的 $APPDATA/covers
        // 只有在 db 位于 app_data_dir 根上时才对齐。
        let handle = LibraryHandle::new(PathBuf::from("/data/nowhere/library.db"));

        assert_eq!(handle.covers_dir(), PathBuf::from("/data/nowhere/covers"));
    }

    #[test]
    fn reply_body_is_camel_case_json() {
        let reply = CmdExecReply {
            block_id: "block-1".to_owned(),
            ok: true,
        };

        match tauri::ipc::IpcResponse::body(reply).expect("reply body") {
            tauri::ipc::InvokeResponseBody::Json(json) => {
                assert_eq!(json, r#"{"blockId":"block-1","ok":true}"#);
            }
            tauri::ipc::InvokeResponseBody::Raw(_) => panic!("expected a JSON body"),
        }
    }

    // ---- 在线音源：注册表 / 缓存 / 去重 / 错误映射（T-0038） ----

    struct TestDir {
        path: PathBuf,
    }

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    impl TestDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "np-command-{label}-{}-{}",
                std::process::id(),
                TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Clone, Copy)]
    enum ResolvePlan {
        Expired403,
        Expired410,
        Network,
        Copyright,
    }

    impl ResolvePlan {
        fn into_error(self) -> SourceError {
            match self {
                Self::Expired403 => SourceError::Http(403),
                Self::Expired410 => SourceError::Http(410),
                Self::Network => SourceError::Network("连接失败".to_string()),
                Self::Copyright => SourceError::Unavailable(
                    "该曲目在网易云不可播放（可能版权受限或需要会员）".to_string(),
                ),
            }
        }
    }

    struct FakeOnlineSource {
        id: SourceId,
        resolves: AtomicUsize,
        searches: AtomicUsize,
        plans: Mutex<Vec<ResolvePlan>>,
        stream: ResolvedStream,
        delay: Duration,
    }

    impl FakeOnlineSource {
        fn new(id: SourceId, plans: Vec<ResolvePlan>) -> Arc<Self> {
            Arc::new(Self {
                id,
                resolves: AtomicUsize::new(0),
                searches: AtomicUsize::new(0),
                plans: Mutex::new(plans),
                stream: ResolvedStream {
                    url: "https://example.invalid/audio".to_string(),
                    mime: "audio/mpeg".to_string(),
                    expires_at: None,
                    headers: Vec::new(),
                },
                delay: Duration::ZERO,
            })
        }

        fn with_delay(self: &Arc<Self>, delay: Duration) -> Arc<Self> {
            let mut cloned = FakeOnlineSource {
                id: self.id,
                resolves: AtomicUsize::new(0),
                searches: AtomicUsize::new(0),
                plans: Mutex::new(Vec::new()),
                stream: self.stream.clone(),
                delay,
            };
            cloned.plans = Mutex::new(
                self.plans
                    .lock()
                    .expect("plans mutex")
                    .iter()
                    .copied()
                    .collect(),
            );
            Arc::new(cloned)
        }

        fn resolves(&self) -> usize {
            self.resolves.load(Ordering::SeqCst)
        }

        fn searches(&self) -> usize {
            self.searches.load(Ordering::SeqCst)
        }
    }

    impl AudioSource for FakeOnlineSource {
        fn id(&self) -> SourceId {
            self.id
        }

        fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SourceTrack>, SourceError> {
            self.searches.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn resolve(&self, _track: &SourceTrack) -> Result<ResolvedStream, SourceError> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let mut plans = self.plans.lock().expect("plans mutex");
            if plans.is_empty() {
                return Ok(self.stream.clone());
            }
            Err(plans.remove(0).into_error())
        }
    }

    struct FakeDownloader {
        calls: AtomicUsize,
        bytes: Vec<u8>,
        fail: bool,
        progress: Vec<u64>,
    }

    impl FakeDownloader {
        fn new(bytes: &[u8], fail: bool, progress: Vec<u64>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                bytes: bytes.to_vec(),
                fail,
                progress,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Downloader for FakeDownloader {
        fn download(
            &self,
            stream: &ResolvedStream,
            dest: &Path,
        ) -> Result<DownloadOutcome, SourceError> {
            self.download_with_progress(stream, dest, &mut |_| {})
        }

        fn download_with_progress(
            &self,
            _stream: &ResolvedStream,
            dest: &Path,
            progress: &mut dyn FnMut(u64),
        ) -> Result<DownloadOutcome, SourceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(SourceError::Download("模拟中断".to_string()));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| SourceError::Download(error.to_string()))?;
            }
            fs::write(dest, &self.bytes)
                .map_err(|error| SourceError::Download(error.to_string()))?;
            for written in &self.progress {
                progress(*written);
            }
            Ok(DownloadOutcome::Downloaded {
                bytes: self.bytes.len() as u64,
            })
        }
    }

    fn online_track(source: SourceId, id: &str, title: &str) -> SourceTrack {
        SourceTrack {
            source,
            source_id: id.to_string(),
            title: title.to_string(),
            artist: "在线艺人".to_string(),
            duration_ms: 200_000,
            album: None,
            cover_url: None,
        }
    }

    type CoverCalls = Vec<(String, Vec<(String, String)>)>;

    struct FakeCoverTransport {
        body: Vec<u8>,
        calls: Mutex<CoverCalls>,
    }

    impl FakeCoverTransport {
        fn new(body: &[u8]) -> Self {
            Self {
                body: body.to_vec(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> CoverCalls {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl Transport for FakeCoverTransport {
        fn get(&self, _url: &str) -> Result<Vec<u8>, SourceError> {
            Ok(self.body.clone())
        }

        fn get_to(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            sink: &mut dyn std::io::Write,
        ) -> Result<u64, SourceError> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((
                    url.to_string(),
                    headers
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                        .collect(),
                ));
            sink.write_all(&self.body)
                .map_err(|error| SourceError::Download(error.to_string()))?;
            Ok(self.body.len() as u64)
        }
    }

    struct FakeCoverStore {
        key: Option<String>,
        calls: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeCoverStore {
        fn new(key: Option<&str>) -> Self {
            Self {
                key: key.map(str::to_owned),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<u8>> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl CoverStore for FakeCoverStore {
        fn store(&self, _dir: &Path, bytes: &[u8]) -> Option<String> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(bytes.to_vec());
            self.key.clone()
        }
    }

    fn online_service(
        source: Arc<FakeOnlineSource>,
        downloader: Arc<FakeDownloader>,
    ) -> (TestDir, OnlineService) {
        let dir = TestDir::new("online");
        let cache = StreamCache::with_limits(
            dir.path(),
            Arc::clone(&downloader) as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
        let transport: Arc<dyn Transport> = Arc::new(FakeCoverTransport::new(&[]));
        let cover_store: Arc<dyn CoverStore> = Arc::new(FakeCoverStore::new(None));
        let covers_dir = dir.path().join("covers");
        (
            dir,
            OnlineService::from_parts(registry, cache, transport, covers_dir, cover_store),
        )
    }

    #[test]
    fn run_online_search_default_caps_local_at_ten() {
        let local_tracks: Vec<Track> = (0..20)
            .map(|index| {
                track(
                    index,
                    &format!("Local {index}"),
                    "Artist",
                    &format!("/music/{index}.flac"),
                    1_000,
                )
            })
            .collect();
        let library = FakeLibraryApi::new().with_search_tracks(local_tracks);
        let online_tracks: Vec<SourceTrack> = (0..20)
            .map(|index| online_track(SourceId::Bilibili, &format!("BV{index}"), "Bili"))
            .collect();
        let online = FakeOnline {
            tracks: online_tracks,
            all_failed: false,
        };
        let options = SearchOptions {
            keyword: "x".to_owned(),
            artist: None,
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_search_with_options(
            &options,
            &online,
            &library,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        let (_, rows) = table_of_block(done);
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[0][1], "本地");
        assert_eq!(rows[9][1], "本地");
        assert_eq!(rows[10][1], "B站");
        assert_eq!(rows[19][1], "B站");
    }

    #[test]
    fn run_online_search_source_excludes_local_and_other_sources() {
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Local",
            "Artist",
            "/music/local.flac",
            1_000,
        )]);
        let online = FakeOnline {
            tracks: vec![
                online_track(SourceId::Bilibili, "BV1xx", "Bili"),
                online_track(SourceId::Netease, "10001", "Net"),
            ],
            all_failed: false,
        };
        let options = SearchOptions {
            keyword: "x".to_owned(),
            artist: None,
            source: Some(SourceId::Bilibili),
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_search_with_options(
            &options,
            &online,
            &library,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        let (_, rows) = table_of_block(done);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "B站");
        assert!(library.calls().is_empty(), "source 搜索不应查本地");
    }

    #[test]
    fn run_online_search_artist_empty_explains_filter() {
        let library = FakeLibraryApi::new();
        let online = FakeOnline {
            tracks: vec![online_track(SourceId::Bilibili, "BV1xx", "Bili")],
            all_failed: false,
        };
        let options = SearchOptions {
            keyword: "歌".to_owned(),
            artist: Some("周杰伦".to_owned()),
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_search_with_options(
            &options,
            &online,
            &library,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        assert!(done.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. }
                if text.contains("过滤前有 1 条，被 --artist 过滤掉了 1 条")
        )));
        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            })
        );
    }

    #[test]
    fn run_online_search_default_footer_names_per_source_ten() {
        let library = FakeLibraryApi::new();
        let online = FakeOnline {
            tracks: vec![online_track(SourceId::Netease, "10001", "Net")],
            all_failed: false,
        };
        let options = SearchOptions {
            keyword: "歌".to_owned(),
            artist: None,
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_search_with_options(
            &options,
            &online,
            &library,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        assert!(
            done.footer
                .as_deref()
                .is_some_and(|footer| footer.contains("每源 10，--limit 可调"))
        );
    }

    fn run_online_capture(
        options: &SearchOptions,
        online: &RecordingOnline,
        library: &FakeLibraryApi,
    ) -> Block {
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        run_online_search_with_options(
            options,
            online,
            library,
            &emit,
            &running_template(),
            &|| 2_000,
        );
        emitted
            .lock()
            .expect("emitted")
            .last()
            .expect("done block")
            .clone()
    }

    fn artist_options(keyword: &str, artist: &str, limit: usize) -> SearchOptions {
        SearchOptions {
            keyword: keyword.to_owned(),
            artist: Some(artist.to_owned()),
            source: None,
            limit,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        }
    }

    #[test]
    fn artist_query_is_artist_first_and_pool_expands() {
        let library = FakeLibraryApi::new();
        let mut author = online_track(SourceId::Bilibili, "BV1xx", "晴天");
        author.artist = "周杰伦".to_owned();
        let online = RecordingOnline::new(vec![author]);
        let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

        let block = run_online_capture(&options, &online, &library);

        assert_eq!(online.calls(), vec![("周杰伦 晴天".to_owned(), 50, None)]);
        assert_eq!(table_of_block(&block).1.len(), 1);
    }

    #[test]
    fn explicit_limit_scales_artist_pool_by_three() {
        let library = FakeLibraryApi::new();
        let online = RecordingOnline::new(Vec::new());
        let options = artist_options("晴天", "周杰伦", 5);

        run_online_capture(&options, &online, &library);

        assert_eq!(online.calls()[0].1, 15);
    }

    #[test]
    fn author_match_sorts_before_title_match() {
        let library = FakeLibraryApi::new();
        let mut title_match = online_track(SourceId::Bilibili, "BVtitle", "上传者A");
        title_match.title = "【周杰伦】晴天 翻唱".to_owned();
        title_match.artist = "上传者A".to_owned();
        let mut author_match = online_track(SourceId::Bilibili, "BVauthor", "晴天");
        author_match.artist = "周杰伦".to_owned();
        let online = RecordingOnline::new(vec![title_match, author_match]);
        let options = artist_options("晴天", "周杰伦", DEFAULT_SEARCH_RESULTS);

        let block = run_online_capture(&options, &online, &library);

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][3], "周杰伦");
        assert_eq!(rows[1][3], "上传者A");
    }

    #[test]
    fn bilibili_title_match_is_kept() {
        let library = FakeLibraryApi::new();
        let mut track = online_track(SourceId::Bilibili, "BVtitle", "切片号");
        track.title = "【明前奶绿】不为谁而作的歌".to_owned();
        track.artist = "切片号".to_owned();
        let online = RecordingOnline::new(vec![track]);
        let options = artist_options("不为谁而作的歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

        let block = run_online_capture(&options, &online, &library);

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][3], "切片号");
    }

    #[test]
    fn bilibili_title_must_also_contain_keyword_when_both_given() {
        let library = FakeLibraryApi::new();
        let mut track = online_track(SourceId::Bilibili, "BVother", "切片号");
        track.title = "【明前奶绿】别的歌".to_owned();
        track.artist = "切片号".to_owned();
        let online = RecordingOnline::new(vec![track]);
        let options = artist_options("不为谁而作的歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

        let block = run_online_capture(&options, &online, &library);

        assert!(block.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. }
                if text.contains("过滤前有 1 条，被 --artist 过滤掉了 1 条")
        )));
    }

    #[test]
    fn artist_only_keeps_title_without_keyword_constraint() {
        let library = FakeLibraryApi::new();
        let mut track = online_track(SourceId::Bilibili, "BVother", "切片号");
        track.title = "【明前奶绿】别的歌".to_owned();
        track.artist = "切片号".to_owned();
        let online = RecordingOnline::new(vec![track]);
        let options = SearchOptions {
            keyword: "明前奶绿".to_owned(),
            artist: Some("明前奶绿".to_owned()),
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: false,
            up: None,
            mid: None,
        };

        let block = run_online_capture(&options, &online, &library);

        assert_eq!(table_of_block(&block).1.len(), 1);
    }

    #[test]
    fn netease_title_match_is_not_kept() {
        let library = FakeLibraryApi::new();
        let mut track = online_track(SourceId::Netease, "10001", "别的艺人");
        track.title = "明前奶绿 翻唱".to_owned();
        let online = RecordingOnline::new(vec![track]);
        let options = artist_options("歌", "明前奶绿", DEFAULT_SEARCH_RESULTS);

        let block = run_online_capture(&options, &online, &library);

        assert!(block.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. }
                if text.contains("过滤前有 1 条，被 --artist 过滤掉了 1 条")
        )));
    }

    #[test]
    fn no_artist_keeps_per_source_pool() {
        let library = FakeLibraryApi::new();
        let online = RecordingOnline::new(Vec::new());
        let options = SearchOptions {
            keyword: "晴天".to_owned(),
            artist: None,
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
        };

        run_online_capture(&options, &online, &library);

        assert_eq!(online.calls()[0].1, ONLINE_SEARCH_PER_SOURCE);
    }

    fn up_options(keyword: &str, up: Option<&str>, mid: Option<u64>) -> SearchOptions {
        SearchOptions {
            keyword: keyword.to_owned(),
            artist: None,
            source: None,
            limit: DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: !keyword.is_empty(),
            up: up.map(str::to_owned),
            mid,
        }
    }

    fn up_track() -> SourceTrack {
        let mut track = online_track(SourceId::Bilibili, "BV1rG411n7qp", "最长的电影");
        track.artist = "明前奶绿".to_owned();
        track.duration_ms = 236_000;
        track
    }

    fn run_up_capture(options: &SearchOptions, online: &FakeUpOnline) -> Block {
        let library = FakeLibraryApi::new();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        run_up_search(
            options,
            online,
            &library,
            &emit,
            &running_template(),
            &|| 2_000,
        );
        emitted
            .lock()
            .expect("emitted")
            .last()
            .expect("done")
            .clone()
    }

    #[test]
    fn up_option_conflicts_are_arg_errors() {
        for input in [
            "search --up 明前奶绿 --artist 周杰伦",
            "search --up 明前奶绿 --local",
            "search --up 明前奶绿 --source netease",
            "search --up 明前奶绿 --mid 2132180406",
            "search --mid abc",
            "search --mid 0",
        ] {
            let error = parse(input).expect_err(input);
            assert_eq!(error.code.as_str(), "E_CMD_ARG", "{input}");
        }
    }

    #[test]
    fn up_search_prints_matched_user_and_results() {
        let online = FakeUpOnline::new(
            Ok(UpResolution {
                mid: 2132180406,
                name: Some("明前奶绿".to_owned()),
                fans: 310066,
                same_name: 2,
            }),
            Ok(vec![up_track()]),
        );
        let options = up_options("最长的电影", Some("明前奶绿"), None);

        let block = run_up_capture(&options, &online);

        let text = text_of_block(&block);
        assert!(text.contains("已匹配 UP：明前奶绿（mid=2132180406，粉丝 31.0 万）"));
        assert!(block.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. } if text.contains("另有 1 个同名用户")
        )));
        assert_eq!(table_of_block(&block).1.len(), 1);
        assert!(block.footer.as_deref().unwrap().contains("共 1 条"));
        assert_eq!(online.resolve_calls(), 1);
        assert_eq!(online.archive_calls()[0].0, 2132180406);
    }

    #[test]
    fn up_mid_skips_user_resolution() {
        let online = FakeUpOnline::new(
            Err(CmdError::new("E_SRC_UNAVAILABLE", "不应调用")),
            Ok(vec![up_track()]),
        );
        let options = up_options("", None, Some(2132180406));

        let block = run_up_capture(&options, &online);

        assert_eq!(online.resolve_calls(), 0);
        assert!(text_of_block(&block).contains("已指定 UP：mid=2132180406"));
        assert!(online.archive_calls()[0].1.starts_with("UP 2132180406"));
        assert_eq!(online.archive_calls()[0].3, UP_ARCHIVE_PAGE_SIZE);
    }

    #[test]
    fn up_no_keyword_requests_thirty_and_notes_recent() {
        let online = FakeUpOnline::new(
            Ok(UpResolution {
                mid: 1,
                name: Some("UP".to_owned()),
                fans: 0,
                same_name: 0,
            }),
            Ok(vec![up_track()]),
        );
        let options = up_options("", Some("UP"), None);

        let block = run_up_capture(&options, &online);

        assert_eq!(online.archive_calls()[0].3, 30);
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("只显示最近 30 条"))
        );
    }

    #[test]
    fn up_keyword_uses_requested_limit() {
        let online = FakeUpOnline::new(
            Ok(UpResolution {
                mid: 1,
                name: Some("UP".to_owned()),
                fans: 0,
                same_name: 0,
            }),
            Ok(vec![up_track()]),
        );
        let mut options = up_options("歌", Some("UP"), None);
        options.explicit_limit = true;
        options.limit = 5;

        run_up_capture(&options, &online);

        assert_eq!(online.archive_calls()[0].3, 5);
    }

    #[test]
    fn up_rate_limit_is_human_error() {
        let online = FakeUpOnline::new(
            Ok(UpResolution {
                mid: 1,
                name: Some("UP".to_owned()),
                fans: 0,
                same_name: 0,
            }),
            Err(CmdError::new(
                "E_SRC_UNAVAILABLE",
                "B 站限流，请稍后再试（未自动重试）",
            )),
        );
        let options = up_options("歌", Some("UP"), None);

        let block = run_up_capture(&options, &online);

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("B 站限流"));
        assert!(error_fields(&block).1.contains("未自动重试"));
    }

    #[test]
    fn up_resolution_failure_is_error() {
        let online = FakeUpOnline::new(
            Err(CmdError::new("E_SRC_UNAVAILABLE", "没有找到 UP：nope")),
            Ok(Vec::new()),
        );
        let options = up_options("", Some("nope"), None);

        let block = run_up_capture(&options, &online);

        assert_eq!(block.status, BlockStatus::Error);
        assert!(error_fields(&block).1.contains("没有找到 UP"));
        assert!(online.archive_calls().is_empty());
    }

    #[test]
    fn run_online_search_empty_hits_sets_empty_search_listing() {
        let library = FakeLibraryApi::new();
        let online = FakeOnline {
            tracks: Vec::new(),
            all_failed: false,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        let template = running_template();

        run_online_search(
            "nothing",
            MAX_SEARCH_RESULTS,
            &online,
            &library,
            &emit,
            &template,
            &|| 2_000,
        );

        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 0,
            })
        );
    }

    #[test]
    fn online_prepare_downloads_and_reports_progress() {
        let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, vec![1, 2, 3]));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let track = online_track(SourceId::Netease, "10001", "Song");

        let mut seen = Vec::new();
        let prepared = service
            .prepare(&track, &mut |written| seen.push(written))
            .expect("下载成功");

        assert!(!prepared.cached);
        assert_eq!(prepared.bytes, 3);
        assert_eq!(fs::read(&prepared.path).expect("读取缓存"), b"abc");
        assert_eq!(seen, vec![1, 2, 3]);
        assert_eq!(source.resolves(), 1);
        assert_eq!(downloader.calls(), 1);
    }

    #[test]
    fn online_prepare_hits_cache_without_resolve() {
        let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let track = online_track(SourceId::Netease, "10001", "Song");
        service.prepare(&track, &mut |_| {}).expect("首次下载");

        let prepared = service.prepare(&track, &mut |_| {}).expect("命中缓存");

        assert!(prepared.cached);
        assert_eq!(prepared.bytes, 3);
        assert_eq!(source.resolves(), 1, "命中缓存不应 resolve");
        assert_eq!(downloader.calls(), 1, "命中缓存不应下载");
        assert!(
            service.track_by_path(&prepared.path).is_some(),
            "命中缓存也要保证元数据已登记"
        );
    }

    #[test]
    fn online_prepare_retries_expired_url_once() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, vec![ResolvePlan::Expired403]);
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        let prepared = service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
                &mut |_| {},
            )
            .expect("第二次 resolve 应成功");

        assert!(!prepared.cached);
        assert_eq!(source.resolves(), 2);
    }

    #[test]
    fn online_prepare_gives_up_after_two_expired_urls() {
        let source = FakeOnlineSource::new(
            SourceId::Bilibili,
            vec![ResolvePlan::Expired403, ResolvePlan::Expired410],
        );
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        let error = service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
                &mut |_| {},
            )
            .expect_err("两次过期应失败");

        assert_eq!(error.code, "E_SRC_UNAVAILABLE");
        assert_eq!(error.message, "链接已过期，请重新搜索");
        assert_eq!(source.resolves(), 2);
    }

    #[test]
    fn online_prepare_maps_copyright_to_human_message() {
        let source = FakeOnlineSource::new(SourceId::Netease, vec![ResolvePlan::Copyright]);
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        let error = service
            .prepare(
                &online_track(SourceId::Netease, "10002", "VIP"),
                &mut |_| {},
            )
            .expect_err("版权受限");

        assert_eq!(error.code, "E_SRC_UNAVAILABLE");
        assert!(error.message.contains("网易云不可播放"));
        assert!(error.message.contains("版权受限") || error.message.contains("会员"));
    }

    #[test]
    fn online_prepare_maps_download_failure() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", true, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        let error = service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
                &mut |_| {},
            )
            .expect_err("下载失败");

        assert_eq!(error.code, "E_SRC_DOWNLOAD");
        assert_eq!(error.message, "下载失败（已清理临时文件）");
    }

    #[test]
    fn online_prepare_maps_network_error() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, vec![ResolvePlan::Network]);
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        let error = service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
                &mut |_| {},
            )
            .expect_err("网络错误");

        assert_eq!(error.code, "E_NET_TIMEOUT");
        assert!(error.message.contains("网络超时"));
    }

    #[test]
    fn online_prepare_dedups_concurrent_same_track() {
        let source = FakeOnlineSource::new(SourceId::Netease, Vec::new())
            .with_delay(Duration::from_millis(50));
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let service = Arc::new(service);
        let track = online_track(SourceId::Netease, "10001", "Song");

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let service = Arc::clone(&service);
                let track = track.clone();
                std::thread::spawn(move || service.prepare(&track, &mut |_| {}))
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread").expect("prepare");
        }

        assert_eq!(source.resolves(), 1, "同曲目并发只 resolve 一次");
        assert_eq!(downloader.calls(), 1, "同曲目并发只下载一次");
    }

    #[test]
    fn search_outcome_numbers_hits_across_sources() {
        let local = track(1, "Local", "Artist", "/music/local.flac", 1_000);
        let hits = vec![
            SearchHit::Local(local),
            SearchHit::Online(online_track(SourceId::Bilibili, "BV1xx", "Bili")),
            SearchHit::Online(online_track(SourceId::Netease, "10001", "Net")),
        ];

        let outcome = build_search_outcome("测试", &hits, false);
        let table = outcome
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { rows, .. } => Some(rows.clone()),
                _ => None,
            })
            .expect("table");

        assert_eq!(table.len(), 3);
        assert_eq!(table[0][0], "1");
        assert_eq!(table[0][1], "本地");
        assert_eq!(table[1][0], "2");
        assert_eq!(table[1][1], "B站");
        assert_eq!(table[2][0], "3");
        assert_eq!(table[2][1], "网易云");
        assert!(
            outcome
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("共 3 条"))
        );
    }

    #[test]
    fn search_outcome_all_online_failed_adds_hint() {
        let hits = vec![SearchHit::Local(track(
            1,
            "Local",
            "Artist",
            "/music/local.flac",
            1_000,
        ))];

        let outcome = build_search_outcome("测试", &hits, true);

        assert!(outcome.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. } if text == "在线源暂时不可用"
        )));
    }

    #[test]
    fn search_outcome_empty_is_an_empty_table_with_hint() {
        let outcome = build_search_outcome("没有结果", &[], false);

        let table = outcome
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .expect("empty table");
        assert!(table.is_empty());
        assert!(outcome.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. } if text == "换个关键词试试"
        )));
    }

    struct FakeOnline {
        tracks: Vec<SourceTrack>,
        all_failed: bool,
    }

    impl OnlineApi for FakeOnline {
        fn known_sources(&self) -> Vec<SourceId> {
            KNOWN_SOURCES.to_vec()
        }

        fn set_source_enabled(&self, _source: SourceId, _enabled: bool) -> Result<(), CmdError> {
            Ok(())
        }

        fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
            Ok(OnlineSearch {
                tracks: self.tracks.clone(),
                all_failed: self.all_failed,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            _progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
        }
    }

    struct FakeUpOnline {
        resolution: Result<UpResolution, CmdError>,
        archives: Result<Vec<SourceTrack>, CmdError>,
        resolve_calls: AtomicUsize,
        archive_calls: Mutex<Vec<(u64, String, String, usize)>>,
    }

    impl FakeUpOnline {
        fn new(
            resolution: Result<UpResolution, CmdError>,
            archives: Result<Vec<SourceTrack>, CmdError>,
        ) -> Self {
            Self {
                resolution,
                archives,
                resolve_calls: AtomicUsize::new(0),
                archive_calls: Mutex::new(Vec::new()),
            }
        }

        fn resolve_calls(&self) -> usize {
            self.resolve_calls.load(Ordering::SeqCst)
        }

        fn archive_calls(&self) -> Vec<(u64, String, String, usize)> {
            self.archive_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl OnlineApi for FakeUpOnline {
        fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
            Ok(OnlineSearch {
                tracks: Vec::new(),
                all_failed: false,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            _progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
        }

        fn resolve_up(&self, _name: &str) -> Result<UpResolution, CmdError> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            self.resolution.clone()
        }

        fn up_archives(
            &self,
            mid: u64,
            name: &str,
            keyword: &str,
            limit: usize,
        ) -> Result<Vec<SourceTrack>, CmdError> {
            self.archive_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((mid, name.to_owned(), keyword.to_owned(), limit));
            self.archives.clone()
        }
    }

    struct RecordingOnline {
        tracks: Vec<SourceTrack>,
        calls: Mutex<Vec<(String, usize, Option<SourceId>)>>,
    }

    impl RecordingOnline {
        fn new(tracks: Vec<SourceTrack>) -> Self {
            Self {
                tracks,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(String, usize, Option<SourceId>)> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl OnlineApi for RecordingOnline {
        fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError> {
            self.search_all_for(query, limit, None)
        }

        fn search_all_for(
            &self,
            query: &str,
            limit: usize,
            source: Option<SourceId>,
        ) -> Result<OnlineSearch, CmdError> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((query.to_owned(), limit, source));
            let mut tracks = self.tracks.clone();
            if let Some(source) = source {
                tracks.retain(|track| track.source == source);
            }
            Ok(OnlineSearch {
                tracks,
                all_failed: false,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            _progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            Err(CmdError::new("E_SRC_UNAVAILABLE", "测试未实现"))
        }
    }

    struct FakeQueueOnline {
        path: PathBuf,
    }

    impl OnlineApi for FakeQueueOnline {
        fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
            Ok(OnlineSearch {
                tracks: Vec::new(),
                all_failed: false,
            })
        }

        fn prepare(
            &self,
            _track: &SourceTrack,
            _progress: &mut dyn FnMut(u64),
        ) -> Result<PreparedTrack, CmdError> {
            Ok(PreparedTrack {
                path: self.path.clone(),
                bytes: 4,
                cached: false,
            })
        }
    }

    fn running_template() -> Block {
        Block {
            id: "online-block".to_owned(),
            input: "play 1".to_owned(),
            started_at: 1_000,
            duration_ms: None,
            status: BlockStatus::Running,
            outputs: Vec::new(),
            footer: None,
            schema_version: SCHEMA_VERSION,
            actions: None,
        }
    }

    #[test]
    fn run_online_search_merges_local_and_online_hits() {
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Local",
            "Artist",
            "/music/local.flac",
            1_000,
        )]);
        let online = FakeOnline {
            tracks: vec![online_track(SourceId::Bilibili, "BV1xx", "Bili")],
            all_failed: false,
        };
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        let template = running_template();

        run_online_search(
            "测试",
            MAX_SEARCH_RESULTS,
            &online,
            &library,
            &emit,
            &template,
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done block");
        assert_eq!(done.status, BlockStatus::Done);
        let rows = done
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { rows, .. } => Some(rows.clone()),
                _ => None,
            })
            .expect("table");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], "本地");
        assert_eq!(rows[1][1], "B站");
        assert_eq!(library.hits().len(), 2);
        assert_eq!(
            library.active_listing(),
            Some(ActiveListing {
                source: ListingSource::Search,
                len: 2,
            })
        );
    }

    struct FakePlayback {
        paths: Mutex<Vec<String>>,
        fail: bool,
    }

    impl FakePlayback {
        fn new() -> Self {
            Self {
                paths: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                paths: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn paths(&self) -> Vec<String> {
            self.paths.lock().expect("paths mutex").clone()
        }
    }

    impl PlaybackTarget for FakePlayback {
        fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError> {
            self.paths
                .lock()
                .expect("paths mutex")
                .push(path.to_owned());
            if self.fail {
                return Err(CmdError::new("E_DECODE", "无法解码"));
            }
            Ok(PlaybackState::default())
        }
    }

    #[test]
    fn run_online_play_downloads_then_plays_with_footer() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let playback = FakePlayback::new();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        let template = running_template();

        run_online_play(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            0,
            &service,
            &playback,
            &emit,
            &template,
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        assert_eq!(done.status, BlockStatus::Done);
        assert!(
            done.footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：B站") && footer.contains("已缓存"))
        );
        assert_eq!(playback.paths().len(), 1);
    }

    #[test]
    fn run_online_play_cached_footer_says_hit() {
        let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let track = online_track(SourceId::Netease, "10001", "Song");
        service.prepare(&track, &mut |_| {}).expect("预热缓存");
        let playback = FakePlayback::new();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_play(
            &track,
            0,
            &service,
            &playback,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let done = blocks.last().expect("done");
        assert_eq!(done.status, BlockStatus::Done);
        assert!(
            done.footer.as_deref().is_some_and(
                |footer| footer.contains("来源：网易云") && footer.contains("命中缓存")
            )
        );
    }

    #[test]
    fn run_online_play_error_block_uses_mapped_code() {
        let online = FakeOnline {
            tracks: Vec::new(),
            all_failed: false,
        };
        struct FailingOnline;
        impl OnlineApi for FailingOnline {
            fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
                Ok(OnlineSearch {
                    tracks: Vec::new(),
                    all_failed: false,
                })
            }

            fn prepare(
                &self,
                _track: &SourceTrack,
                _progress: &mut dyn FnMut(u64),
            ) -> Result<PreparedTrack, CmdError> {
                Err(CmdError::new("E_SRC_UNAVAILABLE", "版权受限"))
            }
        }
        let playback = FakePlayback::new();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_play(
            &online_track(SourceId::Netease, "10002", "VIP"),
            0,
            &FailingOnline,
            &playback,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let error = blocks.last().expect("error block");
        assert_eq!(error.status, BlockStatus::Error);
        assert_eq!(error_fields(error).0, "E_SRC_UNAVAILABLE");
        assert!(playback.paths().is_empty());
        let _ = online;
    }

    #[test]
    fn run_online_play_progress_is_time_throttled() {
        struct ProgressOnline {
            clock: Arc<AtomicU64>,
            values: Vec<(u64, u64)>,
            path: PathBuf,
        }
        impl OnlineApi for ProgressOnline {
            fn search_all(&self, _query: &str, _limit: usize) -> Result<OnlineSearch, CmdError> {
                Ok(OnlineSearch {
                    tracks: Vec::new(),
                    all_failed: false,
                })
            }

            fn prepare(
                &self,
                _track: &SourceTrack,
                progress: &mut dyn FnMut(u64),
            ) -> Result<PreparedTrack, CmdError> {
                for (at, written) in &self.values {
                    self.clock.store(*at, Ordering::SeqCst);
                    progress(*written);
                }
                Ok(PreparedTrack {
                    path: self.path.clone(),
                    bytes: 4,
                    cached: false,
                })
            }
        }

        let clock = Arc::new(AtomicU64::new(1_000));
        let dir = TestDir::new("progress");
        let path = dir.path().join("audio.mp3");
        fs::write(&path, b"abc").expect("write");
        let online = ProgressOnline {
            clock: Arc::clone(&clock),
            values: vec![(1_000, 1), (1_200, 2), (1_600, 3), (2_200, 4)],
            path,
        };
        let playback = FakePlayback::new();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };
        let clock_for_run = Arc::clone(&clock);

        run_online_play(
            &online_track(SourceId::Bilibili, "BV1xx", "Video"),
            0,
            &online,
            &playback,
            &emit,
            &running_template(),
            &move || clock_for_run.load(Ordering::SeqCst),
        );

        let blocks = emitted.lock().expect("emitted");
        let progress_blocks = blocks
            .iter()
            .filter(|block| {
                block
                    .outputs
                    .iter()
                    .any(|output| matches!(output, Output::Progress { .. }))
            })
            .count();
        assert_eq!(progress_blocks, 3, "500ms 节流：1000/1600/2200 各一次");
    }

    #[test]
    fn run_online_play_playback_failure_emits_error() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let playback = FakePlayback::failing();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emit = {
            let emitted = Arc::clone(&emitted);
            move |block: &Block| emitted.lock().expect("emitted").push(block.clone())
        };

        run_online_play(
            &online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video"),
            0,
            &service,
            &playback,
            &emit,
            &running_template(),
            &|| 2_000,
        );

        let blocks = emitted.lock().expect("emitted");
        let error = blocks.last().expect("error block");
        assert_eq!(error.status, BlockStatus::Error);
        assert_eq!(error_fields(error).0, "E_DECODE");
        assert_eq!(playback.paths().len(), 1);
    }

    #[test]
    fn search_dispatch_only_goes_online_without_local_flag() {
        let online = parse("search 晴天").expect("valid").expect("command");
        let local = parse("search 晴天 --local")
            .expect("valid")
            .expect("command");

        assert!(search_is_online(&online));
        assert!(!search_is_online(&local));
    }

    #[test]
    fn online_play_hit_requires_search_listing_and_online_track() {
        let hits = vec![
            SearchHit::Local(track(1, "Local", "Artist", "/music/a.flac", 1_000)),
            SearchHit::Online(online_track(SourceId::Bilibili, "BV1xx", "Bili")),
        ];
        let play_two = parse("play 2").expect("valid").expect("command");
        let play_one = parse("play 1").expect("valid").expect("command");
        let play_three = parse("play 3").expect("valid").expect("command");
        let search = ActiveListing {
            source: ListingSource::Search,
            len: 2,
        };
        let queue = ActiveListing {
            source: ListingSource::Queue,
            len: 2,
        };

        let hit = online_play_hit(&play_two, Some(&search), &hits).expect("在线结果");
        assert_eq!(hit.0, 1);
        assert_eq!(hit.1.source_id, "BV1xx");
        assert!(
            online_play_hit(&play_one, Some(&search), &hits).is_none(),
            "本地结果不截走"
        );
        assert!(
            online_play_hit(&play_two, Some(&queue), &hits).is_none(),
            "队列上下文不截走"
        );
        assert!(
            online_play_hit(&play_two, None, &hits).is_none(),
            "无上下文不截走"
        );
        assert!(
            online_play_hit(&play_three, Some(&search), &hits).is_none(),
            "越界"
        );
    }

    // ---- 远程曲目元数据（T-0039） ----

    #[test]
    fn remote_track_has_the_frontend_track_shape() {
        let dir = TestDir::new("remote-shape");
        let service = OnlineService::new(
            dir.path().to_path_buf(),
            dir.path().join("covers"),
            KNOWN_SOURCES.iter().copied().collect(),
        );
        let path = dir.path().join("bilibili-BV1xx411c7mD.m4a");
        fs::write(&path, b"abc").expect("write cache");
        let source_track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "晴天");
        service.remember_remote(&path, &source_track, Some("0123456789abcdef".to_string()));

        let track = service.track_by_path(&path).expect("远程命中");

        assert_eq!(track.id, 0);
        assert_eq!(track.kind, "remote");
        assert_eq!(track.path, path.to_string_lossy());
        assert_eq!(track.source, "bilibili");
        assert_eq!(track.source_id, "BV1xx411c7mD");
        assert_eq!(track.title, "晴天");
        assert_eq!(track.artist, "在线艺人");
        assert_eq!(track.duration_ms, 200_000);
        assert_eq!(track.album, "");
        assert_eq!(track.cover_key.as_deref(), Some("0123456789abcdef"));
        assert_eq!(track.size_bytes, 3);
        assert_eq!(track.track_no, 0);
    }

    #[test]
    fn lookup_track_prefers_local_and_skips_remote_map() {
        let dir = TestDir::new("remote-local-first");
        let service = OnlineService::new(
            dir.path().to_path_buf(),
            dir.path().join("covers"),
            KNOWN_SOURCES.iter().copied().collect(),
        );
        let path = dir.path().join("bilibili-BV1xx.m4a");
        fs::write(&path, b"abc").expect("write cache");
        service.remember_remote(
            &path,
            &online_track(SourceId::Bilibili, "BV1xx", "远程标题"),
            None,
        );
        let local = track(1, "本地标题", "本地艺人", "/music/local.flac", 1_000);

        let found = lookup_track(Some(local.clone()), &service, &path);

        assert_eq!(found, Some(local));
        assert_eq!(service.track_lookups(), 0, "本地命中不应查远程映射");
    }

    #[test]
    fn lookup_track_returns_none_when_both_sides_miss() {
        let dir = TestDir::new("remote-miss");
        let service = OnlineService::new(
            dir.path().to_path_buf(),
            dir.path().join("covers"),
            KNOWN_SOURCES.iter().copied().collect(),
        );

        assert!(lookup_track(None, &service, &dir.path().join("nope.m4a")).is_none());
        assert_eq!(service.track_lookups(), 1);
    }

    #[test]
    fn prepare_success_registers_remote_metadata() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let source_track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "晴天");

        let prepared = service.prepare(&source_track, &mut |_| {}).expect("下载");

        let track = service.track_by_path(&prepared.path).expect("已登记");
        assert_eq!(track.kind, "remote");
        assert_eq!(track.title, "晴天");
        assert_eq!(track.source_id, "BV1xx411c7mD");
        assert_eq!(service.remote_meta_len(), 1);
    }

    #[test]
    fn prepare_failure_does_not_register_remote_metadata() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", true, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));

        service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx411c7mD", "失败"),
                &mut |_| {},
            )
            .expect_err("下载失败");

        assert_eq!(service.remote_meta_len(), 0);
    }

    #[test]
    fn missing_cache_file_removes_remote_metadata() {
        let dir = TestDir::new("remote-missing");
        let service = OnlineService::new(
            dir.path().to_path_buf(),
            dir.path().join("covers"),
            KNOWN_SOURCES.iter().copied().collect(),
        );
        let path = dir.path().join("netease-10001.mp3");
        fs::write(&path, b"abc").expect("write cache");
        service.remember_remote(
            &path,
            &online_track(SourceId::Netease, "10001", "Song"),
            None,
        );
        fs::remove_file(&path).expect("remove cache");

        assert!(service.track_by_path(&path).is_none());
        assert_eq!(service.remote_meta_len(), 0);
    }

    #[test]
    fn prepare_prunes_metadata_for_files_removed_by_cache_cleanup() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let (_dir, service) = online_service(Arc::clone(&source), Arc::clone(&downloader));
        let stale = _dir.path().join("bilibili-BVstale.m4a");
        fs::write(&stale, b"old").expect("write stale");
        service.remember_remote(
            &stale,
            &online_track(SourceId::Bilibili, "BVstale", "Old"),
            None,
        );
        fs::remove_file(&stale).expect("模拟 LRU 清理");

        service
            .prepare(
                &online_track(SourceId::Bilibili, "BVnew", "New"),
                &mut |_| {},
            )
            .expect("新曲目下载");

        assert_eq!(service.remote_meta_len(), 1, "只应保留新下载的映射");
        assert!(service.track_by_path(&stale).is_none());
    }

    #[test]
    fn library_track_by_path_falls_back_to_remote_metadata() {
        let dir = TestDir::new("remote-library");
        let handle = LibraryHandle::new(dir.path().join("library.db"));
        let path = dir.path().join("netease-10001.mp3");
        fs::write(&path, b"abc").expect("write cache");
        handle.online.remember_remote(
            &path,
            &online_track(SourceId::Netease, "10001", "Song"),
            None,
        );

        let found = handle
            .track_by_path(path.to_str().expect("utf8"))
            .expect("查询")
            .expect("远程命中");

        assert_eq!(found.kind, "remote");
        assert_eq!(found.source, "netease");
        assert_eq!(found.title, "Song");
    }

    #[test]
    fn prepare_fetches_bilibili_cover_with_referer_and_records_key() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let dir = TestDir::new("cover-bili");
        let cache = StreamCache::with_limits(
            dir.path(),
            Arc::clone(&downloader) as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
        let transport = Arc::new(FakeCoverTransport::new(b"jpeg"));
        let store = Arc::new(FakeCoverStore::new(Some("0123456789abcdef")));
        let service = OnlineService::from_parts(
            registry,
            cache,
            Arc::clone(&transport) as Arc<dyn Transport>,
            dir.path().join("covers"),
            Arc::clone(&store) as Arc<dyn CoverStore>,
        );
        let mut track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video");
        track.cover_url = Some("https://example.invalid/cover.jpg".to_string());

        let prepared = service.prepare(&track, &mut |_| {}).expect("下载");

        assert_eq!(store.calls(), vec![b"jpeg".to_vec()]);
        let calls = transport.calls();
        assert_eq!(calls[0].0, "https://example.invalid/cover.jpg");
        assert!(
            calls[0]
                .1
                .iter()
                .any(|(name, value)| { name == "Referer" && value == "https://www.bilibili.com" })
        );
        let remote = service.track_by_path(&prepared.path).expect("元数据");
        assert_eq!(remote.cover_key.as_deref(), Some("0123456789abcdef"));
    }

    #[test]
    fn cover_failure_does_not_fail_prepare() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let dir = TestDir::new("cover-fail");
        let cache = StreamCache::with_limits(
            dir.path(),
            Arc::clone(&downloader) as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
        let transport = Arc::new(FakeCoverTransport::new(b"not-an-image"));
        let store = Arc::new(FakeCoverStore::new(None));
        let service = OnlineService::from_parts(
            registry,
            cache,
            Arc::clone(&transport) as Arc<dyn Transport>,
            dir.path().join("covers"),
            Arc::clone(&store) as Arc<dyn CoverStore>,
        );
        let mut track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video");
        track.cover_url = Some("https://example.invalid/cover.jpg".to_string());

        let prepared = service
            .prepare(&track, &mut |_| {})
            .expect("播放不受封面影响");

        assert!(service.track_by_path(&prepared.path).is_some());
        assert_eq!(store.calls().len(), 1);
        assert_eq!(
            service
                .track_by_path(&prepared.path)
                .expect("元数据")
                .cover_key,
            None
        );
    }

    #[test]
    fn cache_hit_reuses_cover_key_without_refetching() {
        let source = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let dir = TestDir::new("cover-cache");
        let cache = StreamCache::with_limits(
            dir.path(),
            Arc::clone(&downloader) as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
        let transport = Arc::new(FakeCoverTransport::new(b"jpeg"));
        let store = Arc::new(FakeCoverStore::new(Some("0123456789abcdef")));
        let service = OnlineService::from_parts(
            registry,
            cache,
            Arc::clone(&transport) as Arc<dyn Transport>,
            dir.path().join("covers"),
            Arc::clone(&store) as Arc<dyn CoverStore>,
        );
        let mut track = online_track(SourceId::Bilibili, "BV1xx411c7mD", "Video");
        track.cover_url = Some("https://example.invalid/cover.jpg".to_string());

        service.prepare(&track, &mut |_| {}).expect("首次");
        service.prepare(&track, &mut |_| {}).expect("命中缓存");

        assert_eq!(transport.calls().len(), 1, "缓存命中不应重新抓封面");
        assert_eq!(store.calls().len(), 1);
    }

    #[test]
    fn netease_cover_has_no_referer_header() {
        let source = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let dir = TestDir::new("cover-netease");
        let cache = StreamCache::with_limits(
            dir.path(),
            Arc::clone(&downloader) as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![source as Arc<dyn AudioSource>]);
        let transport = Arc::new(FakeCoverTransport::new(b"jpeg"));
        let store = Arc::new(FakeCoverStore::new(Some("0123456789abcdef")));
        let service = OnlineService::from_parts(
            registry,
            cache,
            Arc::clone(&transport) as Arc<dyn Transport>,
            dir.path().join("covers"),
            Arc::clone(&store) as Arc<dyn CoverStore>,
        );
        let mut track = online_track(SourceId::Netease, "10001", "Song");
        track.cover_url = Some("https://example.invalid/cover.jpg".to_string());

        service.prepare(&track, &mut |_| {}).expect("下载");

        assert!(transport.calls()[0].1.is_empty());
    }

    #[test]
    fn cache_root_is_injected_from_setup() {
        let dir = TestDir::new("cache-root");
        let cache_root = dir.path().join("system-cache");
        let handle =
            LibraryHandle::with_cache_root(dir.path().join("library.db"), cache_root.clone());

        assert_eq!(handle.online.cache_root(), cache_root);
    }

    #[test]
    fn shutdown_sets_cancel_flag_and_is_idempotent() {
        let dir = TestDir::new("shutdown-cancel");
        let handle = LibraryHandle::new(dir.path().join("library.db"));
        assert!(!handle.online.is_cancelled());

        handle.shutdown();
        handle.shutdown();

        assert!(handle.online.is_cancelled());
    }

    // ---- 设置命令族：theme（T-0042） ----

    struct FakeTheme {
        entries: Vec<ThemeListEntry>,
        apply: Result<ThemeApplyReply, CmdError>,
        export: Result<ThemeExportTarget, CmdError>,
        broadcasts: Mutex<Vec<String>>,
    }

    impl FakeTheme {
        fn new(apply: Result<ThemeApplyReply, CmdError>) -> Self {
            Self {
                entries: vec![
                    ThemeListEntry {
                        id: "graphite".to_owned(),
                        name: "石墨".to_owned(),
                        skin: np_core::theme::Skin::Plain,
                        source: ThemeSource::Builtin,
                    },
                    ThemeListEntry {
                        id: "mine".to_owned(),
                        name: "我的".to_owned(),
                        skin: np_core::theme::Skin::Phosphor,
                        source: ThemeSource::User,
                    },
                ],
                apply,
                export: Ok(ThemeExportTarget {
                    path: "/themes/ice.json".to_owned(),
                    created: true,
                }),
                broadcasts: Mutex::new(Vec::new()),
            }
        }

        fn broadcasts(&self) -> Vec<String> {
            self.broadcasts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl ThemeApi for FakeTheme {
        fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError> {
            Ok(self.entries.clone())
        }

        fn apply(&self, _id: &str) -> Result<ThemeApplyReply, CmdError> {
            self.apply.clone()
        }

        fn export(&self, _id: &str) -> Result<ThemeExportTarget, CmdError> {
            self.export.clone()
        }

        fn broadcast(&self, id: &str) {
            self.broadcasts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(id.to_owned());
        }
    }

    fn theme_reply(id: &str) -> ThemeApplyReply {
        ThemeApplyReply {
            id: id.to_owned(),
            name: "冰".to_owned(),
            skin: np_core::theme::Skin::Plain,
            source: ThemeSource::Builtin,
            tokens: np_core::theme::ThemeTokens::default(),
            warnings: Vec::new(),
        }
    }

    fn settings_for(dir: &TestDir) -> SettingsHandle {
        SettingsHandle::load(dir.path().join("config"))
    }

    fn done_with_services(
        api: &FakeApi,
        library: &FakeLibraryApi,
        settings: &SettingsHandle,
        theme: &FakeTheme,
        input: &str,
    ) -> Block {
        let services = Services {
            settings: Some(settings),
            theme: Some(theme),
            online: None,
        };
        match execute_with_services(
            api,
            library,
            services,
            "block-services",
            input,
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Blocks { done, .. } => *done,
            Execution::Accepted { .. } => panic!("unexpected accepted block: {input}"),
            Execution::Ignored => panic!("expected a block: {input}"),
        }
    }

    #[test]
    fn theme_set_writes_settings_and_broadcasts() {
        let dir = TestDir::new("theme-set");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_services(&api, &library, &settings, &theme, "theme set ice");

        assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
        assert_eq!(settings.theme_id().as_deref(), Some("ice"));
        assert_eq!(theme.broadcasts(), vec!["ice".to_owned()]);
        assert!(text_of_block(&block).contains("主题已切换"));
    }

    #[test]
    fn theme_set_invalid_theme_does_not_write_or_broadcast() {
        let dir = TestDir::new("theme-set-bad");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Err(CmdError::new("E_THEME_NOT_FOUND", "主题不存在")));

        let block = done_with_services(&api, &library, &settings, &theme, "theme set nope");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_THEME_NOT_FOUND");
        assert_eq!(settings.theme_id(), None);
        assert!(theme.broadcasts().is_empty());
    }

    #[test]
    fn theme_list_marks_current_and_sources() {
        let dir = TestDir::new("theme-list");
        let settings = settings_for(&dir);
        settings
            .set_and_save("theme.id", SettingValue::Str("mine".into()))
            .expect("save theme");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_services(&api, &library, &settings, &theme, "theme list");

        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "");
        assert_eq!(rows[0][1], "graphite");
        assert_eq!(rows[0][3], "内置");
        assert_eq!(rows[1][0], "●");
        assert_eq!(rows[1][1], "mine");
        assert_eq!(rows[1][3], "用户");
        assert_eq!(rows[1][4], "phosphor");
    }

    #[test]
    fn theme_export_reports_path_and_created() {
        let dir = TestDir::new("theme-export");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_services(&api, &library, &settings, &theme, "theme export ice");

        assert_eq!(block.status, BlockStatus::Done);
        assert!(text_of_block(&block).contains("/themes/ice.json"));
        assert!(text_of_block(&block).contains("已导出"));
    }

    fn done_with_online_services(
        api: &FakeApi,
        library: &FakeLibraryApi,
        settings: &SettingsHandle,
        theme: &FakeTheme,
        online: &dyn OnlineApi,
        input: &str,
    ) -> Block {
        let services = Services {
            settings: Some(settings),
            theme: Some(theme),
            online: Some(online),
        };
        match execute_with_services(
            api,
            library,
            services,
            "block-online-services",
            input,
            Arc::new(NoopLibraryObserver),
        ) {
            Execution::Blocks { done, .. } => *done,
            Execution::Accepted { .. } => panic!("unexpected accepted block: {input}"),
            Execution::Ignored => panic!("expected a block: {input}"),
        }
    }

    #[test]
    fn config_set_infers_types_and_lists_sorted() {
        let dir = TestDir::new("config-set");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set theme.id mine",
        );
        done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set playback.crossfadeMs 300",
        );
        done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set ui.compact true",
        );

        let entries = settings.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0, "playback.crossfadeMs");
        assert_eq!(entries[0].1, SettingValue::Num(300.0));
        assert_eq!(entries[1].0, "theme.id");
        assert_eq!(entries[2].1, SettingValue::Bool(true));

        let block = done_with_services(&api, &library, &settings, &theme, "config list");
        let text = text_of_block(&block);
        assert!(text.contains("playback.crossfadeMs = 300"));
        assert!(text.contains("theme.id = mine"));
    }

    #[test]
    fn config_get_missing_is_arg_error() {
        let dir = TestDir::new("config-get-missing");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_services(&api, &library, &settings, &theme, "config get theme.id");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(error_fields(&block).1.contains("未设置"));
    }

    #[test]
    fn config_unset_and_reset() {
        let dir = TestDir::new("config-reset");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));
        done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set ui.compact true",
        );
        done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set theme.id mine",
        );

        let unset =
            done_with_services(&api, &library, &settings, &theme, "config unset ui.compact");
        assert!(text_of_block(&unset).contains("已移除"));
        assert!(settings.get("ui.compact").is_none());

        let reset = done_with_services(&api, &library, &settings, &theme, "config reset");
        assert_eq!(reset.status, BlockStatus::Done);
        assert!(settings.entries().is_empty());
    }

    #[test]
    fn config_theme_id_requires_known_theme() {
        let dir = TestDir::new("config-theme-validate");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let ok = done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set theme.id mine",
        );
        assert_eq!(ok.status, BlockStatus::Done, "{}", text_of_block(&ok));
        assert_eq!(settings.theme_id().as_deref(), Some("mine"));

        let bad = done_with_services(
            &api,
            &library,
            &settings,
            &theme,
            "config set theme.id nope",
        );
        assert_eq!(bad.status, BlockStatus::Error);
        assert_eq!(settings.theme_id().as_deref(), Some("mine"));
    }

    #[test]
    fn config_source_enabled_requires_known_source_and_bool() {
        let dir = TestDir::new("config-source-validate");
        let settings = settings_for(&dir);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));
        let online = FakeOnline {
            tracks: Vec::new(),
            all_failed: false,
        };

        let ok = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &online,
            "config set source.bilibili.enabled false",
        );
        assert_eq!(ok.status, BlockStatus::Done, "{}", text_of_block(&ok));

        let bad = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &online,
            "config set source.youtube.enabled false",
        );
        assert_eq!(bad.status, BlockStatus::Error);
        assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(false));
    }

    fn two_source_service(
        dir: &TestDir,
        bili: Arc<FakeOnlineSource>,
        netease: Arc<FakeOnlineSource>,
    ) -> OnlineService {
        let downloader = Arc::new(FakeDownloader::new(b"abc", false, Vec::new()));
        let cache = StreamCache::with_limits(
            dir.path(),
            downloader as Arc<dyn Downloader>,
            10 * 1024 * 1024,
            5 * 1024 * 1024,
        );
        let registry = SourceRegistry::new(vec![
            bili as Arc<dyn AudioSource>,
            netease as Arc<dyn AudioSource>,
        ]);
        let transport: Arc<dyn Transport> = Arc::new(FakeCoverTransport::new(&[]));
        let cover_store: Arc<dyn CoverStore> = Arc::new(FakeCoverStore::new(None));
        OnlineService::from_parts(
            registry,
            cache,
            transport,
            dir.path().join("covers"),
            cover_store,
        )
    }

    #[test]
    fn source_list_reads_enabled_state_from_settings() {
        let dir = TestDir::new("source-list");
        let settings = settings_for(&dir);
        settings
            .set_and_save("source.bilibili.enabled", SettingValue::Bool(false))
            .expect("save");
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let service = two_source_service(&dir, bili, netease);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block =
            done_with_online_services(&api, &library, &settings, &theme, &service, "source list");

        let (_, rows) = table_of_block(&block);
        let bili_row = rows.iter().find(|row| row[0] == "bilibili").expect("bili");
        let netease_row = rows
            .iter()
            .find(|row| row[0] == "netease")
            .expect("netease");
        assert_eq!(bili_row[2], "禁用");
        assert_eq!(netease_row[2], "启用");
    }

    #[test]
    fn source_disable_writes_settings_and_blocks_old_result_resolve() {
        let dir = TestDir::new("source-disable");
        let settings = settings_for(&dir);
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let service = two_source_service(&dir, Arc::clone(&bili), netease);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "source disable bilibili",
        );

        assert_eq!(block.status, BlockStatus::Done, "{}", text_of_block(&block));
        assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(false));
        assert_eq!(service.known_sources().len(), 2, "禁用不删列表");
        let error = service
            .prepare(
                &online_track(SourceId::Bilibili, "BV1xx", "Old"),
                &mut |_| {},
            )
            .expect_err("禁用后旧结果应报错");
        assert_eq!(error.code, "E_SRC_UNAVAILABLE");
        assert!(error.message.contains("禁用"));
        assert_eq!(bili.resolves(), 0, "禁用源不应再 resolve");
    }

    #[test]
    fn disabled_source_is_skipped_in_search() {
        let dir = TestDir::new("source-search-skip");
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let service = two_source_service(&dir, Arc::clone(&bili), Arc::clone(&netease));
        service
            .set_source_enabled(SourceId::Bilibili, false)
            .expect("disable");

        service.search_all("测试", 5).expect("search");

        assert_eq!(bili.searches(), 0);
        assert_eq!(netease.searches(), 1);
    }

    #[test]
    fn source_enable_restores_settings_and_service() {
        let dir = TestDir::new("source-enable");
        let settings = settings_for(&dir);
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let service = two_source_service(&dir, bili, netease);
        service
            .set_source_enabled(SourceId::Bilibili, false)
            .expect("disable");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "source enable bilibili",
        );

        assert_eq!(block.status, BlockStatus::Done);
        assert_eq!(settings.get_bool("source.bilibili.enabled"), Some(true));
    }

    #[test]
    fn source_unknown_id_is_arg_error_without_side_effects() {
        let dir = TestDir::new("source-unknown");
        let settings = settings_for(&dir);
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        let service = two_source_service(&dir, bili, netease);
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "source disable youtube",
        );

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert!(settings.entries().is_empty());
    }

    fn cache_service(dir: &TestDir) -> OnlineService {
        let bili = FakeOnlineSource::new(SourceId::Bilibili, Vec::new());
        let netease = FakeOnlineSource::new(SourceId::Netease, Vec::new());
        two_source_service(dir, bili, netease)
    }

    #[test]
    fn cache_stat_counts_streams_and_covers() {
        let dir = TestDir::new("cache-stat");
        let settings = settings_for(&dir);
        let service = cache_service(&dir);
        fs::create_dir_all(dir.path().join("streams")).expect("streams");
        fs::create_dir_all(dir.path().join("covers")).expect("covers");
        fs::write(dir.path().join("streams/a.mp3"), b"abcd").expect("streams file");
        fs::write(dir.path().join("covers/b.jpg"), b"abcdef").expect("covers file");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block =
            done_with_online_services(&api, &library, &settings, &theme, &service, "cache stat");

        let text = text_of_block(&block);
        assert!(text.contains("streams · 1 个文件 / 4 B"), "{text}");
        assert!(text.contains("covers · 1 个文件 / 6 B"), "{text}");
        assert!(text.contains("合计 · 2 个文件 / 10 B"), "{text}");
    }

    #[test]
    fn cache_clear_streams_keeps_covers() {
        let dir = TestDir::new("cache-clear-streams");
        let settings = settings_for(&dir);
        let service = cache_service(&dir);
        fs::create_dir_all(dir.path().join("streams")).expect("streams");
        fs::create_dir_all(dir.path().join("covers")).expect("covers");
        fs::write(dir.path().join("streams/a.mp3"), b"abcd").expect("streams file");
        fs::write(dir.path().join("covers/b.jpg"), b"abcdef").expect("covers file");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "cache clear streams",
        );

        assert_eq!(block.status, BlockStatus::Done);
        assert!(!dir.path().join("streams/a.mp3").exists());
        assert!(dir.path().join("covers/b.jpg").exists());
        assert!(text_of_block(&block).contains("释放 4 B"));
    }

    #[test]
    fn cache_clear_covers_warns_rebuild() {
        let dir = TestDir::new("cache-clear-covers");
        let settings = settings_for(&dir);
        let service = cache_service(&dir);
        fs::create_dir_all(dir.path().join("covers")).expect("covers");
        fs::write(dir.path().join("covers/b.jpg"), b"abcdef").expect("covers file");
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "cache clear covers",
        );

        assert!(!dir.path().join("covers/b.jpg").exists());
        assert!(block.outputs.iter().any(|output| matches!(
            output,
            Output::Hint { text, .. } if text.contains("下次扫描会重建")
        )));
    }

    #[test]
    fn cache_clear_all_stops_remote_playback_first() {
        let dir = TestDir::new("cache-clear-remote");
        let settings = settings_for(&dir);
        let service = cache_service(&dir);
        let path = dir.path().join("streams/bilibili-BV1xx.mp3");
        fs::create_dir_all(dir.path().join("streams")).expect("streams");
        fs::write(&path, b"abcd").expect("cache file");
        service.remember_remote(
            &path,
            &online_track(SourceId::Bilibili, "BV1xx", "Song"),
            None,
        );
        let path_text = path.to_string_lossy().into_owned();
        let remote = service.track_by_path(&path).expect("remote meta");
        let api = FakeApi::new()
            .with_state(PlaybackState {
                track_id: Some(path_text.clone()),
                ..PlaybackState::default()
            })
            .with_queue(queue_view(&[&path_text], Some(0)));
        let library = FakeLibraryApi::new().with_track_by_path(&path_text, remote);
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "cache clear all",
        );

        assert_eq!(block.status, BlockStatus::Done);
        assert!(api.calls().contains(&Call::QueueRemove(0)));
        assert!(text_of_block(&block).contains("已先停止播放远程曲目"));
        assert!(!path.exists());
    }

    #[test]
    fn cache_clear_does_not_stop_local_playback() {
        let dir = TestDir::new("cache-clear-local");
        let settings = settings_for(&dir);
        let service = cache_service(&dir);
        let api = FakeApi::new()
            .with_state(PlaybackState {
                track_id: Some("/music/local.flac".to_owned()),
                ..PlaybackState::default()
            })
            .with_queue(queue_view(&["/music/local.flac"], Some(0)));
        let local = track(1, "Local", "Artist", "/music/local.flac", 1_000);
        let library = FakeLibraryApi::new().with_track_by_path("/music/local.flac", local);
        let theme = FakeTheme::new(Ok(theme_reply("ice")));

        let block = done_with_online_services(
            &api,
            &library,
            &settings,
            &theme,
            &service,
            "cache clear streams",
        );

        assert_eq!(block.status, BlockStatus::Done);
        assert!(!api.calls().contains(&Call::QueueRemove(0)));
        assert!(!text_of_block(&block).contains("已先停止"));
    }

    #[test]
    fn help_lists_the_new_settings_families() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "help");
        let text = text_of_block(&block);

        for family in ["theme", "config", "source", "cache"] {
            assert!(text.contains(family), "help 缺少 {family}: {text}");
        }
        assert!(block.outputs.len() >= 18);
    }
}

#[cfg(test)]
mod error_actions_tests {
    use super::*;

    /// 解析一个必然合法的命令；返回 Option<Parsed>。
    fn parsed(input: &str) -> Option<Parsed> {
        Some(
            parse(input)
                .expect("parse error")
                .expect("expected a command"),
        )
    }

    #[test]
    fn search_error_gets_retry_and_local_search() {
        let input = "search 晴天 --source netease";
        let actions = error_actions(input, parsed(input).as_ref()).expect("actions");

        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].label, "重试");
        assert_eq!(actions[0].input, input);
        assert_eq!(actions[1].label, "只用本地搜索");
        assert_eq!(actions[1].input, "search 晴天 --local");
    }

    #[test]
    fn local_search_error_only_gets_retry() {
        let input = "search 晴天 --local";
        let actions = error_actions(input, parsed(input).as_ref()).expect("actions");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].label, "重试");
    }

    #[test]
    fn non_search_error_only_gets_retry() {
        let input = "queue list";
        let actions = error_actions(input, parsed(input).as_ref()).expect("actions");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].input, input);
    }

    #[test]
    fn empty_input_has_no_actions() {
        assert_eq!(error_actions("   ", None), None);
    }

    #[test]
    fn keyword_with_spaces_is_quoted_for_the_action() {
        let input = "search \"a b\" --source netease";
        let actions = error_actions(input, parsed(input).as_ref()).expect("actions");

        assert_eq!(actions[1].input, "search \"a b\" --local");
    }
}
