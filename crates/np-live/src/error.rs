//! 直播错误：每一层都映射成可读状态，不把 reqwest / symphonia 的原文直接甩给 UI。

use np_sources::SourceError;

/// `np-live` 的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    /// 房间号不是正整数。
    #[error("房间号无效：{0}")]
    InvalidRoom(String),

    /// B 站接口返回非 0 `code`（如 60004 房间不存在）。
    #[error("B 站接口错误（code={code}）：{message}")]
    Api { code: i64, message: String },

    /// `live_status != 1`。
    #[error("直播间未开播")]
    Offline,

    /// 已开播但 playurl 里没有 `http_stream/flv`。
    #[error("直播间没有可用的 FLV 音频流")]
    NoStream,

    /// API / 连接层失败。
    #[error("网络请求失败：{0}")]
    Source(#[from] SourceError),

    /// FLV 头、tag 或 JSON 结构不符合预期。
    #[error("直播流数据无效：{0}")]
    BadStream(String),

    /// 音频不是可处理的 AAC（AOT 1–4）。
    #[error("不支持的直播音频：{0}")]
    UnsupportedAudio(String),

    /// 准备阶段被 `Esc` 取消。
    #[error("直播已取消")]
    Cancelled,

    /// 读取直播流时的 I/O 错误。
    #[error("读取直播流失败：{0}")]
    Io(#[from] std::io::Error),
}

impl LiveError {
    /// 供 TUI 状态栏显示的一行文本。
    pub fn user_message(&self) -> String {
        self.to_string()
    }

    /// 这类错误能否通过重新取 playurl 重连恢复。
    ///
    /// 未开播/房间不存在/不支持的音频/用户取消不重试；
    /// 网络、读取、FLV 数据错误可以重试。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LiveError::Source(_) | LiveError::Io(_) | LiveError::BadStream(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_and_stream_errors_are_retryable() {
        assert!(LiveError::BadStream("x".to_owned()).is_retryable());
        assert!(LiveError::Io(std::io::Error::other("x")).is_retryable());
        assert!(!LiveError::Offline.is_retryable());
        assert!(!LiveError::Cancelled.is_retryable());
        assert!(!LiveError::UnsupportedAudio("x".to_owned()).is_retryable());
    }
}
