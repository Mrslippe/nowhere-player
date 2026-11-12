//! 目录遍历、并行解析、单线程写库。
//!
//! 结构：生产者线程遍历目录并把每个文件丢给 rayon 线程池解析；解析结果经
//! 容量为 [`CHANNEL_CAPACITY`] 的同步 channel 回到调用线程（也就是唯一的
//! writer），由它串行写 SQLite。整轮扫描只有一个 `Connection` 参与写入。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{SystemTime, UNIX_EPOCH};

use walkdir::{DirEntry, WalkDir};

use crate::db::{Database, WriteBatch};
use crate::error::{LibraryError, Result};
use crate::metadata::{self, ParsedTrack};
use crate::repo;

/// 解析结果 channel 的容量上限。
const CHANNEL_CAPACITY: usize = 64;

/// 扫描进度与取消的观察者。
///
/// 回调可能来自 writer 线程；实现必须是 `Send + Sync`。`on_parsed` 只在文件
/// 真正走了标签解析（即增量未命中）时调用，测试用它断言增量行为。
pub trait ScanObserver: Send + Sync {
    /// 每处理完一个文件调用一次；`total` 是本轮候选文件总数。
    fn on_progress(&self, scanned: usize, total: usize, current_path: &Path);

    /// 文件被真正解析（不是增量命中）时调用。
    fn on_parsed(&self, _path: &Path) {}

    /// 返回 `true` 时扫描器停止派发新文件，并提交已解析的部分。
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 什么都不做的观察者。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopObserver;

impl ScanObserver for NoopObserver {
    fn on_progress(&self, _scanned: usize, _total: usize, _current_path: &Path) {}
}

/// 一轮扫描的统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// 处理过的候选文件数（含增量命中与失败）。
    pub scanned: usize,
    /// 真正解析并写入的曲目数。
    pub indexed: usize,
    /// 增量命中、只刷新了 `last_seen_at` 的曲目数。
    pub skipped: usize,
    /// 解析或写入失败的文件数。
    pub failed: usize,
    /// 本轮未见到的旧记录数（只统计，不删除）。
    pub missing: usize,
    /// 是否因取消提前结束。
    pub cancelled: bool,
}

/// 扫描器。借用 [`Database`] 与一个观察者。
pub struct Scanner<'a> {
    db: &'a Database,
    observer: &'a dyn ScanObserver,
    full: bool,
}

impl<'a> Scanner<'a> {
    pub fn new(db: &'a Database, observer: &'a dyn ScanObserver) -> Self {
        Self {
            db,
            observer,
            full: false,
        }
    }

    /// 忽略 `(size, mtime)` 增量指纹，全部重新解析（`library scan --full`）。
    pub fn full(mut self) -> Self {
        self.full = true;
        self
    }

    /// 扫描 `roots`（不存在的根直接跳过）。
    pub fn scan<P: AsRef<Path>>(&self, roots: &[P]) -> Result<ScanReport> {
        let scan_started_at = self.scan_started_at()?;
        let mut absolute_roots = Vec::with_capacity(roots.len());
        for root in roots {
            let root = std::path::absolute(root.as_ref())?;
            if root.is_dir() {
                absolute_roots.push(root);
            }
        }
        for root in &absolute_roots {
            repo::touch_root(self.db.conn(), root, scan_started_at)?;
        }

        let candidates = collect_candidates(&absolute_roots, self.observer);
        let total = candidates.len();
        // 全量扫描把索引清空：所有候选都会走标签解析。
        let index = if self.full {
            HashMap::new()
        } else {
            repo::load_index(self.db.conn())?
        };
        let cancelled = AtomicBool::new(false);
        let (tx, rx) = sync_channel::<ScanItem>(CHANNEL_CAPACITY);
        // 只能把路径（而不是 `&Database`）带进线程：`Connection` 不是 `Sync`。

        let report = std::thread::scope(|scope| -> Result<ScanReport> {
            let producer = {
                let candidates: &[Candidate] = &candidates;
                let index = &index;
                let cancelled = &cancelled;
                let observer = self.observer;
                scope.spawn(move || produce(candidates, index, &tx, cancelled, observer))
            };

            let result = self.write_loop(&rx, total, scan_started_at, &cancelled);
            if result.is_err() {
                // writer 提前失败时也要让生产者停下来，否则 channel 会卡死。
                cancelled.store(true, Ordering::SeqCst);
                while rx.recv().is_ok() {}
            }
            if producer.join().is_err() {
                return Err(LibraryError::WorkerPanicked);
            }
            result
        })?;

        Ok(report)
    }

    /// 扫描起点：保证严格大于库里已有的 `last_seen_at`，同一毫秒内连续扫描
    /// 也能正确判定"缺失"。
    fn scan_started_at(&self) -> Result<i64> {
        let now = now_ms();
        let max_seen: i64 = self.db.conn().query_row(
            "SELECT COALESCE(MAX(last_seen_at), 0) FROM tracks",
            [],
            |row| row.get(0),
        )?;
        Ok(now.max(max_seen.saturating_add(1)))
    }

    /// 唯一的写库循环。
    fn write_loop(
        &self,
        rx: &Receiver<ScanItem>,
        total: usize,
        scan_started_at: i64,
        cancelled: &AtomicBool,
    ) -> Result<ScanReport> {
        let mut report = ScanReport::default();
        let mut batch = WriteBatch::begin(self.db.conn())?;

        while let Ok(item) = rx.recv() {
            if self.observer.is_cancelled() {
                cancelled.store(true, Ordering::SeqCst);
            }
            report.scanned += 1;
            let path = item.path().to_path_buf();

            if !cancelled.load(Ordering::SeqCst) {
                match item {
                    ScanItem::Unchanged(_) => {
                        match batch.run(|conn| repo::mark_seen(conn, &path, scan_started_at)) {
                            Ok(()) => report.skipped += 1,
                            Err(_) => report.failed += 1,
                        }
                    }
                    ScanItem::Parsed { track } => {
                        let parsed_path = track.path.clone();
                        match batch.run(|conn| repo::upsert_track(conn, &track, scan_started_at)) {
                            Ok(()) => {
                                report.indexed += 1;
                                self.observer.on_parsed(&parsed_path);
                            }
                            Err(_) => report.failed += 1,
                        }
                    }
                    ScanItem::Failed { .. } => report.failed += 1,
                }
            }

            self.observer.on_progress(report.scanned, total, &path);
            batch.tick()?;
        }

        batch.commit()?;
        report.missing = repo::count_missing(self.db.conn(), scan_started_at)?;
        report.cancelled = cancelled.load(Ordering::SeqCst) || self.observer.is_cancelled();
        Ok(report)
    }
}

/// 生产者：遍历目录，把每个候选文件派发给 rayon 线程池。
///
/// 解析成功的文件在这里顺手把封面写入缓存（rayon 线程里做解码/缩放的
/// 重活，只有落盘加锁），写失败通过 `cover_failed` 告诉 writer 计入
/// `failed`，不影响曲目入库。
fn produce(
    candidates: &[Candidate],
    index: &HashMap<String, (i64, i64)>,
    tx: &SyncSender<ScanItem>,
    cancelled: &AtomicBool,
    observer: &dyn ScanObserver,
) {
    rayon::scope(|scope| {
        for candidate in candidates {
            if cancelled.load(Ordering::Relaxed) || observer.is_cancelled() {
                cancelled.store(true, Ordering::Relaxed);
                break;
            }
            let sender = tx.clone();
            scope.spawn(move |_| {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let unchanged = index.get(&candidate.key).is_some_and(|(size, mtime)| {
                    *size == candidate.size_bytes && *mtime == candidate.mtime_ms
                });
                let item = if unchanged {
                    ScanItem::Unchanged(candidate.path.clone())
                } else {
                    match metadata::parse_file(
                        &candidate.path,
                        candidate.size_bytes,
                        candidate.mtime_ms,
                    ) {
                        Ok(track) => ScanItem::Parsed {
                            track: Box::new(track),
                        },
                        Err(err) => ScanItem::Failed {
                            path: candidate.path.clone(),
                            message: err.to_string(),
                        },
                    }
                };
                let _ = sender.send(item);
            });
        }
    });
}

/// 候选文件及其增量指纹。
struct Candidate {
    path: PathBuf,
    key: String,
    size_bytes: i64,
    mtime_ms: i64,
}

enum ScanItem {
    Unchanged(PathBuf),
    Parsed {
        track: Box<ParsedTrack>,
    },
    Failed {
        path: PathBuf,
        /// 错误详情留给后续命令层；本卡只计数。
        #[allow(dead_code)]
        message: String,
    },
}

impl ScanItem {
    fn path(&self) -> &Path {
        match self {
            ScanItem::Unchanged(path) => path,
            ScanItem::Parsed { track, .. } => &track.path,
            ScanItem::Failed { path, .. } => path,
        }
    }
}

fn collect_candidates(roots: &[PathBuf], observer: &dyn ScanObserver) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    'roots: for root in roots {
        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_skipped(entry));
        for entry in walker.filter_map(std::result::Result::ok) {
            // 目录遍历本身可能很慢；退出应用时也要能在这里及时停下。
            if observer.is_cancelled() {
                break 'roots;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !has_audio_extension(path) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            candidates.push(Candidate {
                key: path.to_string_lossy().into_owned(),
                path: path.to_path_buf(),
                size_bytes: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                mtime_ms: mtime_ms(&metadata),
            });
        }
    }
    candidates
}

/// 跳过隐藏条目与依赖目录（ARCH-003 §4）。
fn is_skipped(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name == "node_modules"
}

fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| metadata::AUDIO_EXTENSIONS.contains(&extension.as_str()))
}

fn mtime_ms(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
