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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use np_core::command::{Completion, complete, parse, registry};
use np_core::{
    Block, BlockStatus, Output, ParseError, Parsed, PlaybackMode, PlaybackState, PlayerError,
    PlayerErrorCode, QueueView, RepeatMode, SCHEMA_VERSION, Span,
};
use np_library::{
    CoverCacheUsage, Database, LibraryError, ScanObserver, ScanReport, Scanner, Stats, Track,
    cover_cache_usage,
};
use np_sources::{
    AudioSource, BilibiliSource, Downloader, HttpDownloader, NeteaseSource, ReqwestTransport,
    SourceError, SourceId, SourceRegistry, SourceTrack, StreamCache, Transport,
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

/// 搜索结果上限（T-0023 §6.4）。
pub(crate) const MAX_SEARCH_RESULTS: usize = 100;

/// 每个在线源在 `search` 里最多取多少条；合并后再按总上限截断。
pub(crate) const ONLINE_SEARCH_PER_SOURCE: usize = 20;

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
}

/// 音源在终端里的中文短名。
fn source_label(source: SourceId) -> &'static str {
    match source {
        SourceId::Bilibili => "B站",
        SourceId::Netease => "网易云",
        SourceId::Ytdlp => "yt-dlp",
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

/// 命令层依赖的在线音源能力；生产实现是 [`OnlineService`]，单测用假实现。
pub(crate) trait OnlineApi: Send + Sync {
    /// 串行搜索所有在线源；单源失败在实现内隔离。
    fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError>;
    /// resolve → 下载/缓存；进度按已写字节数回调。
    fn prepare(
        &self,
        track: &SourceTrack,
        progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError>;
}

/// 后台播放线程需要用到的播放器能力；`PlayerHandle` 实现它。
pub(crate) trait PlaybackTarget: Send + Sync {
    /// 播放本地路径。
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError>;
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
pub(crate) fn execute_with_library(
    api: &dyn PlayerApi,
    library: &dyn LibraryApi,
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
    };

    let clock = Instant::now();
    let result = match &parsed {
        Ok(parsed) => Executor {
            api,
            library,
            observer,
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
            ..running.clone()
        },
    };

    Execution::Blocks {
        running: Box::new(running),
        done: Box::new(done),
    }
}

/// 把 [`Parsed`] 映射成 [`PlayerApi`] / [`LibraryApi`] 调用并构造输出。
struct Executor<'a> {
    api: &'a dyn PlayerApi,
    library: &'a dyn LibraryApi,
    observer: Arc<dyn LibraryObserver>,
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
            "search" => self.search(parsed),
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

    /// `play`：无参 = 继续；纯数字 = 1 基序号；其余 = 文件路径。
    ///
    /// 数字序号**优先用播放队列**；队列为空且存在搜索结果时用搜索结果，
    /// 输出会写明来源（T-0023 §6.4）。
    fn play(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let Some(value) = parsed.positional.first() else {
            return Ok(state_outcome("继续播放", &self.api.resume()?));
        };

        if is_unsigned_integer(value) {
            let index = user_index(value)?;
            let queue = self.api.queue_list()?;
            if !queue.tracks.is_empty() {
                let view = self.api.queue_jump(index)?;
                return Ok(queue_jump_outcome(index, &view));
            }

            let hits = self.library.search_hits();
            if hits.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "播放队列为空，也没有搜索结果")
                    .with_hint("先用 search <关键词> 搜索，或 queue add <路径> 加入队列"));
            }
            let hit = hits.get(index).ok_or_else(|| {
                CmdError::new(
                    "E_CMD_ARG",
                    format!("搜索结果只有 {} 条，没有第 {} 条", hits.len(), index + 1),
                )
            })?;
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
                    hits.len(),
                    state_footer(&state)
                )),
            });
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
            "add" => self.add_paths(&parsed.positional),
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

    /// `queue list`：序号（1 基）/ 曲目 / 是否当前。
    fn queue_list(&self) -> Result<Outcome, CmdError> {
        let view = self.api.queue_list()?;
        let rows = view
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| {
                vec![
                    (index + 1).to_string(),
                    track_display(track),
                    if view.index == Some(index) {
                        "▶".to_owned()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();

        let footer = if view.tracks.is_empty() {
            "队列为空 · 用 queue add <路径> 加入曲目".to_owned()
        } else {
            format!("{} 首 · 输入 play 1 开始播放", view.tracks.len())
        };

        Ok(Outcome {
            outputs: vec![Output::Table {
                columns: vec!["序号".to_owned(), "曲目".to_owned(), "当前".to_owned()],
                rows,
                selectable: true,
            }],
            footer: Some(footer),
        })
    }

    /// `queue add`：逐个加入、汇总成功数，中途失败不回滚已加入的曲目。
    fn add_paths(&self, paths: &[String]) -> Result<Outcome, CmdError> {
        let mut added = 0usize;
        let mut failed = 0usize;
        let mut first_error: Option<CmdError> = None;
        let mut view: Option<QueueView> = None;

        for path in paths {
            match self.api.queue_add(vec![path.clone()]) {
                Ok(updated) => {
                    added += 1;
                    view = Some(updated);
                }
                Err(error) => {
                    failed += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if added == 0 {
            return Err(
                first_error.unwrap_or_else(|| CmdError::new("E_CMD_ARG", "没有要加入的曲目"))
            );
        }

        let mut spans = vec![bold(format!("已加入 {added} 首"))];
        if failed > 0 {
            spans.push(dim(format!(" · 失败 {failed} 首")));
        }
        let footer = match (&first_error, view) {
            (Some(error), _) => Some(format!("首条失败：{} {}", error.code, error.message)),
            (None, Some(view)) => Some(format!("队列现有 {} 首", view.tracks.len())),
            (None, None) => None,
        };

        Ok(Outcome {
            outputs: vec![text_of(spans)],
            footer,
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

    /// `search <关键词>`：结果带 1 基序号，并存进会话上下文供 `play <n>`。
    fn search(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let keyword = positional_arg(parsed, "关键词")?.trim();
        if keyword.is_empty() {
            return Err(CmdError::new("E_CMD_ARG", "搜索关键词不能为空"));
        }

        // 多取一条用来判断是否超过上限，避免“100 条正好”被误报为截断。
        let mut tracks = self.library.search(keyword, MAX_SEARCH_RESULTS + 1)?;
        let truncated = tracks.len() > MAX_SEARCH_RESULTS;
        tracks.truncate(MAX_SEARCH_RESULTS);
        self.library.remember_search(tracks.clone());

        if tracks.is_empty() {
            return Ok(Outcome {
                outputs: vec![text_of(vec![
                    bold("没有找到"),
                    dim(format!(" · {keyword}")),
                ])],
                footer: Some("换个关键词试试".to_owned()),
            });
        }

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
            outputs.push(text_of(vec![dim("结果超过 100 条，只显示前 100 条")]));
        }

        Ok(Outcome {
            outputs,
            footer: Some(format!(
                "{} 条结果 · 输入 play <序号> 播放（来源：搜索结果）",
                tracks.len()
            )),
        })
    }

    /// `library scan [--full]`：受理后台扫描并立即返回（`ok` = 已受理）。
    fn library_scan(&self, parsed: &Parsed) -> Result<Outcome, CmdError> {
        let full = parsed.flags.contains_key("full");
        self.library.scan(full, Arc::clone(&self.observer))?;
        let (outputs, footer) = scan_started_output(full);

        Ok(Outcome { outputs, footer })
    }

    /// `help`：不带参数列全部命令；带参数显示该命令的用法与说明。
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

/// `play <n>` 命中队列时的输出：写明来源是播放队列。
fn queue_jump_outcome(index: usize, view: &QueueView) -> Outcome {
    let mut outcome = jump_outcome("播放", index, view);
    outcome.footer = Some(format!("来源：播放队列 · 队列 {} 首", view.tracks.len()));
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
}

/// 生产版 [`LibraryApi`]：持有库路径、主线程连接、扫描任务与会话搜索结果。
///
/// `online` 是进程内唯一的在线音源服务；`LibraryHandle` 是 setup 里已经
/// managed 的状态，把它挂在这里可以避免新增 Tauri state 注册点。
pub(crate) struct LibraryHandle {
    inner: Arc<LibraryInner>,
    online: Arc<OnlineService>,
}

impl LibraryHandle {
    pub(crate) fn new(db_path: PathBuf) -> Self {
        let cache_root = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            inner: Arc::new(LibraryInner {
                db_path,
                db: Mutex::new(None),
                scan: Mutex::new(None),
                hits: Mutex::new(Vec::new()),
            }),
            online: Arc::new(OnlineService::new(cache_root)),
        }
    }

    /// 命令层持有的在线音源服务（与媒体库同生命周期）。
    pub(crate) fn online(&self) -> Arc<dyn OnlineApi> {
        Arc::clone(&self.online) as Arc<dyn OnlineApi>
    }

    /// 退出时打断后台扫描并等它提交已入库部分（复用 T-0013 的 shutdown 路径）。
    pub(crate) fn shutdown(&self) {
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

    /// 按绝对路径精确查一首曲目；不存在返回 `None`。
    pub(crate) fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
        self.with_db(|db| db.track_by_path(path))
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

struct OnlineRuntime {
    registry: SourceRegistry,
    cache: StreamCache,
}

/// 生产版 [`OnlineApi`]：持有注册表、下载缓存与进程内去重锁。
///
/// 传输层在首次使用时惰性创建：`LibraryHandle::new` 是 infallible 的，
/// 客户端构造失败会变成一次可读的 `E_SRC_UNAVAILABLE`，不影响本地命令。
pub(crate) struct OnlineService {
    cache_root: PathBuf,
    runtime: OnceLock<Result<OnlineRuntime, String>>,
    /// 单测注入的现成 runtime；生产恒为 `None`。
    fixed: Option<OnlineRuntime>,
    locks: Mutex<HashMap<DedupKey, DedupLock>>,
}

impl OnlineService {
    pub(crate) fn new(cache_root: PathBuf) -> Self {
        Self {
            cache_root,
            runtime: OnceLock::new(),
            fixed: None,
            locks: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_parts(registry: SourceRegistry, cache: StreamCache) -> Self {
        Self {
            cache_root: PathBuf::new(),
            runtime: OnceLock::new(),
            fixed: Some(OnlineRuntime { registry, cache }),
            locks: Mutex::new(HashMap::new()),
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
        let downloader: Arc<dyn Downloader> = Arc::new(HttpDownloader::new(transport));
        let cache = StreamCache::new(self.cache_root.clone(), downloader);
        Ok(OnlineRuntime { registry, cache })
    }

    fn lock_for(&self, key: &DedupKey) -> DedupLock {
        self.locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

impl OnlineApi for OnlineService {
    fn search_all(&self, query: &str, limit: usize) -> Result<OnlineSearch, CmdError> {
        let runtime = self.runtime()?;
        let attempted = runtime.registry.sources().len();
        let merged = runtime.registry.search_all(query, limit);
        let all_failed = attempted > 0 && merged.failures.len() == attempted;
        Ok(OnlineSearch {
            tracks: merged.tracks,
            all_failed,
        })
    }

    fn prepare(
        &self,
        track: &SourceTrack,
        progress: &mut dyn FnMut(u64),
    ) -> Result<PreparedTrack, CmdError> {
        let runtime = self.runtime()?;
        let key = (track.source, track.source_id.clone());
        let lock = self.lock_for(&key);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        // 命中缓存时连 resolve 都不做（网易云 URL 只有 20 分钟有效期）。
        if let Some(path) = runtime.cache.lookup(track.source, &track.source_id) {
            let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            return Ok(PreparedTrack {
                path,
                bytes,
                cached: true,
            });
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
                    return Ok(PreparedTrack {
                        path,
                        bytes,
                        cached: false,
                    });
                }
                Err(error) if is_expired_url_error(&error) && attempt == 0 => continue,
                Err(error) if is_expired_url_error(&error) => return Err(expired_url_error()),
                Err(error) => return Err(source_error_to_cmd(error)),
            }
        }

        Err(expired_url_error())
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
fn build_search_outcome(keyword: &str, hits: &[SearchHit], all_online_failed: bool) -> Outcome {
    if hits.is_empty() {
        let mut outputs = vec![
            text_of(vec![bold("没有找到"), dim(format!(" · {keyword}"))]),
            Output::Table {
                columns: search_columns(),
                rows: Vec::new(),
                selectable: true,
            },
        ];
        outputs.push(hint_output(if all_online_failed {
            "在线源暂时不可用"
        } else {
            "换个关键词试试"
        }));
        return Outcome {
            outputs,
            footer: None,
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
        footer: Some(format!(
            "{} 条结果 · 输入 play <序号> 播放（来源见「来源」列）",
            hits.len()
        )),
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
    }
}

/// 后台执行 `search`：本地库 + 在线源，合并后写入会话上下文并推最终块。
pub(crate) fn run_online_search(
    keyword: &str,
    limit: usize,
    online: &dyn OnlineApi,
    library: &dyn LibraryApi,
    emit: &dyn Fn(&Block),
    template: &Block,
    clock: &dyn Fn() -> u64,
) {
    let local = match library.search(keyword, limit) {
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
    };

    let (online_tracks, all_failed) = match online.search_all(keyword, ONLINE_SEARCH_PER_SOURCE) {
        Ok(result) => (result.tracks, result.all_failed),
        // 在线客户端初始化失败也不能吞掉本地结果。
        Err(_) => (Vec::new(), true),
    };

    let mut hits: Vec<SearchHit> = local.into_iter().map(SearchHit::Local).collect();
    hits.extend(online_tracks.into_iter().map(SearchHit::Online));
    hits.truncate(limit);
    library.remember_hits(hits.clone());

    let outcome = build_search_outcome(keyword, &hits, all_failed);
    emit(&finish_block(
        template,
        BlockStatus::Done,
        outcome.outputs,
        outcome.footer,
        clock,
    ));
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
    }
}

/// 在线命令的快速分派：命中时立即推 running 并起后台线程，返回回执。
///
/// 非在线命令返回 `None`，交回原来的同步执行器。`search --local` 与队列优先
/// 的 `play <n>` 都不会被这里截走。
fn maybe_start_online(
    app: &AppHandle,
    handle: &PlayerHandle,
    library: &LibraryHandle,
    input: &str,
    block_id: &str,
) -> Option<CmdExecReply> {
    let parsed = match parse(input) {
        Ok(Some(parsed)) => parsed,
        _ => return None,
    };

    if parsed.spec.name == "search" && !parsed.flags.contains_key("local") {
        let keyword = parsed.positional.first()?.trim().to_owned();
        if keyword.is_empty() {
            return None;
        }
        let template = search_started_block(block_id, input, &keyword);
        emit_block(app, &template);

        let app = app.clone();
        let online = library.online();
        std::thread::spawn(move || {
            let library = app.state::<LibraryHandle>();
            let emit_app = app.clone();
            let emit = move |block: &Block| emit_block(&emit_app, block);
            run_online_search(
                &keyword,
                MAX_SEARCH_RESULTS,
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

    if parsed.spec.name == "play" {
        let value = parsed.positional.first()?;
        if !is_unsigned_integer(value) {
            return None;
        }
        let index = user_index(value).ok()?;
        // 队列优先：队列非空时保持原有语义，交给执行器。
        if handle
            .queue_list()
            .ok()
            .is_some_and(|queue| !queue.tracks.is_empty())
        {
            return None;
        }
        let hits = library.search_hits();
        let SearchHit::Online(track) = hits.get(index)?.clone() else {
            return None;
        };

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

    match execute_with_library(&api, &*library, &block_id, &input, observer) {
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

        fn with_reject_paths(self, paths: &[&str]) -> Self {
            *self.reject_paths.lock().expect("reject mutex") =
                paths.iter().map(|path| (*path).to_owned()).collect();
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
    fn play_number_is_one_based_and_jumps() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], None));
        let block = done_block(&api, "play 3");

        assert_eq!(api.calls(), vec![Call::QueueList, Call::QueueJump(2)]);
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
    fn queue_list_renders_rows_and_current_marker() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac", "b.flac", "c.flac"], Some(1)));
        let block = done_block(&api, "queue list");

        let table = block
            .outputs
            .iter()
            .find_map(|output| match output {
                Output::Table { columns, rows, .. } => Some((columns, rows)),
                _ => None,
            })
            .expect("queue list must render a table");

        assert_eq!(
            table.0,
            &vec!["序号".to_owned(), "曲目".to_owned(), "当前".to_owned()]
        );
        assert_eq!(table.1.len(), 3);
        assert_eq!(table.1[0][0], "1");
        assert_eq!(table.1[1][2], "▶");
        assert_eq!(table.1[0][2], "");
    }

    #[test]
    fn queue_add_reports_success_count() {
        let api = FakeApi::new();
        let block = done_block(&api, r"queue add C:\Music\a.flac");

        assert_eq!(
            api.calls(),
            vec![Call::QueueAdd(vec![r"C:\Music\a.flac".to_owned()])]
        );
        assert!(text_of_block(&block).contains("已加入 1 首"));
        assert_eq!(block.footer.as_deref(), Some("队列现有 1 首"));
    }

    #[test]
    fn queue_add_keeps_going_after_partial_failure() {
        let api = FakeApi::new().with_reject_paths(&["b.flac"]);
        let paths = vec![
            "a.flac".to_owned(),
            "b.flac".to_owned(),
            "c.flac".to_owned(),
        ];
        let library = FakeLibraryApi::new();
        let outcome = Executor {
            api: &api,
            library: &library,
            observer: Arc::new(NoopLibraryObserver),
        }
        .add_paths(&paths)
        .expect("partial success is still a success");

        assert_eq!(api.calls().len(), 3);
        assert_eq!(
            outcome.footer.as_deref(),
            Some("首条失败：E_DECODE 无法加入：b.flac")
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
            vec![LibraryCall::Search("kid".to_owned(), 101)]
        );
        assert_eq!(library.session().len(), 2);
        let (_, rows) = table_of_block(&block);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "1");
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：搜索结果"))
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
    fn search_truncates_results_at_one_hundred() {
        let api = FakeApi::new();
        let tracks: Vec<Track> = (0..101)
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
        assert_eq!(rows.len(), 100);
        assert_eq!(library.session().len(), 100);
        assert!(text_of_block(&block).contains("只显示前 100 条"));
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
    fn play_prefers_the_queue_over_search_results() {
        let api = FakeApi::new().with_queue(queue_view(&["a.flac"], None));
        let library = FakeLibraryApi::new().with_search_tracks(vec![track(
            1,
            "Search Song",
            "Artist",
            "/music/search.flac",
            1_000,
        )]);
        done_with_library(&api, &library, "search song");
        let block = done_with_library(&api, &library, "play 1");

        assert_eq!(api.calls(), vec![Call::QueueList, Call::QueueJump(0)]);
        assert!(
            block
                .footer
                .as_deref()
                .is_some_and(|footer| footer.contains("来源：播放队列"))
        );
    }

    #[test]
    fn play_falls_back_to_search_results_when_the_queue_is_empty() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new().with_search_tracks(vec![
            track(1, "First", "Artist", "/music/first.flac", 1_000),
            track(2, "Second", "Artist", "/music/second.flac", 1_000),
        ]);
        done_with_library(&api, &library, "search artist");
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(
            api.calls(),
            vec![
                Call::QueueList,
                Call::Play(Some("/music/second.flac".to_owned()))
            ]
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
    fn play_number_without_queue_or_search_results_fails() {
        let api = FakeApi::new();
        let library = FakeLibraryApi::new();
        let block = done_with_library(&api, &library, "play 2");

        assert_eq!(block.status, BlockStatus::Error);
        assert_eq!(error_fields(&block).0, "E_CMD_ARG");
        assert_eq!(api.calls(), vec![Call::QueueList]);
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
        assert_eq!(api.calls(), vec![Call::QueueList]);
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
        assert_eq!(completion.items.len(), 1);
        assert_eq!(completion.items[0].text, "play");
        assert_eq!(
            completion.items[0].kind,
            np_core::command::CompletionKind::Command
        );
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
        plans: Mutex<Vec<ResolvePlan>>,
        stream: ResolvedStream,
        delay: Duration,
    }

    impl FakeOnlineSource {
        fn new(id: SourceId, plans: Vec<ResolvePlan>) -> Arc<Self> {
            Arc::new(Self {
                id,
                resolves: AtomicUsize::new(0),
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
    }

    impl AudioSource for FakeOnlineSource {
        fn id(&self) -> SourceId {
            self.id
        }

        fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SourceTrack>, SourceError> {
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
        (dir, OnlineService::from_parts(registry, cache))
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
                .is_some_and(|footer| footer.contains("3 条结果"))
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
}
