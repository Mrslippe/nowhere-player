//! 把 [`ResolvedStream`] 落盘为本地文件。
//!
//! 边界：下载是同步阻塞的，只占用调用线程，不持有任何 UI / 播放器状态。
//! T-0038 可以直接把 [`Downloader::download`] 放进后台线程。
//!
//! 写入是原子的：先写 `<dest>.part`，成功后再 `rename` 到 `dest`；失败时
//! 删除 `.part`（与 `np-library` 的封面缓存同一套做法）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, SourceError};
use crate::model::ResolvedStream;
use crate::transport::Transport;

/// 一次下载的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// 目标文件已存在且非空，没有重新下载。
    AlreadyPresent,
    /// 已下载并原子落盘；`bytes` 是最终文件大小。
    Downloaded {
        /// 写入的字节数。
        bytes: u64,
    },
}

/// 下载器。实现必须 `Send + Sync`，且不得持有 UI / 播放器状态，
/// 以便调用方把它放到后台线程（T-0038）。
pub trait Downloader: Send + Sync {
    /// 把 `stream` 下载到 `dest`；父目录不存在时创建。
    fn download(&self, stream: &ResolvedStream, dest: &Path) -> Result<DownloadOutcome>;
}

/// 通过注入的 [`Transport`] 下载。
///
/// 生产注入 [`crate::ReqwestTransport`]（流式写盘）；测试注入返回夹具字节
/// 的假实现，因此下载逻辑可以完全离线测试。
pub struct HttpDownloader {
    transport: Arc<dyn Transport>,
}

impl HttpDownloader {
    /// 用给定传输层构造。
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }
}

impl Downloader for HttpDownloader {
    fn download(&self, stream: &ResolvedStream, dest: &Path) -> Result<DownloadOutcome> {
        if is_present(dest) {
            return Ok(DownloadOutcome::AlreadyPresent);
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(download_io)?;
        }

        let part = partial_path(dest);
        let _ = fs::remove_file(&part);

        let result = write_atomically(self.transport.as_ref(), stream, &part, dest);
        if result.is_err() {
            // 失败时不能留下半截文件，否则下次会被当成缓存。
            let _ = fs::remove_file(&part);
        }
        result
    }
}

fn write_atomically(
    transport: &dyn Transport,
    stream: &ResolvedStream,
    part: &Path,
    dest: &Path,
) -> Result<DownloadOutcome> {
    // 源自己声明下载所需的 header；下载器不认识任何具体的源。
    let headers: Vec<(&str, &str)> = stream
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let mut file = fs::File::create(part).map_err(download_io)?;
    let written = transport.get_to(&stream.url, &headers, &mut file)?;
    file.flush().map_err(download_io)?;
    let bytes = file.metadata().map(|meta| meta.len()).unwrap_or(written);
    drop(file);

    fs::rename(part, dest).map_err(download_io)?;
    Ok(DownloadOutcome::Downloaded { bytes })
}

pub(crate) fn is_present(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// `<dest>.part`。用 `OsString` 拼接，避免非 UTF-8 路径被 lossy 转换。
pub(crate) fn partial_path(dest: &Path) -> PathBuf {
    let mut part = dest.as_os_str().to_os_string();
    part.push(".part");
    PathBuf::from(part)
}

fn download_io(error: std::io::Error) -> SourceError {
    SourceError::Download(format!("写入下载文件失败: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::testing::TempDir;

    struct RecordingTransport {
        body: Vec<u8>,
        fail_after: Option<usize>,
        calls: Mutex<Vec<Vec<(String, String)>>>,
    }

    impl RecordingTransport {
        fn new(body: &[u8], fail_after: Option<usize>) -> Self {
            Self {
                body: body.to_vec(),
                fail_after,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Vec<(String, String)>> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl Transport for RecordingTransport {
        fn get(&self, _url: &str) -> Result<Vec<u8>> {
            Ok(self.body.clone())
        }

        fn get_to(
            &self,
            _url: &str,
            headers: &[(&str, &str)],
            sink: &mut dyn Write,
        ) -> Result<u64> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(
                    headers
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                        .collect(),
                );
            let written = self
                .fail_after
                .map_or(self.body.len(), |after| after.min(self.body.len()));
            sink.write_all(&self.body[..written])
                .map_err(|error| SourceError::Download(error.to_string()))?;
            if self.fail_after.is_some() {
                return Err(SourceError::Download("模拟传输中断".to_string()));
            }
            Ok(self.body.len() as u64)
        }
    }

    fn stream(headers: Vec<(String, String)>) -> ResolvedStream {
        ResolvedStream {
            url: "https://example.invalid/audio.m4a".to_string(),
            mime: "audio/mp4".to_string(),
            expires_at: None,
            headers,
        }
    }

    fn downloader(
        body: &[u8],
        fail_after: Option<usize>,
    ) -> (HttpDownloader, Arc<RecordingTransport>) {
        let transport = Arc::new(RecordingTransport::new(body, fail_after));
        (HttpDownloader::new(transport.clone()), transport)
    }

    #[test]
    fn download_writes_file_atomically_with_cdn_headers() {
        let temp = TempDir::new("download-ok");
        let (downloader, transport) = downloader(b"aac audio bytes", None);
        let dest = temp.path().join("streams/bilibili-BV1xx411c7mD.m4a");
        let stream = stream(vec![(
            "Referer".to_string(),
            "https://example.invalid/ref".to_string(),
        )]);

        let outcome = downloader.download(&stream, &dest).expect("下载成功");

        assert_eq!(
            outcome,
            DownloadOutcome::Downloaded {
                bytes: b"aac audio bytes".len() as u64
            }
        );
        assert_eq!(fs::read(&dest).expect("读取下载文件"), b"aac audio bytes");
        assert!(!partial_path(&dest).exists(), ".part 不应残留");

        // 下载器只透传 stream.headers，不再硬编码任何源的特殊 header。
        assert_eq!(
            transport.calls()[0],
            vec![(
                "Referer".to_string(),
                "https://example.invalid/ref".to_string()
            )]
        );
    }

    #[test]
    fn failed_download_removes_partial_file() {
        let temp = TempDir::new("download-fail");
        let (downloader, _transport) = downloader(b"0123456789", Some(3));
        let dest = temp.path().join("streams/bilibili-BV1xx411c7mD.m4a");

        let error = downloader
            .download(&stream(Vec::new()), &dest)
            .expect_err("应失败");

        assert!(matches!(error, SourceError::Download(_)), "{error:?}");
        assert_eq!(error.code(), "E_SRC_DOWNLOAD");
        assert!(!dest.exists(), "失败不应留下目标文件");
        assert!(!partial_path(&dest).exists(), "失败应删除 .part");
    }

    #[test]
    fn existing_file_is_not_downloaded_again() {
        let temp = TempDir::new("download-present");
        let (downloader, transport) = downloader(b"new", None);
        let dest = temp.path().join("streams/bilibili-BV1xx411c7mD.m4a");
        fs::create_dir_all(dest.parent().expect("parent")).expect("create dir");
        fs::write(&dest, b"old").expect("write existing");

        let outcome = downloader
            .download(&stream(Vec::new()), &dest)
            .expect("应命中");

        assert_eq!(outcome, DownloadOutcome::AlreadyPresent);
        assert_eq!(fs::read(&dest).expect("读取"), b"old");
        assert!(transport.calls().is_empty(), "命中时不应请求网络");
    }

    #[test]
    fn zero_byte_target_is_overwritten() {
        let temp = TempDir::new("download-empty");
        let (downloader, _transport) = downloader(b"fresh", None);
        let dest = temp.path().join("streams/bilibili-BV1xx411c7mD.m4a");
        fs::create_dir_all(dest.parent().expect("parent")).expect("create dir");
        fs::write(&dest, b"").expect("write empty");

        let outcome = downloader
            .download(&stream(Vec::new()), &dest)
            .expect("应重新下载");

        assert_eq!(
            outcome,
            DownloadOutcome::Downloaded {
                bytes: b"fresh".len() as u64
            }
        );
        assert_eq!(fs::read(&dest).expect("读取"), b"fresh");
    }

    #[test]
    fn downloader_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HttpDownloader>();
    }
}
