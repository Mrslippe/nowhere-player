use super::settings::KNOWN_SOURCES;
use super::*;

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
pub struct OnlineService {
    cache_root: PathBuf,
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
    pub fn new(cache_root: PathBuf, enabled: HashSet<SourceId>) -> Self {
        Self {
            cache_root,
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
    pub(crate) fn from_parts(registry: SourceRegistry, cache: StreamCache) -> Self {
        let cache_root = cache
            .dir()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Self {
            cache_root,
            runtime: OnceLock::new(),
            fixed: Some(OnlineRuntime {
                registry: SourceRegistry::new(registry.sources().to_vec()),
                cache,
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

    /// 下载/命中缓存后登记远程元数据；会话级，不持久化。
    pub(crate) fn remember_remote(&self, path: &Path, track: &SourceTrack) {
        let meta = RemoteMeta {
            source: track.source.as_str().to_owned(),
            source_id: track.source_id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            duration_ms: track.duration_ms,
        };
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path.to_path_buf(), meta);
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
    pub fn track_by_path(&self, path: &Path) -> Option<Track> {
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
        _runtime: &OnlineRuntime,
        track: &SourceTrack,
        path: PathBuf,
        bytes: u64,
        cached: bool,
    ) -> PreparedTrack {
        self.remember_remote(&path, track);
        self.prune_remote_meta();
        PreparedTrack {
            path,
            bytes,
            cached,
        }
    }

    /// 已知源 id（含被禁用的）。
    pub fn known_sources(&self) -> Vec<SourceId> {
        self.all_ids.clone()
    }

    /// 进程内启停一个源；不影响已在进行的下载。
    pub fn set_source_enabled(&self, source: SourceId, enabled: bool) -> Result<(), CmdError> {
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

    pub fn cache_usage(&self) -> Result<CacheUsage, CmdError> {
        let (streams_files, streams_bytes) = dir_usage(&self.cache_root.join("streams"));
        Ok(CacheUsage {
            streams_files,
            streams_bytes,
        })
    }

    pub fn clear_cache(&self, target: CacheTarget) -> Result<u64, CmdError> {
        let mut released = 0u64;
        if target.includes_streams() {
            released = released.saturating_add(clear_dir_files(&self.cache_root.join("streams")));
        }
        Ok(released)
    }

    #[cfg(test)]
    pub(crate) fn remote_meta_len(&self) -> usize {
        self.remote_meta
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// 退出时置位；正在写的下载会在下一个 chunk 失败并清理 `.part`。
    pub fn cancel_all(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    #[cfg(test)]
    pub(crate) fn track_lookups(&self) -> usize {
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
