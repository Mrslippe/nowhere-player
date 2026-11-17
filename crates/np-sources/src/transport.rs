//! 可注入的 HTTP 传输层。
//!
//! 测试注入假实现返回夹具；生产实现见 [`ReqwestTransport`]。

use std::io::{Read, Write};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Result, SourceError};

/// 固定桌面 UA（ADR-0004 §4：不随版本变）。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// 连接超时（ADR-0004 §4）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 整体超时（ADR-0004 §4），用于 API 请求。
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 下载是整段文件（缓存上限 500MB），沿用 15s 会误杀大文件；
/// 单独放宽到 10 分钟。API 请求仍走 `TOTAL_TIMEOUT`。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 同一源两次请求的最小间隔（ADR-0004 §4）。
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(300);
/// 重试前的等待。
const RETRY_BACKOFF: Duration = Duration::from_millis(500);

/// 一次 HTTP 响应的窄视图（T-0092a）。
///
/// 只保留调用方需要的状态、响应头与完整响应体。响应头保留原始顺序与重复项
/// （例如多个 `Set-Cookie`），名称大小写由服务端决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头 `(name, value)`，顺序与线上一致。
    pub headers: Vec<(String, String)>,
    /// 完整响应体。
    pub body: Vec<u8>,
}

/// 最小 HTTP GET 抽象（ADR-0004 §5）。
///
/// 返回响应体字节。非 2xx 状态由实现映射为 [`crate::SourceError::Http`] 或
/// [`crate::SourceError::Unavailable`]；测试实现可以直接返回错误，
/// 从而完全不触网。
pub trait Transport: Send + Sync {
    /// 请求 `url` 并返回响应体。
    fn get(&self, url: &str) -> Result<Vec<u8>>;

    /// 带额外请求头请求 `url`，返回状态、响应头与响应体（T-0092a）。
    ///
    /// 默认实现回退到 [`Transport::get`]：忽略 `headers`，把结果包装成
    /// `status = 200`、`headers` 为空的 [`HttpResponse`]。这样旧测试假实现无需
    /// 修改；生产实现应真正发送 `headers` 并回传线上状态和响应头。
    ///
    /// 与 [`Transport::get`] 不同，这个窄接口**不**把非 2xx 映射成错误，调用方
    /// 需要自己检查 [`HttpResponse::status`]，以便读取登录接口的响应头。
    fn get_with_headers(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse> {
        let _ = headers;
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: self.get(url)?,
        })
    }

    /// 带额外请求头把响应体写入 `sink`，返回写入的字节数。
    ///
    /// 默认实现调用 [`Transport::get`] 后整块写入（忽略 `headers`），
    /// 让测试假实现保持简单；生产实现应流式写入，并把 `headers`
    /// 真正发出去（B 站 CDN 缺 `Referer` 会 403）。
    fn get_to(&self, url: &str, headers: &[(&str, &str)], sink: &mut dyn Write) -> Result<u64> {
        let _ = headers;
        let body = self.get(url)?;
        sink.write_all(&body)
            .map_err(|error| SourceError::Download(format!("写入下载文件失败: {error}")))?;
        Ok(body.len() as u64)
    }

    /// 打开一条流式 HTTP 连接（ADR-0008 §4.2）。
    ///
    /// 返回的 reader 没有总时限，供直播这类无限长的响应使用；默认实现
    /// 明确返回 `NotImplemented`，旧测试假实现不需要改。
    fn open_stream(&self, _url: &str, _headers: &[(&str, &str)]) -> Result<Box<dyn Read + Send>> {
        Err(SourceError::NotImplemented(
            "当前传输层不支持流式读取".to_owned(),
        ))
    }
}

/// 基于 `reqwest::blocking` 的生产传输层。
///
/// - 无 cookie jar（不启用 reqwest 的 cookies feature）；
/// - 连接 5s / API 整体 15s 超时；下载是整段文件，单独放宽到 10 分钟；
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

    fn get_with_headers(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse> {
        self.throttle();
        let mut request = self.client.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().map_err(map_reqwest_error)?;
        let status = response.status().as_u16();
        let response_headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(map_reqwest_error)?;
        Ok(HttpResponse {
            status,
            headers: response_headers,
            body,
        })
    }

    fn get_to(&self, url: &str, headers: &[(&str, &str)], sink: &mut dyn Write) -> Result<u64> {
        self.throttle();
        let mut request = self.client.get(url).timeout(DOWNLOAD_TIMEOUT);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let mut response = request.send().map_err(map_reqwest_error)?;
        classify_status(response.status().as_u16())?;
        let expected = response.content_length();
        let copied = std::io::copy(&mut response, sink)
            .map_err(|error| SourceError::Download(format!("写入下载文件失败: {error}")))?;
        if let Some(expected) = expected
            && copied != expected
        {
            return Err(SourceError::Download(format!(
                "下载不完整: 收到 {copied} / {expected} 字节"
            )));
        }
        Ok(copied)
    }

    fn open_stream(&self, url: &str, headers: &[(&str, &str)]) -> Result<Box<dyn Read + Send>> {
        // 单独建客户端：`self.client` 带 15s 总超时，直播会被误杀。
        // 只保留 5s 连接超时；响应体由调用方按需读取，不做总时限。
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(None::<Duration>)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| SourceError::Network(format!("直播客户端初始化失败: {error}")))?;
        self.throttle();
        let mut request = client.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().map_err(map_reqwest_error)?;
        classify_status(response.status().as_u16())?;
        Ok(Box::new(response))
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

    struct BodyOnlyTransport;

    impl Transport for BodyOnlyTransport {
        fn get(&self, _url: &str) -> Result<Vec<u8>> {
            Ok(b"body".to_vec())
        }
    }

    #[test]
    fn get_with_headers_defaults_to_get_without_headers() {
        let transport = BodyOnlyTransport;
        let response = transport
            .get_with_headers("https://example.invalid/response", &[("X-Test", "1")])
            .expect("默认实现应回退到 get");
        assert_eq!(response.status, 200);
        assert!(response.headers.is_empty());
        assert_eq!(response.body, b"body");
    }

    #[test]
    fn open_stream_defaults_to_not_implemented() {
        let transport = BodyOnlyTransport;
        assert!(matches!(
            transport.open_stream("https://example.invalid/stream.flv", &[]),
            Err(SourceError::NotImplemented(_))
        ));
    }
}
