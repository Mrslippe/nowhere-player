//! 可注入的 HTTP 传输层。
//!
//! 测试注入假实现返回夹具；生产实现见 [`ReqwestTransport`]。

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Result, SourceError};

/// 固定桌面 UA（ADR-0004 §4：不随版本变）。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// 连接超时（ADR-0004 §4）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 整体超时（ADR-0004 §4）。
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 同一源两次请求的最小间隔（ADR-0004 §4）。
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(300);
/// 重试前的等待。
const RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// 最小 HTTP GET 抽象（ADR-0004 §5）。
///
/// 返回响应体字节。非 2xx 状态由实现映射为 [`crate::SourceError::Http`] 或
/// [`crate::SourceError::Unavailable`]；测试实现可以直接返回错误，
/// 从而完全不触网。
pub trait Transport: Send + Sync {
    /// 请求 `url` 并返回响应体。
    fn get(&self, url: &str) -> Result<Vec<u8>>;
}

/// 基于 `reqwest::blocking` 的生产传输层。
///
/// - 无 cookie jar（不启用 reqwest 的 cookies feature）；
/// - 连接 5s / 整体 15s 超时；
/// - 请求节流 300ms，5xx 与连接错误重试 1 次；
/// - 只在生产路径使用；测试注入夹具，不会构造它，也就不会拉起运行时。
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
    last_request: Mutex<Option<Instant>>,
}

impl ReqwestTransport {
    /// 构造客户端。UA / 超时在这里固定，不随调用方变化。
    pub fn new() -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TOTAL_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| SourceError::Network(format!("HTTP 客户端初始化失败: {error}")))?;
        Ok(Self {
            client,
            last_request: Mutex::new(None),
        })
    }

    fn throttle(&self) {
        let mut last_request = self
            .last_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = *last_request {
            let elapsed = previous.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                thread::sleep(MIN_REQUEST_INTERVAL - elapsed);
            }
        }
        *last_request = Some(Instant::now());
    }

    fn get_once(&self, url: &str) -> Result<Vec<u8>> {
        self.throttle();
        let response = self.client.get(url).send().map_err(map_reqwest_error)?;
        classify_status(response.status().as_u16())?;
        response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(map_reqwest_error)
    }
}

impl Transport for ReqwestTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        match self.get_once(url) {
            Err(error @ (SourceError::Network(_) | SourceError::Http(500..=599))) => {
                thread::sleep(RETRY_BACKOFF);
                match self.get_once(url) {
                    Ok(body) => Ok(body),
                    Err(_) => Err(error),
                }
            }
            result => result,
        }
    }
}

/// 把 HTTP 状态码分成"成功 / 源不可用 / HTTP 错误"。
fn classify_status(status: u16) -> Result<()> {
    match status {
        200..=299 => Ok(()),
        412 => Err(SourceError::Unavailable(format!(
            "HTTP {status}: B 站风控拦截"
        ))),
        403 => Err(SourceError::Unavailable(format!(
            "HTTP {status}: 签名或权限被拒绝"
        ))),
        _ => Err(SourceError::Http(status)),
    }
}

fn map_reqwest_error(error: reqwest::Error) -> SourceError {
    if error.is_timeout() {
        SourceError::Network(format!("请求超时: {error}"))
    } else if error.is_connect() {
        SourceError::Network(format!("连接失败: {error}"))
    } else {
        SourceError::Network(format!("请求失败: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_accepts_2xx() {
        assert!(classify_status(200).is_ok());
        assert!(classify_status(204).is_ok());
    }

    #[test]
    fn classify_maps_http_500_to_http_variant() {
        assert!(matches!(classify_status(500), Err(SourceError::Http(500))));
        assert!(matches!(classify_status(404), Err(SourceError::Http(404))));
    }

    #[test]
    fn classify_maps_risk_control_to_unavailable() {
        assert!(matches!(
            classify_status(412),
            Err(SourceError::Unavailable(_))
        ));
        assert!(matches!(
            classify_status(403),
            Err(SourceError::Unavailable(_))
        ));
    }
}
