//! B 站直播弹幕发送（T-0094）。
//!
//! 只负责把一条弹幕通过 [`np_sources::Transport::post_form`] 发到
//! `POST https://api.live.bilibili.com/msg/send`，并提供 5 秒本地限速与错误映射；
//! 输入框、快捷键和本地回显留在 `np-tui`。
//!
//! 不打印 cookie、csrf 或请求体；[`DanmakuError`] 只携带 HTTP 状态码、服务端
//! 业务消息或固定说明，不携带凭证值。

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use np_sources::{SourceError, Transport};
use serde::Deserialize;

use crate::auth::BiliAuth;

/// 弹幕发送接口。
const SEND_URL: &str = "https://api.live.bilibili.com/msg/send";
/// 发送弹幕时带上直播页来源，更接近浏览器请求。
const REFERER: &str = "https://live.bilibili.com/";
/// 同一进程内两次发送弹幕的最小间隔。
pub const DANMAKU_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// 进程级发送时间戳：弹幕接口按账号限频，调用方不需要自己维护状态。
static LAST_SEND: Mutex<Option<Instant>> = Mutex::new(None);

/// 弹幕发送错误。
///
/// 只包含状态码、服务端业务消息或固定说明；不携带 cookie、csrf、请求体或
/// reqwest 的原始错误文本（后者可能带 URL）。
#[derive(Debug, thiserror::Error)]
pub enum DanmakuError {
    /// 未登录，或登录凭证缺少 `SESSDATA` / `bili_jct`。
    #[error("请先登录 B 站账号")]
    NotLoggedIn,

    /// 服务端 code `-111`：csrf 校验失败。
    #[error("登录凭证已失效，请重新登录")]
    CsrfInvalid,

    /// HTTP 412 或服务端风控 code。
    #[error("B 站风控拦截，请稍后再试")]
    RiskControl,

    /// 房间禁止发送弹幕，或用户在该房间被禁言。
    #[error("当前房间禁止发送弹幕")]
    Banned,

    /// 本地 5 秒限速或服务端频率限制。
    #[error("发送弹幕过于频繁，请稍后再试")]
    RateLimited,

    /// 弹幕内容为空。
    #[error("弹幕内容不能为空")]
    EmptyMessage,

    /// 房间号不是正整数。
    #[error("房间号无效")]
    InvalidRoom,

    /// 连接失败、超时或其它传输层错误。
    #[error("发送弹幕网络请求失败")]
    Network,

    /// 传输层没有实现表单 POST。
    #[error("当前传输层不支持发送弹幕")]
    Unsupported,

    /// 弹幕接口返回非 2xx。
    #[error("B 站弹幕接口返回 HTTP {0}")]
    Http(u16),

    /// 响应不是预期 JSON，或缺少必要字段。
    #[error("弹幕响应无效")]
    InvalidResponse,

    /// B 站接口返回未归类的业务错误。
    #[error("B 站弹幕接口错误（code={code}）：{message}")]
    Api { code: i64, message: String },
}

impl From<SourceError> for DanmakuError {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Http(status) => Self::Http(status),
            SourceError::Unavailable(_) => Self::RiskControl,
            SourceError::Network(_) | SourceError::Download(_) => Self::Network,
            SourceError::NotImplemented(_) => Self::Unsupported,
            SourceError::Parse(_) => Self::InvalidResponse,
        }
    }
}

/// 发送一条弹幕。
///
/// 请求带完整登录 cookie、`csrf` / `csrf_token`（均取 `bili_jct`）、`roomid`
/// 与 `msg`。同一进程内两次发送至少间隔 [`DANMAKU_MIN_INTERVAL`]；失败不自动
/// 重试，重复内容也不会被吞掉或重发。
pub fn send_danmaku(
    transport: &dyn Transport,
    auth: &BiliAuth,
    room_id: u64,
    message: &str,
) -> Result<(), DanmakuError> {
    let mut last_sent = LAST_SEND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    send_danmaku_with_last(
        transport,
        auth,
        room_id,
        message,
        &mut last_sent,
        Instant::now(),
    )
}

/// 带显式时钟与限速状态的发送实现，便于单测不睡眠。
fn send_danmaku_with_last(
    transport: &dyn Transport,
    auth: &BiliAuth,
    room_id: u64,
    message: &str,
    last_sent: &mut Option<Instant>,
    now: Instant,
) -> Result<(), DanmakuError> {
    if room_id == 0 {
        return Err(DanmakuError::InvalidRoom);
    }
    if message.trim().is_empty() {
        return Err(DanmakuError::EmptyMessage);
    }
    if auth.cookie("SESSDATA").is_none() {
        return Err(DanmakuError::NotLoggedIn);
    }
    let Some(csrf) = auth.cookie("bili_jct") else {
        return Err(DanmakuError::NotLoggedIn);
    };
    if let Some(previous) = *last_sent
        && now.saturating_duration_since(previous) < DANMAKU_MIN_INTERVAL
    {
        return Err(DanmakuError::RateLimited);
    }
    // 先记账再发：即使网络失败也不允许立刻重试，避免触发风控。
    *last_sent = Some(now);

    let cookie = auth.cookie_header();
    let room_id = room_id.to_string();
    let rnd = rnd_value();
    let form = [
        ("csrf", csrf),
        ("csrf_token", csrf),
        ("roomid", room_id.as_str()),
        ("msg", message),
        ("color", "16777215"),
        ("fontsize", "25"),
        ("mode", "1"),
        ("rnd", rnd.as_str()),
        ("plat", "1"),
    ];
    let headers = [("Cookie", cookie.as_str()), ("Referer", REFERER)];
    let response = transport.post_form(SEND_URL, &headers, &form)?;

    if !(200..=299).contains(&response.status) {
        return Err(map_http_status(response.status));
    }
    let parsed: SendResponse =
        serde_json::from_slice(&response.body).map_err(|_| DanmakuError::InvalidResponse)?;
    if parsed.code == 0 {
        Ok(())
    } else {
        Err(map_api_code(parsed.code, parsed.message))
    }
}

/// 弹幕接口响应里只关心业务码与消息。
#[derive(Deserialize)]
struct SendResponse {
    code: i64,
    #[serde(default)]
    message: String,
}

/// HTTP 状态映射；其余状态保留原码，供 UI 显示。
fn map_http_status(status: u16) -> DanmakuError {
    match status {
        412 => DanmakuError::RiskControl,
        429 => DanmakuError::RateLimited,
        _ => DanmakuError::Http(status),
    }
}

/// 当前 Unix 毫秒，作为表单 `rnd`；取不到系统时间时退回 `0`。
fn rnd_value() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
        .to_string()
}

/// 服务端业务码映射。
fn map_api_code(code: i64, message: String) -> DanmakuError {
    match code {
        -101 => DanmakuError::NotLoggedIn,
        -111 => DanmakuError::CsrfInvalid,
        -352 | -412 => DanmakuError::RiskControl,
        -509 | 10031 => DanmakuError::RateLimited,
        10034..=10036 => DanmakuError::Banned,
        _ => DanmakuError::Api { code, message },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use np_sources::{HttpResponse, SourceError};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct RecordedPost {
        url: String,
        headers: Vec<(String, String)>,
        form: Vec<(String, String)>,
    }

    impl RecordedPost {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(header_name, _)| header_name == name)
                .map(|(_, value)| value.as_str())
        }

        fn form_value(&self, name: &str) -> Option<&str> {
            self.form
                .iter()
                .find(|(field_name, _)| field_name == name)
                .map(|(_, value)| value.as_str())
        }
    }

    struct FakeTransport {
        responses: Mutex<VecDeque<Result<HttpResponse, SourceError>>>,
        requests: Mutex<Vec<RecordedPost>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().map(Ok).collect()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn failing(error: SourceError) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from([Err(error)])),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn only_post(&self) -> RecordedPost {
            let requests = self.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "应只记录一次 POST");
            requests[0].clone()
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, _url: &str) -> np_sources::Result<Vec<u8>> {
            Err(SourceError::NotImplemented(
                "FakeTransport 只实现 post_form".to_owned(),
            ))
        }

        fn post_form(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            form: &[(&str, &str)],
        ) -> np_sources::Result<HttpResponse> {
            self.requests.lock().unwrap().push(RecordedPost {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                form: form
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            });
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Err(SourceError::NotImplemented(
                        "FakeTransport 响应已耗尽".to_owned(),
                    ))
                })
        }
    }

    fn http_response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn success() -> HttpResponse {
        http_response(200, r#"{"code":0,"message":"0"}"#)
    }

    fn api_response(code: i64, message: &str) -> HttpResponse {
        http_response(200, &format!(r#"{{"code":{code},"message":"{message}"}}"#))
    }

    fn sample_auth() -> BiliAuth {
        BiliAuth::new(
            vec![
                ("SESSDATA".to_owned(), "sess-secret".to_owned()),
                ("bili_jct".to_owned(), "csrf-secret".to_owned()),
                ("DedeUserID".to_owned(), "42".to_owned()),
            ],
            None,
        )
    }

    fn error_for_response(response: HttpResponse) -> DanmakuError {
        let transport = FakeTransport::new(vec![response]);
        send_danmaku_with_last(
            &transport,
            &sample_auth(),
            21622811,
            "晚上好",
            &mut None,
            Instant::now(),
        )
        .unwrap_err()
    }

    fn error_for_status(status: u16) -> DanmakuError {
        error_for_response(http_response(status, "server error"))
    }

    #[test]
    fn sends_cookie_csrf_room_and_message() {
        let transport = FakeTransport::new(vec![success()]);
        let auth = sample_auth();
        let t0 = Instant::now();
        let mut last = None;

        send_danmaku_with_last(&transport, &auth, 21622811, "晚上好", &mut last, t0)
            .expect("code 0 应成功");

        let post = transport.only_post();
        assert_eq!(post.url, SEND_URL);
        assert_eq!(post.form_value("csrf"), Some("csrf-secret"));
        assert_eq!(post.form_value("csrf_token"), Some("csrf-secret"));
        assert_eq!(post.form_value("roomid"), Some("21622811"));
        assert_eq!(post.form_value("msg"), Some("晚上好"));
        assert_eq!(post.form_value("color"), Some("16777215"));
        assert_eq!(post.form_value("fontsize"), Some("25"));
        assert_eq!(post.form_value("mode"), Some("1"));
        assert_eq!(post.form_value("plat"), Some("1"));
        let rnd = post.form_value("rnd").expect("应带 rnd");
        assert!(!rnd.is_empty());
        assert!(
            rnd.chars().all(|character| character.is_ascii_digit()),
            "rnd 应为 Unix 秒或毫秒数字，实际 {rnd}"
        );
        let cookie = post.header("Cookie").expect("应带 Cookie");
        assert!(cookie.contains("SESSDATA=sess-secret"));
        assert!(cookie.contains("bili_jct=csrf-secret"));
        assert_eq!(post.header("Referer"), Some("https://live.bilibili.com/"));
        assert_eq!(last, Some(t0));
    }

    #[test]
    fn rate_limit_blocks_within_five_seconds() {
        assert_eq!(DANMAKU_MIN_INTERVAL, Duration::from_secs(5));
        let transport = FakeTransport::new(vec![success(), success()]);
        let auth = sample_auth();
        let t0 = Instant::now();
        let mut last = None;

        send_danmaku_with_last(&transport, &auth, 1, "hi", &mut last, t0).unwrap();
        let error = send_danmaku_with_last(
            &transport,
            &auth,
            1,
            "hi",
            &mut last,
            t0 + Duration::from_secs(4),
        )
        .unwrap_err();
        assert!(matches!(error, DanmakuError::RateLimited));
        assert_eq!(transport.request_count(), 1, "限速内不得发第二次请求");

        send_danmaku_with_last(
            &transport,
            &auth,
            1,
            "hi",
            &mut last,
            t0 + DANMAKU_MIN_INTERVAL,
        )
        .expect("满 5 秒应放行");
        assert_eq!(transport.request_count(), 2);
    }

    #[test]
    fn public_send_enforces_process_wide_rate_limit() {
        reset_global_limiter();
        let transport = FakeTransport::new(vec![success()]);
        let auth = sample_auth();

        send_danmaku(&transport, &auth, 1, "hi").expect("首次发送应成功");
        assert!(matches!(
            send_danmaku(&transport, &auth, 1, "hi"),
            Err(DanmakuError::RateLimited)
        ));
        assert_eq!(transport.request_count(), 1);
        reset_global_limiter();
    }

    #[test]
    fn missing_login_or_csrf_is_rejected_before_request() {
        let transport = FakeTransport::new(Vec::new());
        let mut last = None;
        let empty = BiliAuth::new(Vec::new(), None);
        let no_csrf = BiliAuth::new(
            vec![("SESSDATA".to_owned(), "sess-secret".to_owned())],
            None,
        );

        assert!(matches!(
            send_danmaku_with_last(&transport, &empty, 1, "hi", &mut last, Instant::now()),
            Err(DanmakuError::NotLoggedIn)
        ));
        assert!(matches!(
            send_danmaku_with_last(&transport, &no_csrf, 1, "hi", &mut last, Instant::now()),
            Err(DanmakuError::NotLoggedIn)
        ));
        assert_eq!(transport.request_count(), 0);
    }

    #[test]
    fn api_code_minus_101_maps_not_logged_in() {
        assert!(matches!(
            error_for_response(api_response(-101, "账号未登录")),
            DanmakuError::NotLoggedIn
        ));
    }

    #[test]
    fn api_code_minus_111_maps_csrf_invalid() {
        assert!(matches!(
            error_for_response(api_response(-111, "csrf 校验失败")),
            DanmakuError::CsrfInvalid
        ));
    }

    #[test]
    fn api_risk_control_codes_map() {
        for code in [-352, -412] {
            assert!(matches!(
                error_for_response(api_response(code, "风控")),
                DanmakuError::RiskControl
            ));
        }
    }

    #[test]
    fn api_ban_and_rate_limit_codes_map() {
        for code in [10034, 10035, 10036] {
            assert!(matches!(
                error_for_response(api_response(code, "禁言")),
                DanmakuError::Banned
            ));
        }
        for code in [-509, 10031] {
            assert!(matches!(
                error_for_response(api_response(code, "太快")),
                DanmakuError::RateLimited
            ));
        }
    }

    #[test]
    fn unknown_api_code_keeps_code_and_message() {
        match error_for_response(api_response(10030, "弹幕内容包含非法内容")) {
            DanmakuError::Api { code, message } => {
                assert_eq!(code, 10030);
                assert_eq!(message, "弹幕内容包含非法内容");
            }
            other => panic!("应保留为 Api，实际 {other:?}"),
        }
    }

    #[test]
    fn http_statuses_map() {
        assert!(matches!(error_for_status(412), DanmakuError::RiskControl));
        assert!(matches!(error_for_status(429), DanmakuError::RateLimited));
        assert!(matches!(error_for_status(500), DanmakuError::Http(500)));
        assert!(matches!(error_for_status(403), DanmakuError::Http(403)));
    }

    #[test]
    fn network_error_maps_without_leaking_url() {
        let transport = FakeTransport::failing(SourceError::Network(
            "连接 https://api.live.bilibili.com/msg/send?cookie=SESSDATA%3Dsess-secret 失败"
                .to_owned(),
        ));
        let error = send_danmaku_with_last(
            &transport,
            &sample_auth(),
            1,
            "hi",
            &mut None,
            Instant::now(),
        )
        .unwrap_err();
        assert!(matches!(error, DanmakuError::Network));
        assert!(!format!("{error:?} {error}").contains("sess-secret"));
    }

    #[test]
    fn invalid_json_maps_invalid_response() {
        assert!(matches!(
            error_for_response(http_response(200, "{not json")),
            DanmakuError::InvalidResponse
        ));
    }

    #[test]
    fn empty_message_and_invalid_room_are_rejected() {
        let transport = FakeTransport::new(Vec::new());
        let auth = sample_auth();
        let mut last = None;

        assert!(matches!(
            send_danmaku_with_last(&transport, &auth, 1, "   ", &mut last, Instant::now()),
            Err(DanmakuError::EmptyMessage)
        ));
        assert!(matches!(
            send_danmaku_with_last(&transport, &auth, 0, "hi", &mut last, Instant::now()),
            Err(DanmakuError::InvalidRoom)
        ));
        assert_eq!(transport.request_count(), 0);
    }

    #[test]
    fn errors_do_not_leak_cookie_or_request_body() {
        let error = error_for_response(api_response(-111, "csrf 校验失败"));
        let text = format!("{error:?} {error}");
        assert!(!text.contains("sess-secret"));
        assert!(!text.contains("csrf-secret"));
        assert!(!text.contains("msg="));
        assert!(!text.contains("晚上好"));
    }

    #[test]
    fn not_implemented_transport_maps_unsupported() {
        let transport = FakeTransport::failing(SourceError::NotImplemented(
            "当前传输层不支持表单 POST".to_owned(),
        ));
        let error = send_danmaku_with_last(
            &transport,
            &sample_auth(),
            1,
            "hi",
            &mut None,
            Instant::now(),
        )
        .unwrap_err();
        assert!(matches!(error, DanmakuError::Unsupported));
    }

    fn reset_global_limiter() {
        *LAST_SEND
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}
