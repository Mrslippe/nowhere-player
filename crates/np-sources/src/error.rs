//! 音源适配层的错误类型。
//!
//! 变体对应 ADR-0004 §6 的降级语义：单个源失败时调用方把该源结果视为空并给
//! hint；三个源都失败时汇总为 `E_SRC_UNAVAILABLE`；取流地址尚未实现时返回
//! `E_SRC_NOT_IMPLEMENTED`。

/// `np-sources` 的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// 连接失败、超时等传输层错误（可重试）。
    #[error("网络错误: {0}")]
    Network(String),

    /// HTTP 非成功状态码。
    #[error("HTTP 错误: {0}")]
    Http(u16),

    /// 响应不是预期 JSON，或字段结构不符。
    #[error("响应解析失败: {0}")]
    Parse(String),

    /// 源不可用：风控拦截、签名被拒、接口 code 非 0 等。
    #[error("音源不可用: {0}")]
    Unavailable(String),

    /// 下载 / 落盘失败（半截 `.part` 会被清理）。
    #[error("下载失败: {0}")]
    Download(String),

    /// 占位：功能留待后续任务卡实现。
    #[error("尚未实现: {0}")]
    NotImplemented(String),
}

impl SourceError {
    /// CLI 层使用的错误码。
    ///
    /// 只用已冻结文档出现过的码：`E_NET_TIMEOUT`、`E_SRC_UNAVAILABLE`、
    /// `E_SRC_DOWNLOAD`、`E_SRC_NOT_IMPLEMENTED`。`Http` / `Parse` 归入
    /// `E_SRC_UNAVAILABLE`（该源不可用，换源或重试即可）。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Network(_) => "E_NET_TIMEOUT",
            Self::Http(_) | Self::Parse(_) | Self::Unavailable(_) => "E_SRC_UNAVAILABLE",
            Self::Download(_) => "E_SRC_DOWNLOAD",
            Self::NotImplemented(_) => "E_SRC_NOT_IMPLEMENTED",
        }
    }
}

/// crate 内部使用的 `Result` 别名。
pub type Result<T> = std::result::Result<T, SourceError>;
