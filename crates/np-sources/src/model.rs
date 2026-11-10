//! 音源无关的领域模型（ADR-0004 §3）。

use std::time::SystemTime;

/// 单曲时长上限：超过 30 分钟不进入搜索结果、也不下载。
///
/// B 站能搜到 48 小时视频，音频可能是 GB 级，500MB 缓存策略扛不住。
pub const MAX_TRACK_DURATION_MS: u64 = 30 * 60 * 1000;

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

/// B 站用户名搜索命中的一个 UP（T-0049）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpUser {
    /// 空间 mid。
    pub mid: u64,
    /// 用户名。
    pub name: String,
    /// 粉丝数。
    pub fans: u64,
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
    /// 下载该 URL 时必须附带的请求头，由源自己声明（如 B 站 CDN 的
    /// `Referer` / `Origin`）；下载器不再知道任何源的特殊 header。
    pub headers: Vec<(String, String)>,
}
