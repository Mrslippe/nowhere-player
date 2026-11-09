//! 在线音源适配层。
//!
//! 边界见 ADR-0004：本 crate 只负责搜索与取流地址，不碰播放、不碰命令层。
//! 所有网络访问都经过可注入的 [`Transport`]，因此测试可以完全离线。

pub mod wbi;

mod bilibili;
mod cache;
mod download;
mod error;
mod model;
mod transport;

#[cfg(test)]
mod testing;

pub use bilibili::BilibiliSource;
pub use cache::{STREAM_CACHE_MAX_BYTES, STREAM_CACHE_TARGET_BYTES, StreamCache};
pub use download::{DownloadOutcome, Downloader, HttpDownloader};
pub use error::{Result, SourceError};
pub use model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack};
pub use transport::{ReqwestTransport, Transport};

/// 在线音源适配器（ADR-0004 §3）。
pub trait AudioSource: Send + Sync {
    /// 该源标识。
    fn id(&self) -> SourceId;

    /// 搜索，最多返回 `limit` 条。
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SourceTrack>>;

    /// 取可下载地址（可能在几分钟后过期）。
    fn resolve(&self, track: &SourceTrack) -> Result<ResolvedStream>;
}
