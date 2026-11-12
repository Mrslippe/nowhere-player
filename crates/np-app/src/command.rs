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
    Block, BlockAction, BlockStatus, BuiltinTheme, Output, ParseError, Parsed, PlaybackMode,
    PlaybackState, PlayerError, PlayerErrorCode, QueueView, RepeatMode, SCHEMA_VERSION,
    SettingValue, Settings, SettingsWarning, Span, ThemeApplyReply, ThemeListEntry, ThemeSource,
    validate_setting,
};
use np_library::{
    Database, LibraryError, NewPlaylistItem, Playlist, PlaylistItem, PlaylistSummary, ScanObserver,
    ScanReport, Scanner, Stats, Track,
};
use np_sources::{
    AudioSource, BilibiliSource, Downloader, HttpDownloader, MatchKind, NeteaseSource,
    ReqwestTransport, SourceError, SourceId, SourceRegistry, SourceTrack, StreamCache, Transport,
    build_query, matches_artist_in_query as match_source_artist_in_query, resolve_up_user,
};
/// 执行器依赖的播放器能力（T-0018 §6.1）。
///
/// 生产实现是 `HandleApi`（`player_actor` 里的 `PlayerHandle` 适配）；
/// 单测实现是 `FakeApi`。执行器只按 trait 调用，所以命令映射可以整体单测。
pub trait PlayerApi {
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
pub trait LibraryApi {
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
pub trait LibraryObserver: Send + Sync {
    /// 每处理完一个文件调用；实现方负责节流。
    fn on_progress(&self, scanned: usize, total: usize, current: &Path);
    /// 扫描正常结束（含取消）。
    fn on_finished(&self, report: &ScanReport);
    /// 扫描无法开始或异常结束。
    fn on_failed(&self, error: &CmdError);
}

/// 什么都不做的观察者：仅用于不需要进度的场景与播放器单测。
pub struct NoopLibraryObserver;

impl LibraryObserver for NoopLibraryObserver {
    fn on_progress(&self, _scanned: usize, _total: usize, _current: &Path) {}

    fn on_finished(&self, _report: &ScanReport) {}

    fn on_failed(&self, _error: &CmdError) {}
}

#[allow(dead_code)]
const HELP_GROUPS: &[(&str, &[&str])] = &[
    (
        "播放",
        &[
            "play", "pause", "resume", "next", "prev", "seek", "vol", "mode", "queue",
        ],
    ),
    ("媒体库", &["library", "search"]),
    ("歌单", &["playlist"]),
    (
        "系统",
        &[
            "clear", "version", "help", "theme", "config", "source", "cache",
        ],
    ),
];

const HELP_ENTRIES: &[(&str, &str, &[&str])] = &[
    (
        "play",
        "播放当前列表的第 n 首；不带参数继续当前曲目",
        &["play 3", "play"],
    ),
    ("pause", "暂停当前播放", &["pause"]),
    ("resume", "从暂停处继续", &["resume"]),
    ("next", "播放下一首", &["next"]),
    ("prev", "播放上一首", &["prev"]),
    (
        "seek",
        "跳转到指定播放位置；+/- 表示相对当前位置",
        &["seek 1:30", "seek +10"],
    ),
    ("vol", "设置音量；+/- 表示相对调整", &["vol 60", "vol +10"]),
    (
        "mode",
        "查看或设置循环/随机模式",
        &["mode repeat all", "mode shuffle on"],
    ),
    (
        "queue",
        "查看或编辑播放队列",
        &[
            "queue list",
            "queue add /music/a.flac",
            "queue add --from-search 2",
        ],
    ),
    (
        "library",
        "管理媒体库扫描根、扫描与浏览",
        &["library list", "library scan", "library add /music"],
    ),
    (
        "search",
        "搜索本地库与在线音源",
        &[
            "search 晴天",
            "search --artist 周杰伦 --source netease --limit 5",
            "search 最长的电影 --up 明前奶绿",
        ],
    ),
    (
        "playlist",
        "管理歌单：增删条目、显示与播放",
        &[
            "playlist list",
            "playlist create \"周杰伦 情歌\"",
            "playlist add \"周杰伦 情歌\" 1",
            "playlist play \"周杰伦 情歌\"",
        ],
    ),
    (
        "theme",
        "查看、切换、导出主题",
        &["theme list", "theme set ice", "theme export graphite"],
    ),
    (
        "config",
        "读写设置项",
        &[
            "config list",
            "config set theme.id ice",
            "config get theme.id",
        ],
    ),
    (
        "source",
        "启用/禁用在线音源",
        &["source list", "source disable bilibili"],
    ),
    (
        "cache",
        "统计或清理缓存",
        &["cache stat", "cache clear streams"],
    ),
    (
        "help",
        "查看命令分组或某条命令的详情",
        &["help", "help play", "help playlist"],
    ),
    ("version", "显示版本号", &["version"]),
    ("clear", "清空终端输出", &["clear"]),
];

fn help_entry(name: &str) -> (&'static str, &'static [&'static str]) {
    HELP_ENTRIES
        .iter()
        .find(|(command, _, _)| *command == name)
        .map_or(("", &[]), |(_, description, examples)| {
            (*description, *examples)
        })
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

/// 启动时清掉 T-0079 之前遗留的 `<data>/covers/`。
///
/// 返回删除的条目数。清理失败不阻塞启动：这里不打印、不重试，
/// 宿主需要诊断时由宿主自己决定是否记录。
fn purge_legacy_covers(dir: &Path) -> u64 {
    if !dir.is_dir() {
        return 0;
    }
    let mut removed = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if result.is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

fn target_label(target: CacheTarget) -> &'static str {
    match target {
        CacheTarget::Streams => "streams",
        CacheTarget::All => "streams",
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
pub struct OnlineSearch {
    /// 在线源返回的曲目（按注册顺序）。
    pub tracks: Vec<SourceTrack>,
    /// 是否所有在线源都失败（用于 hint）。
    pub all_failed: bool,
}

/// 已落盘的在线曲目。
#[derive(Debug, Clone)]
pub struct PreparedTrack {
    /// 本地缓存路径。
    pub path: PathBuf,
    /// 文件字节数。
    pub bytes: u64,
    /// 是否命中已有缓存（没有 resolve / 下载）。
    pub cached: bool,
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
}

/// 缓存清理目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTarget {
    Streams,
    All,
}

impl CacheTarget {
    fn parse(raw: &str) -> Result<Self, CmdError> {
        match raw {
            "streams" => Ok(Self::Streams),
            "all" => Ok(Self::All),
            _ => Err(CmdError::new("E_CMD_ARG", format!("未知缓存目标：{raw}"))
                .with_hint("可用：streams / all")),
        }
    }

    fn includes_streams(self) -> bool {
        matches!(self, Self::Streams | Self::All)
    }
}

/// 缓存统计（文件数 + 字节数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheUsage {
    pub streams_files: u64,
    pub streams_bytes: u64,
}

impl CacheUsage {
    fn total_files(self) -> u64 {
        self.streams_files
    }

    fn total_bytes(self) -> u64 {
        self.streams_bytes
    }
}

/// 用户名解析后的 UP（T-0049）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpResolution {
    pub mid: u64,
    pub name: Option<String>,
    pub fans: u64,
    /// 精确同名候选数；>1 时输出 --mid 提示。
    pub same_name: usize,
}

/// 命令层依赖的在线音源能力；生产实现是 [`OnlineService`]，单测用假实现。
pub trait OnlineApi: Send + Sync {
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
    /// streams 的文件数与体积。
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
pub trait PlaybackTarget: Send + Sync {
    /// 播放本地路径。
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError>;
}

/// `theme.export` 的回执（不把 ipc 的内部结构漏给命令层以外）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeExportTarget {
    pub path: String,
    pub created: bool,
}

/// 命令层依赖的主题能力；生产实现是 [`ThemeHandle`]，单测用假实现。
pub trait ThemeApi: Send + Sync {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError>;
    fn apply(&self, id: &str) -> Result<ThemeApplyReply, CmdError>;
    fn export(&self, id: &str) -> Result<ThemeExportTarget, CmdError>;
    /// `theme set` 写盘成功后广播给两个窗口。
    fn broadcast(&self, id: &str);
}

/// `theme:changed` 事件名与载荷（前端监听后调用 `theme.apply`）。
pub const THEME_CHANGED_EVENT: &str = "theme:changed";

pub fn normalize_cmd_dots(mut error: CmdError) -> CmdError {
    error.message = normalize_dotted_commands(&error.message);
    error.hint = error.hint.map(|hint| normalize_dotted_commands(&hint));
    error
}

/// IPC 层的历史注释式命令名（`theme.list`）在终端里统一成 `theme list`。
fn normalize_dotted_commands(text: &str) -> String {
    let mut normalized = text.to_owned();
    for (dotted, spaced) in [
        ("theme.list", "theme list"),
        ("theme.set", "theme set"),
        ("theme.exportBuiltin", "theme export"),
        ("theme.export", "theme export"),
        ("queue.list", "queue list"),
        ("library.list", "library list"),
        ("library.scan", "library scan"),
        ("library.stat", "library stat"),
        ("playlist.show", "playlist show"),
        ("playlist.add", "playlist add"),
        ("config.list", "config list"),
        ("source.list", "source list"),
        ("cache.stat", "cache stat"),
    ] {
        normalized = normalized.replace(dotted, spaced);
    }
    normalized
}

/// 扫描进度节流的默认间隔：2 Hz。
pub const PROGRESS_INTERVAL_MS: u64 = 500;

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

    fn remember_search(&self, _tracks: Vec<Track>) {}

    fn search_results(&self) -> Vec<Track> {
        Vec::new()
    }
}

/// 执行期错误：稳定的 IPC 码 + 一句原因 + 可选的修复提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    /// IPC 错误码，如 `E_CMD_ARG` / `E_UNSUPPORTED`。
    pub code: String,
    /// 一句话原因，可直接展示。
    pub message: String,
    /// 修复提示；解析错误用 `usage` 或拼写建议填充。
    pub hint: Option<String>,
}

impl CmdError {
    /// 用错误码与原因构造错误。
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
        }
    }

    /// 解析错误 → 执行错误：`hint` 优先用 `usage`，没有则用拼写建议。
    pub fn from_parse(error: &ParseError) -> Self {
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
    pub fn from_player(error: PlayerError) -> Self {
        Self {
            code: player_error_code(error.code).to_owned(),
            message: error.message,
            hint: None,
        }
    }

    /// 补一条提示；已有提示时不覆盖。
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        if self.hint.is_none() {
            self.hint = Some(hint.into());
        }
        self
    }

    /// 失败时至少给一条提示：没有更具体的就用命令用法。
    pub fn with_usage(mut self, parsed: &Parsed) -> Self {
        if self.hint.is_none() {
            self.hint = Some(format!("用法：{}", parsed.usage()));
        }
        self
    }

    /// 转成终端的 `error` 原语。
    pub fn to_output(&self) -> np_core::Output {
        np_core::Output::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            hint: self.hint.clone(),
            actions: None,
        }
    }
}

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

/// `cmd.exec` / `library.scan` 的“已受理”回执。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanAcceptedReply {
    pub accepted: bool,
}

/// IPC 错误载荷（`{ code, message, hint? }`），与终端 `error` 原语同形。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
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
pub fn run_online_search(
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

/// 把某通道的结果过滤、去重后并入累积列表。
fn absorb_online_tracks(
    options: &SearchOptions,
    tracks: Vec<SourceTrack>,
    seen: &mut HashSet<(String, String)>,
    out: &mut Vec<SourceTrack>,
) -> (usize, usize) {
    let mut fetched = 0;
    let mut removed = 0;
    for track in tracks {
        fetched += 1;
        if !options.no_filter
            && let Some(artist) = options.artist.as_deref()
            && matches!(
                match_source_artist_in_query(&track, artist, options.keyword_filter()),
                MatchKind::None
            )
        {
            removed += 1;
            continue;
        }
        let key = (track.source.as_str().to_owned(), track.source_id.clone());
        if seen.insert(key) {
            out.push(track);
        }
    }
    (fetched, removed)
}

pub fn run_online_search_with_options(
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
    if let Some(artist) = &options.artist
        && !options.no_filter
    {
        local.retain(|track| !matches!(local_artist_match(track, artist), MatchKind::None));
    }
    let local_removed = local_before.saturating_sub(local.len());
    // 默认视图给本地留 10 条上限，剩余名额给在线；显式 --limit 不压本地。
    local.truncate(options.local_cap());

    let started = clock();
    let mut online_tracks: Vec<SourceTrack> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut online_before = 0usize;
    let mut online_removed = 0usize;
    let mut all_failed;
    let mut notices: Vec<String> = Vec::new();

    // 只有「关键词 + --artist」才走 B/C/D 召回；只给 artist 保持旧行为。
    let recall_channels = options.has_keyword && options.artist.is_some();
    if !recall_channels {
        let result = online.search_all_for(
            &options.source_query(),
            options.candidate_pool(),
            options.source,
        );
        match result {
            Ok(result) => {
                all_failed = result.all_failed;
                let (fetched, removed) =
                    absorb_online_tracks(options, result.tracks, &mut seen, &mut online_tracks);
                online_before += fetched;
                online_removed += removed;
            }
            Err(_) => all_failed = true,
        }
    } else {
        let artist = options
            .artist
            .as_deref()
            .expect("recall channels require artist");
        let bili_allowed = options.source != Some(SourceId::Netease);

        // A：纯关键词，总是发。
        match online.search_all_for(&options.keyword, options.candidate_pool(), options.source) {
            Ok(result) => {
                all_failed = result.all_failed;
                let (fetched, removed) =
                    absorb_online_tracks(options, result.tracks, &mut seen, &mut online_tracks);
                online_before += fetched;
                online_removed += removed;
            }
            Err(_) => all_failed = true,
        }

        let over_deadline = || clock().saturating_sub(started) > 4_000;

        let mut d_resolution_failed = false;
        let mut d_blocked = false;

        // B 稳定、不消耗稀缺资源，先于 D。
        if bili_allowed && local.len() + online_tracks.len() < options.limit {
            // B：艺人 + 关键词。
            if over_deadline() {
                notices.push("召回超过 4s，已停止后续 B/C 通道".to_owned());
            } else {
                let query = build_query(&options.keyword, Some(artist));
                match online.search_all_for(
                    &query,
                    options.candidate_pool(),
                    Some(SourceId::Bilibili),
                ) {
                    Ok(result) => {
                        if result.all_failed {
                            all_failed = true;
                        }
                        let (fetched, removed) = absorb_online_tracks(
                            options,
                            result.tracks,
                            &mut seen,
                            &mut online_tracks,
                        );
                        online_before += fetched;
                        online_removed += removed;
                    }
                    Err(_) => all_failed = true,
                }
            }
        }

        // D 稀缺：A+B 不够才用；单请求、最高价值，不设超时守卫；只解析一次、只发一次，412 不重试。
        if bili_allowed && local.len() + online_tracks.len() < options.limit {
            match online.resolve_up(artist) {
                Ok(resolution) if resolution.same_name == 1 => {
                    let name = resolution.name.as_deref().unwrap_or(artist);
                    let remaining = options
                        .limit
                        .saturating_sub(local.len() + online_tracks.len())
                        .max(1);
                    match online.up_archives(resolution.mid, name, &options.keyword, remaining) {
                        Ok(tracks) => {
                            let (fetched, removed) = absorb_online_tracks(
                                options,
                                tracks,
                                &mut seen,
                                &mut online_tracks,
                            );
                            online_before += fetched;
                            online_removed += removed;
                        }
                        Err(error) => {
                            d_blocked = true;
                            notices.push(format!(
                                "空间检索被限流，本次只用普通搜索（{}）",
                                error.message
                            ));
                        }
                    }
                }
                Ok(_) => {
                    d_resolution_failed = true;
                    notices.push("未能解析到该艺人对应的 UP，已退回普通搜索".to_owned());
                }
                Err(error) => {
                    d_resolution_failed = true;
                    if error.message.contains("限流") || error.message.contains("412") {
                        d_blocked = true;
                        notices.push(format!(
                            "空间检索被限流，本次只用普通搜索（{}）",
                            error.message
                        ));
                    } else {
                        notices.push("未能解析到该艺人对应的 UP，已退回普通搜索".to_owned());
                    }
                }
            }
        }

        // C 只在 D 解析失败时兜底；D 已解析出唯一 mid 就不再发 C。
        if bili_allowed
            && local.len() + online_tracks.len() < options.limit
            && d_resolution_failed
            && !d_blocked
        {
            // C：仅艺人；标题仍要满足关键词约束。
            if over_deadline() {
                notices.push("召回超过 4s，已停止后续 C 通道".to_owned());
            } else {
                match online.search_all_for(
                    artist,
                    options.candidate_pool(),
                    Some(SourceId::Bilibili),
                ) {
                    Ok(result) => {
                        if result.all_failed {
                            all_failed = true;
                        }
                        let (fetched, removed) = absorb_online_tracks(
                            options,
                            result.tracks,
                            &mut seen,
                            &mut online_tracks,
                        );
                        online_before += fetched;
                        online_removed += removed;
                    }
                    Err(_) => all_failed = true,
                }
            }
        }
    }

    let before_artist = local_before.saturating_add(online_before);
    let removed = local_removed.saturating_add(online_removed);
    let mut hits: Vec<SearchHit> = local.into_iter().map(SearchHit::Local).collect();
    hits.extend(online_tracks.into_iter().map(SearchHit::Online));
    if let Some(artist) = &options.artist
        && !options.no_filter
    {
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

    let mut outcome = build_search_outcome_for(options, &hits, all_failed, before_artist, removed);
    for notice in notices {
        outcome.outputs.push(hint_output(&notice));
    }
    emit(&finish_block(
        template,
        BlockStatus::Done,
        outcome.outputs,
        outcome.footer,
        clock,
    ));
}

/// 先于后台线程推给前端的 `running` 块（UP 检索）。
pub fn up_started_block(block_id: &str, input: &str, target: &str) -> Block {
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
pub fn run_up_search(
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
pub fn run_online_play(
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
pub fn scan_started_block(block_id: &str, input: &str, full: bool) -> Block {
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
pub struct CmdExecReply {
    pub block_id: String,
    pub ok: bool,
}

/// `cmd.complete` 的纯逻辑：算候选，动态候选按需取一次播放器快照。
///
/// 快照失败（`E_BUSY` 等）只让动态候选降级为空列表，命令名 / 子命令 / 枚举值
/// 照常返回——补全不能被播放器状态卡住（T-0020 §6.1、§11）。
/// `cursor` 与返回的 `replaceStart` / `replaceEnd` 都是**字符偏移**。
/// `play <在线序号>` 的 running 块：resolve/下载都在后台，命令立即返回。
/// `search` 的 running 块：先让用户看到“搜索中”，在线请求结束后再覆盖。
pub fn search_started_block(block_id: &str, input: &str, keyword: &str) -> Block {
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

pub fn online_play_started_block(
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
pub fn online_play_hit(
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

pub fn complete_for(api: &dyn PlayerApi, input: &str, cursor: usize) -> Completion {
    complete(input, cursor, || {
        api.snapshot().ok().and_then(|state| state.queue_length)
    })
}

#[cfg(test)]
mod tests;

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

mod executor;
mod helpers;
mod library;
mod model;
mod online;
mod settings;

use executor::Outcome;
pub use executor::{Execution, Services, error_actions, execute_with_services};
#[cfg(test)]
pub use executor::{execute, execute_with_library};
use helpers::*;
pub use helpers::{EVENT_TERMINAL_BLOCK, now_millis, scan_full_flag};
pub use library::{EventObserver, LibraryHandle};
pub use model::*;
pub use online::OnlineService;
pub use settings::{KNOWN_SOURCES, SettingsHandle};
