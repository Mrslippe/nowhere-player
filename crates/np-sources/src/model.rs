//! 音源无关的领域模型（ADR-0004 §3）。

use std::time::SystemTime;

/// 在线音源标识。
///
/// 与目录名 / 命令层展示的短名一致：`netease` / `bilibili` / `ytdlp`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceId {
    /// 网易云（匿名）。
    Netease,
    /// B 站。
    Bilibili,
    /// yt-dlp 子进程。
    Ytdlp,
}

impl SourceId {
    /// 稳定短名，用于错误信息、缓存文件名与 UI 来源标记。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Netease => "netease",
            Self::Bilibili => "bilibili",
            Self::Ytdlp => "ytdlp",
        }
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 在线曲目。
///
/// 与本地 `Track` 刻意不同：没有 `path`，用 `(source, source_id)` 二元组定位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTrack {
    /// 来源。
    pub source: SourceId,
    /// 源内 ID（B 站为 `BV` 号，取不到时退回 `aid`）。
    pub source_id: String,
    /// 标题（已去掉搜索结果里的高亮标签）。
    pub title: String,
    /// 作者 / UP 主。
    pub artist: String,
    /// 时长，毫秒。搜索阶段就过滤掉无法解析时长的条目。
    pub duration_ms: u64,
    /// 专辑（B 站没有，留 `None`）。
    pub album: Option<String>,
    /// 封面地址（B 站为 `https://i0.hdslb.com/...`）。
    pub cover_url: Option<String>,
}

/// 可直接交给下载器的地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStream {
    /// 可下载的 URL（可能几分钟后过期）。
    pub url: String,
    /// MIME 类型，如 `audio/mp4`。
    pub mime: String,
    /// 过期时间；`None` 表示不适用（如 yt-dlp 直链）。
    pub expires_at: Option<SystemTime>,
}
