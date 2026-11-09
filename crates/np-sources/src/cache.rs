//! 下载缓存：命中判断、原子落盘后的路径与容量清理。
//!
//! 目录布局（ADR-0004 §2）：`<cache>/streams/<source>-<source_id>.<ext>`。
//! 扩展名由 MIME 决定：`audio/mp4` → `.m4a`；`video/mp4`（durl 回退）→ `.mp4`。
//!
//! 缓存命中条件是"存在且大小 > 0"；残留的 `<dest>.part` 一律当未命中并删除。
//! 容量超过 [`STREAM_CACHE_MAX_BYTES`] 时按 mtime 从旧到新删，直到低于
//! [`STREAM_CACHE_TARGET_BYTES`]。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::download::{Downloader, is_present, partial_path};
use crate::error::{Result, SourceError};
use crate::model::{ResolvedStream, SourceId};

/// 缓存目录总大小上限（ADR-0004 §2）。
pub const STREAM_CACHE_MAX_BYTES: u64 = 500 * 1024 * 1024;

/// 清理后要低于的目标大小，留出写入余量（ADR-0004 §2）。
pub const STREAM_CACHE_TARGET_BYTES: u64 = 400 * 1024 * 1024;

/// 在线音频的下载缓存。
///
/// `StreamCache` 是 `Send + Sync` 的纯数据结构，不持有播放器 / UI 状态，
/// 可以在后台线程里调用（T-0038）。
pub struct StreamCache {
    dir: PathBuf,
    downloader: Arc<dyn Downloader>,
    max_bytes: u64,
    target_bytes: u64,
}

impl StreamCache {
    /// 用默认阈值（500MB / 400MB）在 `<cache_root>/streams` 下构造。
    pub fn new(cache_root: impl Into<PathBuf>, downloader: Arc<dyn Downloader>) -> Self {
        Self::with_limits(
            cache_root,
            downloader,
            STREAM_CACHE_MAX_BYTES,
            STREAM_CACHE_TARGET_BYTES,
        )
    }

    /// 显式指定清理阈值（测试用小阈值；将来也可做成配置项）。
    pub fn with_limits(
        cache_root: impl Into<PathBuf>,
        downloader: Arc<dyn Downloader>,
        max_bytes: u64,
        target_bytes: u64,
    ) -> Self {
        Self {
            dir: cache_root.into().join("streams"),
            downloader,
            max_bytes,
            target_bytes,
        }
    }

    /// 缓存目录（`<cache_root>/streams`）。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 计算某个曲目的缓存路径（不检查文件是否存在）。
    ///
    /// `source_id` 只允许 ASCII 字母数字与 `-` / `_`，避免拼出目录穿越。
    pub fn path_for(&self, source: SourceId, source_id: &str, mime: &str) -> Result<PathBuf> {
        if source_id.is_empty()
            || !source_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(SourceError::Download(format!(
                "非法的 source_id: {source_id:?}"
            )));
        }
        let extension = extension_for_mime(mime)?;
        Ok(self.dir.join(format!("{source}-{source_id}.{extension}")))
    }

    /// 确保 `stream` 已下载到缓存，返回本地路径。
    ///
    /// - 命中（文件存在且大小 > 0）直接返回，不触网；
    /// - 未命中时删除残留 `.part`，调用 [`Downloader`] 原子落盘；
    /// - 下载成功后做一次容量清理，并保护刚下载的文件不被删掉。
    pub fn fetch(
        &self,
        source: SourceId,
        source_id: &str,
        stream: &ResolvedStream,
    ) -> Result<PathBuf> {
        let dest = self.path_for(source, source_id, &stream.mime)?;

        if is_present(&dest) {
            let _ = fs::remove_file(partial_path(&dest));
            return Ok(dest);
        }

        fs::create_dir_all(&self.dir).map_err(cache_io)?;
        let _ = fs::remove_file(partial_path(&dest));
        self.downloader.download(stream, &dest)?;
        let _ = self.cleanup_except(Some(&dest));
        Ok(dest)
    }

    /// 按 mtime 从旧到新删除，直到目录总大小低于目标值；返回释放的字节数。
    ///
    /// 目录不存在或读不到时返回 0；删除单个文件失败会跳过，不影响其它文件。
    /// 不保护任何文件（`fetch` 内部用的是带保护的重载）。
    pub fn cleanup(&self) -> Result<u64> {
        self.cleanup_except(None)
    }

    fn cleanup_except(&self, keep: Option<&Path>) -> Result<u64> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Ok(0);
        };

        let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        let mut total = 0u64;
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            total = total.saturating_add(metadata.len());
            files.push((
                entry.path(),
                metadata.len(),
                metadata.modified().unwrap_or(UNIX_EPOCH),
            ));
        }

        if total <= self.max_bytes {
            return Ok(0);
        }

        files.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));

        let mut freed = 0u64;
        for (path, len, _) in files {
            if total < self.target_bytes {
                break;
            }
            if keep.is_some_and(|keep| keep == path.as_path()) {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
                freed = freed.saturating_add(len);
            }
        }
        Ok(freed)
    }
}

/// MIME → 缓存扩展名。只接受当前播放器确定能解的 MP4 家族。
pub(crate) fn extension_for_mime(mime: &str) -> Result<&'static str> {
    let base = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/mp4" => Ok("m4a"),
        "video/mp4" => Ok("mp4"),
        _ => Err(SourceError::Unavailable(format!(
            "不支持的音频 MIME: {mime}"
        ))),
    }
}

fn cache_io(error: std::io::Error) -> SourceError {
    SourceError::Download(format!("创建缓存目录失败: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::download::HttpDownloader;
    use crate::testing::TempDir;
    use crate::transport::Transport;

    struct CountingTransport {
        body: Vec<u8>,
        calls: AtomicUsize,
    }

    impl CountingTransport {
        fn new(body: &[u8]) -> Self {
            Self {
                body: body.to_vec(),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Transport for CountingTransport {
        fn get(&self, _url: &str) -> Result<Vec<u8>> {
            Ok(self.body.clone())
        }

        fn get_to(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
            sink: &mut dyn Write,
        ) -> Result<u64> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            sink.write_all(&self.body)
                .map_err(|error| SourceError::Download(error.to_string()))?;
            Ok(self.body.len() as u64)
        }
    }

    fn stream(mime: &str) -> ResolvedStream {
        ResolvedStream {
            url: "https://example.invalid/audio".to_string(),
            mime: mime.to_string(),
            expires_at: None,
            headers: Vec::new(),
        }
    }

    fn build(
        label: &str,
        max_bytes: Option<u64>,
    ) -> (TempDir, StreamCache, Arc<CountingTransport>) {
        let temp = TempDir::new(label);
        let transport = Arc::new(CountingTransport::new(b"audio"));
        let downloader = Arc::new(HttpDownloader::new(transport.clone()));
        let cache = match max_bytes {
            Some(max) => StreamCache::with_limits(temp.path(), downloader, max, max / 2),
            None => StreamCache::new(temp.path(), downloader),
        };
        (temp, cache, transport)
    }

    fn write_file(path: &Path, bytes: &[u8], mtime_secs: u64) {
        fs::write(path, bytes).expect("写入测试文件");
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("打开测试文件");
        file.set_modified(UNIX_EPOCH + Duration::from_secs(mtime_secs))
            .expect("设置 mtime");
    }

    fn total_size(dir: &Path) -> u64 {
        fs::read_dir(dir)
            .expect("读目录")
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len())
            .sum()
    }

    #[test]
    fn fetch_downloads_once_then_hits_cache() {
        let (_temp, cache, transport) = build("cache-hit", None);
        let stream = stream("audio/mp4");

        let path = cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream)
            .expect("首次下载");
        assert_eq!(transport.calls(), 1);
        assert_eq!(fs::read(&path).expect("读取缓存"), b"audio");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("bilibili-BV1xx411c7mD.m4a")
        );

        let again = cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream)
            .expect("命中缓存");
        assert_eq!(again, path);
        assert_eq!(transport.calls(), 1, "命中不应重新下载");
    }

    #[test]
    fn stale_part_is_treated_as_miss_and_removed() {
        let (_temp, cache, transport) = build("cache-part", None);
        let stream = stream("audio/mp4");
        let dest = cache
            .path_for(SourceId::Bilibili, "BV1xx411c7mD", "audio/mp4")
            .expect("路径");
        fs::create_dir_all(dest.parent().expect("parent")).expect("create dir");
        fs::write(partial_path(&dest), b"half").expect("write part");

        let path = cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream)
            .expect("未命中应重新下载");

        assert_eq!(path, dest);
        assert_eq!(transport.calls(), 1);
        assert!(!partial_path(&dest).exists(), ".part 应被清理");
        assert_eq!(fs::read(&dest).expect("读取缓存"), b"audio");
    }

    #[test]
    fn zero_byte_cache_file_is_treated_as_miss() {
        let (_temp, cache, transport) = build("cache-empty", None);
        let stream = stream("audio/mp4");
        let dest = cache
            .path_for(SourceId::Bilibili, "BV1xx411c7mD", "audio/mp4")
            .expect("路径");
        fs::create_dir_all(dest.parent().expect("parent")).expect("create dir");
        fs::write(&dest, b"").expect("write empty");

        cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream)
            .expect("空文件应重新下载");

        assert_eq!(transport.calls(), 1);
        assert_eq!(fs::read(&dest).expect("读取缓存"), b"audio");
    }

    #[test]
    fn unsupported_mime_is_rejected_before_download() {
        let (_temp, cache, transport) = build("cache-mime", None);

        let error = cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream("audio/flac"))
            .expect_err("不支持的 MIME 应报错");

        assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
        assert_eq!(transport.calls(), 0);
    }

    #[test]
    fn invalid_source_id_cannot_escape_cache_dir() {
        let (_temp, cache, transport) = build("cache-traversal", None);

        let error = cache
            .fetch(SourceId::Bilibili, "../evil", &stream("audio/mp4"))
            .expect_err("路径穿越应被拒绝");

        assert!(matches!(error, SourceError::Download(_)), "{error:?}");
        assert_eq!(transport.calls(), 0);
    }

    #[test]
    fn video_mp4_gets_mp4_extension() {
        let (_temp, cache, _transport) = build("cache-mp4", None);
        let path = cache
            .path_for(SourceId::Bilibili, "BV1xx411c7mD", "video/mp4")
            .expect("路径");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("bilibili-BV1xx411c7mD.mp4")
        );
    }

    #[test]
    fn cleanup_removes_oldest_until_below_target() {
        let (_temp, cache, _transport) = build("cache-cleanup", Some(50));
        fs::create_dir_all(cache.dir()).expect("create dir");
        for index in 1..=6u64 {
            let path = cache.dir().join(format!("bilibili-BV{index}.m4a"));
            write_file(&path, &[b'x'; 10], index);
        }
        assert_eq!(total_size(cache.dir()), 60);

        let freed = cache.cleanup().expect("清理");

        assert_eq!(freed, 40, "60B 删到 20B 需释放 40B");
        assert!(total_size(cache.dir()) < 30);
        for index in 1..=4u64 {
            let path = cache.dir().join(format!("bilibili-BV{index}.m4a"));
            assert!(!path.exists(), "最旧的 BV{index} 应被删除");
        }
        for index in 5..=6u64 {
            let path = cache.dir().join(format!("bilibili-BV{index}.m4a"));
            assert!(path.exists(), "较新的 BV{index} 应保留");
        }
    }

    #[test]
    fn cleanup_is_noop_under_limit() {
        let (_temp, cache, _transport) = build("cache-under", Some(50));
        fs::create_dir_all(cache.dir()).expect("create dir");
        for index in 1..=2u64 {
            write_file(
                &cache.dir().join(format!("bilibili-BV{index}.m4a")),
                &[b'x'; 10],
                index,
            );
        }

        assert_eq!(cache.cleanup().expect("清理"), 0);
        assert_eq!(total_size(cache.dir()), 20);
    }

    #[test]
    fn fetch_keeps_just_downloaded_file_when_over_limit() {
        let (_temp, cache, transport) = build("cache-keep", Some(1));
        cache
            .fetch(SourceId::Bilibili, "BV1xx411c7mD", &stream("audio/mp4"))
            .expect("下载成功");

        assert_eq!(transport.calls(), 1);
        let dest = cache
            .path_for(SourceId::Bilibili, "BV1xx411c7mD", "audio/mp4")
            .expect("路径");
        assert!(dest.is_file(), "刚下载的文件不能被清理掉");
    }

    #[test]
    fn cache_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StreamCache>();
    }
}
