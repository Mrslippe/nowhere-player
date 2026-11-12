use super::helpers::*;
use super::model::*;
use super::online::OnlineService;
use super::settings::KNOWN_SOURCES;
use super::*;

/// 把后台扫描的进度写到 `terminal:block`，同一个 `blockId` 原地更新。
pub struct EventObserver {
    block_id: String,
    input: String,
    started_at: u64,
    origin_ms: u64,
    emit: Arc<EmitFn>,
    clock: Arc<ClockFn>,
    throttle: Mutex<TimeThrottle>,
}

impl EventObserver {
    pub fn with_port(
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
pub struct LibraryHandle {
    inner: Arc<LibraryInner>,
    pub(crate) online: Arc<OnlineService>,
    settings: Arc<SettingsHandle>,
}

impl LibraryHandle {
    #[cfg(test)]
    pub fn new(db_path: PathBuf) -> Self {
        let cache_root = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_cache_root(db_path, cache_root)
    }

    /// 显式注入缓存根（生产用 `app_cache_dir()`，测试用临时目录）。
    #[cfg(test)]
    pub fn with_cache_root(db_path: PathBuf, cache_root: PathBuf) -> Self {
        let config_dir = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_config_dir(db_path, cache_root, config_dir)
    }

    /// 生产入口：三个目录由宿主注入。
    pub fn with_paths(paths: crate::Paths) -> Self {
        let _purged = purge_legacy_covers(&paths.data_dir.join("covers"));
        let settings = Arc::new(SettingsHandle::load(paths.config_dir));
        let enabled = settings.enabled_source_ids(&KNOWN_SOURCES);
        Self {
            inner: Arc::new(LibraryInner {
                db_path: paths.data_dir.join("library.db"),
                db: Mutex::new(None),
                scan: Mutex::new(None),
                hits: Mutex::new(Vec::new()),
                active_listing: Mutex::new(None),
                library_listing: Mutex::new(Vec::new()),
            }),
            online: Arc::new(OnlineService::new(paths.cache_dir, enabled)),
            settings,
        }
    }

    /// 兼容旧入口：缓存根 + 设置目录由 setup 注入。
    pub fn with_config_dir(db_path: PathBuf, cache_root: PathBuf, config_dir: PathBuf) -> Self {
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
            online: Arc::new(OnlineService::new(cache_root, enabled)),
            settings,
        }
    }

    /// 命令层持有的设置 store。
    pub fn settings(&self) -> &SettingsHandle {
        &self.settings
    }

    /// 命令层持有的在线音源服务（与媒体库同生命周期）。
    pub fn online(&self) -> Arc<dyn OnlineApi> {
        Arc::clone(&self.online) as Arc<dyn OnlineApi>
    }

    /// 退出时取消进行中的下载，并打断后台扫描、等它提交已入库部分。
    pub fn shutdown(&self) {
        self.online.cancel_all();
        if let Ok(mut slot) = self.inner.scan.lock()
            && let Some(job) = slot.take()
        {
            job.cancel.store(true, Ordering::SeqCst);
            let _ = job.join.join();
        }
    }

    /// 按绝对路径精确查一首曲目；本地库未命中时回退到在线会话映射。
    pub fn track_by_path(&self, path: &str) -> Result<Option<Track>, CmdError> {
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
