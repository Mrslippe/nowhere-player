//! 在线音源适配层。
//!
//! 边界见 ADR-0004：本 crate 只负责搜索与取流地址，不碰播放、不碰命令层。
//! 所有网络访问都经过可注入的 [`Transport`]，因此测试可以完全离线。

pub mod artist;
pub mod wbi;

mod bilibili;
mod cache;
mod download;
mod error;
mod model;
mod netease;
mod registry;
mod transport;

#[cfg(test)]
mod testing;

pub use artist::{
    MatchKind, ResolvedUp, build_query, match_artist_name, matches_artist, matches_artist_in_query,
    resolve_up_user, title_has_all_keywords,
};
pub use bilibili::BilibiliSource;
pub use cache::{STREAM_CACHE_MAX_BYTES, STREAM_CACHE_TARGET_BYTES, StreamCache};
pub use download::{DownloadOutcome, Downloader, HttpDownloader};
pub use error::{Result, SourceError};
pub use model::{MAX_TRACK_DURATION_MS, ResolvedStream, SourceId, SourceTrack, UpUser};
pub use netease::NeteaseSource;
pub use registry::{SearchMerge, SourceRegistry};
pub use transport::{HttpResponse, ReqwestTransport, Transport};

/// 在线音源适配器（ADR-0004 §3）。
pub trait AudioSource: Send + Sync {
    /// 该源标识。
    fn id(&self) -> SourceId;

    /// 搜索，最多返回 `limit` 条。
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SourceTrack>>;

    /// 取可下载地址（可能在几分钟后过期）。
    fn resolve(&self, track: &SourceTrack) -> Result<ResolvedStream>;

    /// 按用户名搜索 UP（T-0049）；默认不支持。
    fn search_up_users(&self, _name: &str) -> Result<Vec<UpUser>> {
        Err(SourceError::Unavailable(
            "当前音源不支持 UP 检索".to_string(),
        ))
    }

    /// 列某个 UP 的投稿；`keyword` 为空时取最近一页（T-0049）。
    fn user_archives(
        &self,
        _mid: u64,
        _name: &str,
        _keyword: &str,
        _limit: usize,
    ) -> Result<Vec<SourceTrack>> {
        Err(SourceError::Unavailable(
            "当前音源不支持 UP 投稿检索".to_string(),
        ))
    }
}
