//! 可注入的 HTTP 传输层。
//!
//! 测试注入假实现返回夹具；生产实现见 `ReqwestTransport`。

use crate::error::Result;

/// 最小 HTTP GET 抽象（ADR-0004 §5）。
///
/// 返回响应体字节。非 2xx 状态由实现映射为 [`crate::SourceError::Http`] 或
/// [`crate::SourceError::Unavailable`]；测试实现可以直接返回错误，
/// 从而完全不触网。
pub trait Transport: Send + Sync {
    /// 请求 `url` 并返回响应体。
    fn get(&self, url: &str) -> Result<Vec<u8>>;
}
