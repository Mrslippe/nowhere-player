//! B 站直播弹幕 WebSocket 客户端（T-0100a2）。
//!
//! 流程与边界：
//!
//! 1. [`get_danmu_info`] 走 [`np_sources::Transport`] 取弹幕服务器 `token` 与
//!    `host_list`；
//! 2. [`run_danmaku_ws`] 用阻塞式 `tungstenite`（rustls + webpki 根证书）连
//!    `wss://host/sub`，先发 `op=7` 认证，再每 30 秒发 `op=2` 心跳；
//! 3. 下行 `op=5` 按 `protover` 解压（0/1 明文、2 zlib、3 brotli），把
//!    `DANMU_MSG` 解析成 [`DanmakuMessage`] 通过 mpsc 交给调用方；
//! 4. 断线、认证失败、超时、取消都返回可读错误；**不自动重连**（留给
//!    T-0100a3）。
//!
//! 单条 `DANMU_MSG` 解析失败只丢这一条，坏帧也只跳过这一帧，都不中断整条流。
//! 日志、错误文本与 `Debug` 都不输出 token、cookie 或原始包；单测全部用
//! [`np_sources::Transport`] 假实现与内存夹具，不建立真实连接。

use std::fmt;
use std::io::{self, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use np_sources::{SourceError, Transport};
use serde::Deserialize;
use tungstenite::error::ProtocolError;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{HandshakeError, Message, WebSocket};

use crate::auth::BiliAuth;
use crate::danmaku_protocol::{
    DanmakuMessage, DanmakuPacket, OP_AUTH, OP_AUTH_REPLY, OP_HEARTBEAT, OP_NOTIFICATION,
    PROTOVER_BROTLI, PROTOVER_PLAIN, PROTOVER_ZLIB, auth_body, decode_packets, encode_packet,
    heartbeat_body, parse_danmu_msg,
};

/// `getDanmuInfo` 接口。
const DANMU_INFO_URL: &str = "https://api.live.bilibili.com/xlive/web-room/v1/index/getDanmuInfo";
/// 弹幕接口要求的来源页；缺了会被 403。
const DANMU_REFERER: &str = "https://live.bilibili.com/";
/// 认证时请求服务端下发压缩通知；protover=3 才会收到 zlib/brotli。
pub const CLIENT_PROTOVER: u16 = 3;
/// 心跳间隔，与 B 站 Web 客户端一致。
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// 连续这么久没有任何下行帧就判定连接已死（服务端至少每 30s 回一次 `op=3`）。
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// 等待 `op=8` 认证回复的上限。
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// TCP 连接与 TLS/WS 握手的上限。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次 `read` 的阻塞上限：用它轮询取消标志与心跳时间。
const READ_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 解压后 body 的上限，防止压缩炸弹。
const MAX_DECOMPRESSED_LEN: usize = 8 * 1024 * 1024;

type DanmakuSocket = WebSocket<MaybeTlsStream<TcpStream>>;

/// 一个弹幕服务器条目（`host_list` 的一项）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DanmakuHost {
    /// 主机名，不含协议与端口。
    pub host: String,
    /// 明文 TCP 端口（本卡不用）。
    pub port: u16,
    /// 明文 WebSocket 端口（本卡不用）。
    pub ws_port: u16,
    /// TLS WebSocket 端口；缺省按 443 处理。
    pub wss_port: u16,
}

impl DanmakuHost {
    /// 实际连接用的 wss 端口：接口没给时退回 443。
    pub fn wss_port_or_default(&self) -> u16 {
        if self.wss_port == 0 {
            443
        } else {
            self.wss_port
        }
    }

    /// `wss://host[:port]/sub`；443 时省略端口，便于日志与单测稳定比较。
    pub fn wss_url(&self) -> String {
        let host = self.host.trim();
        let port = self.wss_port_or_default();
        if port == 443 {
            format!("wss://{host}/sub")
        } else {
            format!("wss://{host}:{port}/sub")
        }
    }
}

/// `getDanmuInfo` 的解析结果。
///
/// `token` 等价于弹幕连接凭证，因此 `Debug` 只输出 `<redacted>`。
#[derive(Clone, PartialEq, Eq)]
pub struct DanmuInfo {
    token: String,
    hosts: Vec<DanmakuHost>,
}

impl DanmuInfo {
    /// 弹幕服务器 token；不要打印或写进错误文本。
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 按服务端顺序排列的弹幕服务器。
    pub fn hosts(&self) -> &[DanmakuHost] {
        &self.hosts
    }
}

impl fmt::Debug for DanmuInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DanmuInfo")
            .field("token", &"<redacted>")
            .field("hosts", &self.hosts)
            .finish()
    }
}

/// 弹幕客户端错误。
///
/// 只包含状态码、业务码、位置信息或固定说明；不携带 token、cookie、原始包或
/// 带凭证的 URL。
#[derive(Debug, thiserror::Error)]
pub enum DanmakuWsError {
    /// 房间号不是正整数。
    #[error("房间号无效：{0}")]
    InvalidRoom(String),

    /// 取弹幕服务器信息时的传输层错误。
    #[error("弹幕服务器信息请求失败：{0}")]
    Source(#[from] SourceError),

    /// `getDanmuInfo` 返回非 0 业务码。
    #[error("B 站接口错误（code={code}）：{message}")]
    Api { code: i64, message: String },

    /// 响应不是预期 JSON，或缺少 token 等关键字段。
    #[error("弹幕服务器信息无效：{0}")]
    InvalidResponse(String),

    /// `host_list` 没有可用条目。
    #[error("弹幕服务器列表为空")]
    NoHost,

    /// TCP / TLS / WebSocket 握手失败。
    #[error("连接弹幕服务器失败：{0}")]
    Connect(String),

    /// 连接、认证或空闲超时；消息本身是一句完整说明。
    #[error("{0}")]
    Timeout(&'static str),

    /// 服务端 `op=8` 回复的 code 非 0。
    #[error("弹幕服务器认证失败（code={code}）")]
    AuthFailed { code: i64 },

    /// 包头、压缩或认证回复结构不符合预期。
    #[error("弹幕数据无效：{0}")]
    Protocol(String),

    /// 服务端关闭连接。
    #[error("弹幕连接已关闭")]
    Closed,

    /// 调用方的取消标志已置位。
    #[error("弹幕连接已取消")]
    Cancelled,

    /// 读写底层 socket 失败。
    #[error("弹幕连接读写失败：{0}")]
    Io(#[from] io::Error),
}

/// `getDanmuInfo` URL。
pub fn danmu_info_url(room_id: u64) -> String {
    format!("{DANMU_INFO_URL}?id={room_id}&type=0")
}

/// 匿名取弹幕服务器信息。
pub fn get_danmu_info(
    transport: &dyn Transport,
    room_id: u64,
) -> Result<DanmuInfo, DanmakuWsError> {
    get_danmu_info_with_auth(transport, room_id, None)
}

/// 带可选登录凭证取弹幕服务器信息；登录只影响服务端下发的房间推荐，不影响 token
/// 的用途。
pub fn get_danmu_info_with_auth(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
) -> Result<DanmuInfo, DanmakuWsError> {
    if room_id == 0 {
        return Err(DanmakuWsError::InvalidRoom("房间号必须是正整数".to_owned()));
    }
    let url = danmu_info_url(room_id);
    let cookie = auth.map(BiliAuth::cookie_header).unwrap_or_default();
    let response = if cookie.is_empty() {
        transport.get_with_headers(&url, &[("Referer", DANMU_REFERER)])?
    } else {
        transport.get_with_headers(&url, &[("Referer", DANMU_REFERER), ("Cookie", &cookie)])?
    };
    if !(200..=299).contains(&response.status) {
        return Err(DanmakuWsError::Source(SourceError::Http(response.status)));
    }
    parse_danmu_info(&response.body)
}

/// 解析 `getDanmuInfo` 响应；`token` 为空视为坏响应。
pub fn parse_danmu_info(body: &[u8]) -> Result<DanmuInfo, DanmakuWsError> {
    let envelope: DanmuEnvelope = serde_json::from_slice(body)
        .map_err(|error| DanmakuWsError::InvalidResponse(format!("JSON 解析失败：{error}")))?;
    if envelope.code != 0 {
        let message = if envelope.message.is_empty() {
            "弹幕服务器信息请求失败".to_owned()
        } else {
            envelope.message
        };
        return Err(DanmakuWsError::Api {
            code: envelope.code,
            message,
        });
    }
    let data = envelope
        .data
        .ok_or_else(|| DanmakuWsError::InvalidResponse("响应缺少 data".to_owned()))?;
    if data.token.trim().is_empty() {
        return Err(DanmakuWsError::InvalidResponse("响应缺少 token".to_owned()));
    }
    let hosts = data
        .host_list
        .into_iter()
        .map(|entry| DanmakuHost {
            host: entry.host,
            port: entry.port,
            ws_port: entry.ws_port,
            wss_port: entry.wss_port,
        })
        .collect();
    Ok(DanmuInfo {
        token: data.token,
        hosts,
    })
}

/// 跑一条弹幕连接：取服务器信息 → 连接 → 认证 → 心跳 + 收弹幕。
///
/// `cancel` 置位后返回 [`DanmakuWsError::Cancelled`]；接收端 drop 掉 `tx` 后正常
/// 结束。连接与认证都受超时约束，失败后**不自动重连**。
pub fn run_danmaku_ws(
    transport: &dyn Transport,
    room_id: u64,
    auth: Option<&BiliAuth>,
    cancel: &AtomicBool,
    tx: &Sender<DanmakuMessage>,
) -> Result<(), DanmakuWsError> {
    if room_id == 0 {
        return Err(DanmakuWsError::InvalidRoom("房间号必须是正整数".to_owned()));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(DanmakuWsError::Cancelled);
    }
    let info = get_danmu_info_with_auth(transport, room_id, auth)?;
    let host = pick_wss_host(info.hosts()).ok_or(DanmakuWsError::NoHost)?;
    let uid = auth.and_then(BiliAuth::uid).unwrap_or(0);

    let mut socket = connect_wss(host)?;
    send_auth(&mut socket, room_id, uid, info.token())?;
    await_auth(&mut socket, cancel)?;
    read_loop(&mut socket, cancel, tx)
}

/// 选第一个主机名非空的条目；顺序沿用服务端给的优先级。
fn pick_wss_host(hosts: &[DanmakuHost]) -> Option<&DanmakuHost> {
    hosts.iter().find(|host| !host.host.trim().is_empty())
}

/// 建立 TCP 连接并完成 TLS + WebSocket 握手。
fn connect_wss(host: &DanmakuHost) -> Result<DanmakuSocket, DanmakuWsError> {
    let name = host.host.trim();
    if name.is_empty() {
        return Err(DanmakuWsError::Connect("弹幕服务器地址为空".to_owned()));
    }
    let port = host.wss_port_or_default();
    let addresses = (name, port)
        .to_socket_addrs()
        .map_err(|error| DanmakuWsError::Connect(format!("无法解析弹幕服务器地址：{error}")))?;
    let mut last_error: Option<io::Error> = None;
    let mut connected = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(socket) => {
                connected = Some(socket);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some(socket) = connected else {
        return Err(match last_error {
            Some(error) => map_connect_error(error),
            None => DanmakuWsError::Connect("弹幕服务器地址没有可用的 IP".to_owned()),
        });
    };

    // 握手阶段先用连接超时兜底，TLS / HTTP 卡死也不会无限期阻塞。
    socket
        .set_read_timeout(Some(CONNECT_TIMEOUT))
        .map_err(DanmakuWsError::Io)?;
    socket
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(DanmakuWsError::Io)?;

    let (mut socket, _response) =
        tungstenite::client_tls_with_config(host.wss_url(), socket, None, None).map_err(
            |error| match error {
                HandshakeError::Failure(error) => map_ws_error(error),
                HandshakeError::Interrupted(_) => {
                    DanmakuWsError::Timeout("弹幕服务器 WebSocket 握手超时")
                }
            },
        )?;
    tune_socket(&mut socket)?;
    Ok(socket)
}

/// 握手完成后把读写超时降到轮询间隔，让取消与心跳可以按时检查。
fn tune_socket(socket: &mut DanmakuSocket) -> Result<(), DanmakuWsError> {
    // 本项目只启用 rustls 后端，因此一定能拿到底层 TcpStream；其它 TLS 后端
    // 保持阻塞读，取消要等下一帧或断线才生效。
    if let MaybeTlsStream::Rustls(tls) = socket.get_mut() {
        tls.sock
            .set_read_timeout(Some(READ_POLL_INTERVAL))
            .map_err(DanmakuWsError::Io)?;
        tls.sock
            .set_write_timeout(Some(CONNECT_TIMEOUT))
            .map_err(DanmakuWsError::Io)?;
    }
    Ok(())
}

/// 认证包（`op=7`）：字段与 B 站 Web 客户端一致，token 只出现在 body 里。
fn auth_packet(room_id: u64, uid: u64, token: &str) -> Vec<u8> {
    encode_packet(OP_AUTH, &auth_body(room_id, uid, CLIENT_PROTOVER, token))
}

/// 发送 `op=7` 认证包；body 里的 token 不出现在任何错误文本里。
fn send_auth(
    socket: &mut DanmakuSocket,
    room_id: u64,
    uid: u64,
    token: &str,
) -> Result<(), DanmakuWsError> {
    socket
        .send(Message::binary(auth_packet(room_id, uid, token)))
        .map_err(map_send_error)
}

/// 等 `op=8` 认证回复；期间收到的弹幕包直接丢弃（认证前服务端不会推弹幕）。
fn await_auth(socket: &mut DanmakuSocket, cancel: &AtomicBool) -> Result<(), DanmakuWsError> {
    let deadline = Instant::now() + AUTH_TIMEOUT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DanmakuWsError::Cancelled);
        }
        match next_incoming(socket)? {
            Incoming::Data(bytes) => {
                if let Some(code) = parse_frame(&bytes)?.auth_code {
                    return check_auth_code(code);
                }
            }
            Incoming::Closed => return Err(DanmakuWsError::Closed),
            Incoming::Activity | Incoming::Idle => {}
        }
        if Instant::now() >= deadline {
            return Err(DanmakuWsError::Timeout("等待弹幕服务器认证回复超时"));
        }
    }
}

fn check_auth_code(code: i64) -> Result<(), DanmakuWsError> {
    if code == 0 {
        Ok(())
    } else {
        Err(DanmakuWsError::AuthFailed { code })
    }
}

/// 认证后的主循环：按 30s 发心跳、把弹幕交给 mpsc、检查取消与空闲超时。
fn read_loop(
    socket: &mut DanmakuSocket,
    cancel: &AtomicBool,
    tx: &Sender<DanmakuMessage>,
) -> Result<(), DanmakuWsError> {
    let mut last_seen = Instant::now();
    let mut next_heartbeat = Instant::now() + HEARTBEAT_INTERVAL;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DanmakuWsError::Cancelled);
        }
        let now = Instant::now();
        if now >= next_heartbeat {
            socket
                .send(Message::binary(encode_packet(
                    OP_HEARTBEAT,
                    heartbeat_body(),
                )))
                .map_err(map_send_error)?;
            next_heartbeat = now + HEARTBEAT_INTERVAL;
        }
        match next_incoming(socket)? {
            Incoming::Data(bytes) => {
                last_seen = Instant::now();
                // 坏帧只跳过这一帧：单条 JSON 坏掉在 parse_frame 里已经吞掉，
                // 这里兜住包头/压缩损坏，服务端后续帧仍可正常解析。
                let Some(outcome) = parse_frame(&bytes).ok() else {
                    continue;
                };
                for message in outcome.messages {
                    if tx.send(message).is_err() {
                        // 接收端（UI）已退出：正常收尾，不重连。
                        return Ok(());
                    }
                }
            }
            Incoming::Closed => return Err(DanmakuWsError::Closed),
            Incoming::Activity => last_seen = Instant::now(),
            Incoming::Idle => {
                if idle_expired(last_seen, Instant::now(), IDLE_TIMEOUT) {
                    return Err(DanmakuWsError::Timeout("弹幕服务器长时间没有下发数据"));
                }
            }
        }
    }
}

/// 一次 `read` 的结果。
enum Incoming {
    /// 一帧业务数据。
    Data(Vec<u8>),
    /// 控制帧（ping/pong）：连接还活着，但没有业务数据。
    Activity,
    /// 读超时：没有数据，但连接没断。
    Idle,
    /// 服务端关闭了连接。
    Closed,
}

fn next_incoming(socket: &mut DanmakuSocket) -> Result<Incoming, DanmakuWsError> {
    match socket.read() {
        Ok(Message::Binary(bytes)) => Ok(Incoming::Data(bytes.to_vec())),
        Ok(Message::Text(text)) => Ok(Incoming::Data(text.as_str().as_bytes().to_vec())),
        Ok(Message::Close(_)) => Ok(Incoming::Closed),
        // Ping/Pong/Frame：tungstenite 已自动回 pong，只当作“连接还活着”。
        Ok(_) => Ok(Incoming::Activity),
        Err(tungstenite::Error::Io(error)) if is_transient_io(&error) => Ok(Incoming::Idle),
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
            Ok(Incoming::Closed)
        }
        Err(error) => Err(map_ws_error(error)),
    }
}

/// 一帧下行数据解析出的内容。
#[derive(Debug, Default)]
struct FrameOutcome {
    messages: Vec<DanmakuMessage>,
    auth_code: Option<i64>,
}

/// 解析一帧下行数据：拆包 → 解压 → 只认 `DANMU_MSG` 与 `op=8`。
///
/// 单条 `DANMU_MSG` 坏掉只跳过这一条；包头/压缩损坏返回 [`DanmakuWsError::Protocol`]。
fn parse_frame(bytes: &[u8]) -> Result<FrameOutcome, DanmakuWsError> {
    let packets =
        decode_packets(bytes).map_err(|error| DanmakuWsError::Protocol(error.to_string()))?;
    let mut outcome = FrameOutcome::default();
    for packet in packets {
        match packet.operation {
            OP_NOTIFICATION => {
                for notification in expand_notification(packet)? {
                    match parse_danmu_msg(&notification.body) {
                        Ok(Some(message)) => outcome.messages.push(message),
                        // 礼物、进场等其它 cmd 不关心。
                        Ok(None) => {}
                        // 单条 JSON 字段坏了只丢这一条，流继续。
                        Err(_) => {}
                    }
                }
            }
            OP_AUTH_REPLY => outcome.auth_code = Some(parse_auth_code(&packet.body)?),
            _ => {}
        }
    }
    Ok(outcome)
}

/// 把一条 `op=5` 包展开成内层包：明文原样返回，压缩则解压后拆包。
fn expand_notification(packet: DanmakuPacket) -> Result<Vec<DanmakuPacket>, DanmakuWsError> {
    match packet.protover {
        0 | PROTOVER_PLAIN => Ok(vec![packet]),
        PROTOVER_ZLIB | PROTOVER_BROTLI => {
            let expanded = decompress(packet.protover, &packet.body)?;
            if expanded.first() == Some(&b'{') {
                // 少数实现把解压后的单条 JSON 直接放在 body 里，没有内层包头。
                return Ok(vec![DanmakuPacket {
                    operation: OP_NOTIFICATION,
                    protover: PROTOVER_PLAIN,
                    body: expanded,
                }]);
            }
            decode_packets(&expanded).map_err(|error| {
                DanmakuWsError::Protocol(format!("解压后的弹幕包无法拆分：{error}"))
            })
        }
        other => Err(DanmakuWsError::Protocol(format!(
            "不支持的弹幕压缩协议 protover={other}"
        ))),
    }
}

/// 按 protover 解压 body；输出有上限，避免压缩炸弹。
fn decompress(protover: u16, body: &[u8]) -> Result<Vec<u8>, DanmakuWsError> {
    let mut expanded = Vec::new();
    let limit = MAX_DECOMPRESSED_LEN as u64;
    match protover {
        PROTOVER_ZLIB => {
            flate2::read::ZlibDecoder::new(body)
                .take(limit)
                .read_to_end(&mut expanded)
                .map_err(|error| DanmakuWsError::Protocol(format!("zlib 解压失败：{error}")))?;
        }
        PROTOVER_BROTLI => {
            brotli::Decompressor::new(body, 4096)
                .take(limit)
                .read_to_end(&mut expanded)
                .map_err(|error| DanmakuWsError::Protocol(format!("brotli 解压失败：{error}")))?;
        }
        other => {
            return Err(DanmakuWsError::Protocol(format!(
                "不支持的弹幕压缩协议 protover={other}"
            )));
        }
    }
    if expanded.len() as u64 >= limit {
        return Err(DanmakuWsError::Protocol(
            "解压后的弹幕数据超过上限".to_owned(),
        ));
    }
    Ok(expanded)
}

/// `op=8` 的 body 只有一个 `code`。
#[derive(Deserialize)]
struct AuthReply {
    code: i64,
}

fn parse_auth_code(body: &[u8]) -> Result<i64, DanmakuWsError> {
    let reply: AuthReply = serde_json::from_slice(body)
        .map_err(|_| DanmakuWsError::Protocol("弹幕认证回复不是合法 JSON".to_owned()))?;
    Ok(reply.code)
}

fn idle_expired(last_seen: Instant, now: Instant, idle: Duration) -> bool {
    now.saturating_duration_since(last_seen) >= idle
}

fn is_transient_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

fn map_connect_error(error: io::Error) -> DanmakuWsError {
    if error.kind() == io::ErrorKind::TimedOut {
        DanmakuWsError::Timeout("连接弹幕服务器超时")
    } else {
        DanmakuWsError::Connect(format!("无法连接弹幕服务器：{error}"))
    }
}

fn map_send_error(error: tungstenite::Error) -> DanmakuWsError {
    match error {
        tungstenite::Error::Io(error) if is_transient_io(&error) => {
            DanmakuWsError::Timeout("发送弹幕心跳超时")
        }
        other => map_ws_error(other),
    }
}

/// 把 tungstenite 错误映射成可读错误；不复制原始字节或 URL 凭证。
fn map_ws_error(error: tungstenite::Error) -> DanmakuWsError {
    match error {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            DanmakuWsError::Closed
        }
        tungstenite::Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
            DanmakuWsError::Closed
        }
        tungstenite::Error::Io(error) => DanmakuWsError::Io(error),
        tungstenite::Error::Tls(error) => DanmakuWsError::Connect(format!("TLS 握手失败：{error}")),
        tungstenite::Error::Url(error) => {
            DanmakuWsError::Connect(format!("弹幕服务器地址无效：{error}"))
        }
        tungstenite::Error::Http(response) => DanmakuWsError::Connect(format!(
            "弹幕服务器拒绝 WebSocket 握手（HTTP {}）",
            response.status()
        )),
        tungstenite::Error::HttpFormat(error) => {
            DanmakuWsError::Connect(format!("WebSocket 握手请求无效：{error}"))
        }
        tungstenite::Error::Protocol(error) => {
            DanmakuWsError::Protocol(format!("WebSocket 协议错误：{error}"))
        }
        tungstenite::Error::Capacity(error) => {
            DanmakuWsError::Protocol(format!("弹幕数据超过长度上限：{error}"))
        }
        tungstenite::Error::WriteBufferFull(_) => {
            DanmakuWsError::Protocol("弹幕连接写缓冲已满".to_owned())
        }
        tungstenite::Error::Utf8(error) => {
            DanmakuWsError::Protocol(format!("弹幕文本不是 UTF-8：{error}"))
        }
        tungstenite::Error::AttackAttempt => {
            DanmakuWsError::Protocol("弹幕连接疑似受到攻击".to_owned())
        }
    }
}

/// `getDanmuInfo` 响应信封。
#[derive(Deserialize)]
struct DanmuEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<DanmuData>,
}

#[derive(Deserialize)]
struct DanmuData {
    #[serde(default)]
    token: String,
    #[serde(default)]
    host_list: Vec<HostEntry>,
}

#[derive(Deserialize)]
struct HostEntry {
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    ws_port: u16,
    #[serde(default)]
    wss_port: u16,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use np_sources::HttpResponse;
    use serde_json::Value;

    use crate::danmaku_protocol::HEADER_LEN;

    use super::*;

    /// 真实结构的 `DANMU_MSG`。
    const DANMU_JSON: &str = r#"{
        "cmd": "DANMU_MSG",
        "info": [
            [0, 1, 25, 16777215, 1726799123456, 1726799123, 0, "hash", 0, 0, 0, "", 0, "{}", "{}", 0],
            "晚上好",
            [123456, "用户A", 0, 0, 0, 10000, 1, ""]
        ]
    }"#;

    /// 第一个 host 为空、第二个可用、第三个端口非默认，用来测选择逻辑。
    const DANMU_INFO: &str = r#"{
        "code": 0,
        "message": "0",
        "ttl": 1,
        "data": {
            "group": "live",
            "business_id": 0,
            "max_delay": 5000,
            "token": "mock-token-secret",
            "host_list": [
                {"host": "", "port": 2243, "wss_port": 443, "ws_port": 2244},
                {"host": "broadcast.example.invalid", "port": 2243, "wss_port": 443, "ws_port": 2244},
                {"host": "backup.example.invalid", "port": 2243, "wss_port": 8443, "ws_port": 2244}
            ]
        }
    }"#;

    const EMPTY_HOST_LIST: &str = r#"{
        "code": 0,
        "message": "0",
        "data": {"token": "mock-token-secret", "host_list": []}
    }"#;

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        url: String,
        headers: Vec<(String, String)>,
    }

    impl RecordedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(header_name, _)| header_name == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// 只实现 `get_with_headers` 的假传输层；记录 URL 与请求头，不触网。
    struct FakeTransport {
        status: u16,
        body: &'static str,
        requests: Mutex<Vec<RecordedRequest>>,
    }

    impl FakeTransport {
        fn ok(body: &'static str) -> Self {
            Self {
                status: 200,
                body,
                requests: Mutex::new(Vec::new()),
            }
        }

        fn with_status(status: u16, body: &'static str) -> Self {
            Self {
                status,
                body,
                requests: Mutex::new(Vec::new()),
            }
        }

        fn only_request(&self) -> RecordedRequest {
            let requests = self.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "应只记录一次请求");
            requests[0].clone()
        }

        fn request_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            self.get_with_headers(url, &[])
                .map(|response| response.body)
        }

        fn get_with_headers(
            &self,
            url: &str,
            headers: &[(&str, &str)],
        ) -> np_sources::Result<HttpResponse> {
            self.requests.lock().unwrap().push(RecordedRequest {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            });
            Ok(HttpResponse {
                status: self.status,
                headers: Vec::new(),
                body: self.body.as_bytes().to_vec(),
            })
        }
    }

    /// 按 B 站格式拼一个包。
    fn frame(protover: u16, operation: u32, body: &[u8]) -> Vec<u8> {
        let total = HEADER_LEN + body.len();
        let mut bytes = Vec::with_capacity(total);
        bytes.extend_from_slice(&(total as u32).to_be_bytes());
        bytes.extend_from_slice(&(HEADER_LEN as u16).to_be_bytes());
        bytes.extend_from_slice(&protover.to_be_bytes());
        bytes.extend_from_slice(&operation.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    /// 明文通知包（内层）。
    fn plain_notification(json: &str) -> Vec<u8> {
        frame(PROTOVER_PLAIN, OP_NOTIFICATION, json.as_bytes())
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        use std::io::Write;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("测试压缩不应失败");
        encoder.finish().expect("测试压缩不应失败")
    }

    fn brotli(data: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut encoder = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
        encoder.write_all(data).expect("测试压缩不应失败");
        encoder.into_inner()
    }

    fn compressed_notification(protover: u16, json: &str) -> Vec<u8> {
        let inner = plain_notification(json);
        let body = if protover == PROTOVER_ZLIB {
            zlib(&inner)
        } else {
            brotli(&inner)
        };
        frame(protover, OP_NOTIFICATION, &body)
    }

    fn sample_auth() -> BiliAuth {
        BiliAuth::new(
            vec![
                ("SESSDATA".to_owned(), "sess-secret".to_owned()),
                ("DedeUserID".to_owned(), "42".to_owned()),
            ],
            None,
        )
    }

    #[test]
    fn danmu_info_url_requests_room_and_type() {
        let url = danmu_info_url(21622811);
        assert!(url.contains("getDanmuInfo"));
        assert!(url.contains("id=21622811"));
        assert!(url.contains("type=0"));
    }

    #[test]
    fn get_danmu_info_parses_token_and_hosts() {
        let transport = FakeTransport::ok(DANMU_INFO);
        let info = get_danmu_info(&transport, 21622811).expect("夹具应可解析");
        assert_eq!(info.token(), "mock-token-secret");
        let hosts = info.hosts();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[1].host, "broadcast.example.invalid");
        assert_eq!(hosts[1].wss_port_or_default(), 443);
        assert_eq!(hosts[2].wss_port_or_default(), 8443);
    }

    #[test]
    fn get_danmu_info_sends_referer_and_room_id() {
        let transport = FakeTransport::ok(DANMU_INFO);
        get_danmu_info(&transport, 21622811).expect("夹具应可解析");
        let request = transport.only_request();
        assert!(request.url.contains("id=21622811"));
        assert_eq!(
            request.header("Referer"),
            Some("https://live.bilibili.com/")
        );
        assert_eq!(request.header("Cookie"), None, "匿名请求不应带 Cookie");
    }

    #[test]
    fn get_danmu_info_with_auth_sends_cookie() {
        let transport = FakeTransport::ok(DANMU_INFO);
        let auth = sample_auth();
        get_danmu_info_with_auth(&transport, 21622811, Some(&auth)).expect("夹具应可解析");
        let request = transport.only_request();
        let cookie = request.header("Cookie").expect("登录后应带 Cookie");
        assert!(cookie.contains("SESSDATA=sess-secret"));
    }

    #[test]
    fn get_danmu_info_maps_api_error() {
        let transport = FakeTransport::ok(r#"{"code":60004,"message":"房间不存在"}"#);
        match get_danmu_info(&transport, 99999999).unwrap_err() {
            DanmakuWsError::Api { code, message } => {
                assert_eq!(code, 60004);
                assert_eq!(message, "房间不存在");
            }
            other => panic!("应映射成 Api，实际 {other:?}"),
        }
    }

    #[test]
    fn get_danmu_info_maps_http_status() {
        let transport = FakeTransport::with_status(403, "");
        match get_danmu_info(&transport, 1).unwrap_err() {
            DanmakuWsError::Source(SourceError::Http(403)) => {}
            other => panic!("应映射成 HTTP 403，实际 {other:?}"),
        }
    }

    #[test]
    fn get_danmu_info_rejects_missing_token() {
        let transport = FakeTransport::ok(r#"{"code":0,"data":{"host_list":[]}}"#);
        assert!(matches!(
            get_danmu_info(&transport, 1),
            Err(DanmakuWsError::InvalidResponse(_))
        ));
    }

    #[test]
    fn malformed_json_is_a_readable_error() {
        let transport = FakeTransport::ok("{not json");
        let error = get_danmu_info(&transport, 1).unwrap_err();
        assert!(matches!(error, DanmakuWsError::InvalidResponse(_)));
        assert!(!format!("{error:?} {error}").contains("mock-token-secret"));
    }

    #[test]
    fn invalid_room_is_rejected_before_any_request() {
        let transport = FakeTransport::ok(DANMU_INFO);
        assert!(matches!(
            get_danmu_info(&transport, 0),
            Err(DanmakuWsError::InvalidRoom(_))
        ));
        assert_eq!(transport.request_count(), 0);
    }

    #[test]
    fn debug_does_not_print_token() {
        let info = parse_danmu_info(DANMU_INFO.as_bytes()).expect("夹具应可解析");
        let text = format!("{info:?}");
        assert!(!text.contains("mock-token-secret"), "Debug 不得输出 token");
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn wss_url_uses_default_port_and_sub_path() {
        let host = DanmakuHost {
            host: "broadcast.example.invalid".to_owned(),
            port: 2243,
            ws_port: 2244,
            wss_port: 443,
        };
        assert_eq!(host.wss_url(), "wss://broadcast.example.invalid/sub");

        let host = DanmakuHost {
            host: "backup.example.invalid".to_owned(),
            port: 2243,
            ws_port: 2244,
            wss_port: 8443,
        };
        assert_eq!(host.wss_url(), "wss://backup.example.invalid:8443/sub");
    }

    #[test]
    fn host_without_wss_port_falls_back_to_443() {
        let host = DanmakuHost {
            host: "example.invalid".to_owned(),
            port: 2243,
            ws_port: 2244,
            wss_port: 0,
        };
        assert_eq!(host.wss_port_or_default(), 443);
        assert_eq!(host.wss_url(), "wss://example.invalid/sub");
    }

    #[test]
    fn pick_wss_host_skips_empty_host() {
        let info = parse_danmu_info(DANMU_INFO.as_bytes()).expect("夹具应可解析");
        let host = pick_wss_host(info.hosts()).expect("应有可用条目");
        assert_eq!(host.host, "broadcast.example.invalid");
    }

    #[test]
    fn pick_wss_host_is_none_for_empty_list() {
        assert!(pick_wss_host(&[]).is_none());
        let all_empty = [DanmakuHost {
            host: "  ".to_owned(),
            port: 0,
            ws_port: 0,
            wss_port: 0,
        }];
        assert!(pick_wss_host(&all_empty).is_none());
    }

    #[test]
    fn run_rejects_cancelled_before_any_request() {
        let transport = FakeTransport::ok(DANMU_INFO);
        let cancel = AtomicBool::new(true);
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(matches!(
            run_danmaku_ws(&transport, 1, None, &cancel, &tx),
            Err(DanmakuWsError::Cancelled)
        ));
        assert_eq!(transport.request_count(), 0);
    }

    #[test]
    fn run_without_usable_host_does_not_connect() {
        let transport = FakeTransport::ok(EMPTY_HOST_LIST);
        let cancel = AtomicBool::new(false);
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(matches!(
            run_danmaku_ws(&transport, 1, None, &cancel, &tx),
            Err(DanmakuWsError::NoHost)
        ));
        assert_eq!(transport.request_count(), 1, "应只请求一次弹幕服务器信息");
    }

    #[test]
    fn connect_rejects_empty_host_without_dns() {
        let host = DanmakuHost {
            host: String::new(),
            port: 0,
            ws_port: 0,
            wss_port: 443,
        };
        let error = connect_wss(&host).unwrap_err();
        assert!(matches!(error, DanmakuWsError::Connect(_)));
        assert!(error.to_string().contains("地址为空"));
    }

    #[test]
    fn connect_timeout_is_mapped() {
        let error = map_connect_error(io::Error::from(io::ErrorKind::TimedOut));
        assert!(matches!(error, DanmakuWsError::Timeout(_)));
        assert!(error.to_string().contains("超时"));

        let error = map_connect_error(io::Error::from(io::ErrorKind::ConnectionRefused));
        assert!(matches!(error, DanmakuWsError::Connect(_)));
    }

    #[test]
    fn idle_expiry_respects_the_limit() {
        let start = Instant::now();
        assert!(!idle_expired(
            start,
            start + IDLE_TIMEOUT - Duration::from_millis(1),
            IDLE_TIMEOUT
        ));
        assert!(idle_expired(start, start + IDLE_TIMEOUT, IDLE_TIMEOUT));
    }

    #[test]
    fn plain_notification_yields_one_danmaku() {
        let outcome = parse_frame(&plain_notification(DANMU_JSON)).expect("明文包应可解析");
        assert_eq!(outcome.messages.len(), 1);
        let message = &outcome.messages[0];
        assert_eq!(message.user, "用户A");
        assert_eq!(message.text, "晚上好");
        assert_eq!(message.color, 16_777_215);
        assert_eq!(message.timestamp, 1_726_799_123_456);
        assert!(outcome.auth_code.is_none());
    }

    #[test]
    fn zlib_notification_is_decompressed() {
        let bytes = compressed_notification(PROTOVER_ZLIB, DANMU_JSON);
        let outcome = parse_frame(&bytes).expect("zlib 包应可解压");
        assert_eq!(outcome.messages.len(), 1);
        assert_eq!(outcome.messages[0].text, "晚上好");
    }

    #[test]
    fn brotli_notification_is_decompressed() {
        let bytes = compressed_notification(PROTOVER_BROTLI, DANMU_JSON);
        let outcome = parse_frame(&bytes).expect("brotli 包应可解压");
        assert_eq!(outcome.messages.len(), 1);
        assert_eq!(outcome.messages[0].user, "用户A");
    }

    #[test]
    fn frame_with_every_protover_is_parsed() {
        let mut bytes = plain_notification(DANMU_JSON);
        bytes.extend_from_slice(&compressed_notification(PROTOVER_ZLIB, DANMU_JSON));
        bytes.extend_from_slice(&compressed_notification(PROTOVER_BROTLI, DANMU_JSON));

        let outcome = parse_frame(&bytes).expect("三种协议版本应一起解析");
        assert_eq!(outcome.messages.len(), 3);
    }

    #[test]
    fn compressed_body_without_inner_header_is_accepted() {
        // 少数实现直接压缩单条 JSON，不套内层包头。
        let body = zlib(DANMU_JSON.as_bytes());
        let bytes = frame(PROTOVER_ZLIB, OP_NOTIFICATION, &body);
        let outcome = parse_frame(&bytes).expect("无内层包头也应可解");
        assert_eq!(outcome.messages.len(), 1);
    }

    #[test]
    fn auth_reply_code_is_parsed() {
        let outcome = parse_frame(&frame(PROTOVER_PLAIN, OP_AUTH_REPLY, br#"{"code":0}"#))
            .expect("认证回复应可解析");
        assert_eq!(outcome.auth_code, Some(0));
        assert!(outcome.messages.is_empty());

        let outcome = parse_frame(&frame(PROTOVER_PLAIN, OP_AUTH_REPLY, br#"{"code":-101}"#))
            .expect("认证失败回复也应可解析");
        assert_eq!(outcome.auth_code, Some(-101));
        assert!(matches!(
            check_auth_code(-101),
            Err(DanmakuWsError::AuthFailed { code: -101 })
        ));
    }

    #[test]
    fn heartbeat_reply_is_ignored() {
        let outcome = parse_frame(&frame(PROTOVER_PLAIN, OP_HEARTBEAT, b"[object Object]"))
            .expect("心跳包应可解析");
        assert!(outcome.messages.is_empty());
        assert!(outcome.auth_code.is_none());

        let outcome =
            parse_frame(&frame(PROTOVER_PLAIN, 3, &[0, 0, 0, 42])).expect("op=3 应可解析");
        assert!(outcome.messages.is_empty());
    }

    #[test]
    fn auth_packet_has_expected_fields() {
        let bytes = auth_packet(21622811, 42, "mock-token-secret");
        let packets = decode_packets(&bytes).expect("认证包应可解析");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].operation, OP_AUTH);
        let body: Value = serde_json::from_slice(&packets[0].body).expect("认证 body 必须是 JSON");
        assert_eq!(body["roomid"], serde_json::json!(21622811));
        assert_eq!(body["uid"], serde_json::json!(42));
        assert_eq!(body["protover"], serde_json::json!(CLIENT_PROTOVER));
        assert_eq!(body["platform"], serde_json::json!("web"));
        assert_eq!(body["type"], serde_json::json!(2));
        assert_eq!(body["key"], serde_json::json!("mock-token-secret"));
    }

    #[test]
    fn heartbeat_packet_is_op2_with_object_object() {
        let bytes = encode_packet(OP_HEARTBEAT, heartbeat_body());
        let packets = decode_packets(&bytes).expect("心跳包应可解析");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].operation, OP_HEARTBEAT);
        assert_eq!(packets[0].protover, PROTOVER_PLAIN);
        assert_eq!(packets[0].body, b"[object Object]");
    }

    #[test]
    fn bad_danmu_message_is_skipped_without_breaking_the_frame() {
        let mut bytes = plain_notification("{not json");
        bytes.extend_from_slice(&plain_notification(DANMU_JSON));

        let outcome = parse_frame(&bytes).expect("坏一条不应让整帧失败");
        assert_eq!(outcome.messages.len(), 1, "好的一条必须保留");
        assert_eq!(outcome.messages[0].text, "晚上好");
    }

    #[test]
    fn unknown_cmd_is_ignored() {
        let gift = serde_json::json!({"cmd": "SEND_GIFT", "data": {"giftName": "辣条"}});
        let body = serde_json::to_vec(&gift).expect("测试 JSON 应可序列化");
        let outcome =
            parse_frame(&frame(PROTOVER_PLAIN, OP_NOTIFICATION, &body)).expect("礼物包不应报错");
        assert!(outcome.messages.is_empty());
    }

    #[test]
    fn corrupt_zlib_is_a_readable_error() {
        let bytes = frame(PROTOVER_ZLIB, OP_NOTIFICATION, b"not zlib at all");
        let error = parse_frame(&bytes).unwrap_err();
        assert!(matches!(error, DanmakuWsError::Protocol(_)));
        let text = format!("{error} {error:?}");
        assert!(text.contains("zlib"), "错误应说明是哪一步失败，实际 {text}");
        assert!(!text.contains("mock-token-secret"));
    }

    #[test]
    fn unsupported_protover_is_rejected() {
        let bytes = frame(9, OP_NOTIFICATION, b"whatever");
        let error = parse_frame(&bytes).unwrap_err();
        assert!(matches!(error, DanmakuWsError::Protocol(_)));
        assert!(error.to_string().contains("protover=9"));
    }

    #[test]
    fn truncated_frame_is_a_protocol_error() {
        let mut bytes = plain_notification(DANMU_JSON);
        bytes.truncate(bytes.len() - 3);
        let error = parse_frame(&bytes).unwrap_err();
        assert!(matches!(error, DanmakuWsError::Protocol(_)));
        assert!(!format!("{error:?} {error}").contains("晚上好"));
    }

    #[test]
    fn malformed_auth_reply_is_a_readable_error() {
        let error = parse_frame(&frame(PROTOVER_PLAIN, OP_AUTH_REPLY, b"{oops")).unwrap_err();
        let text = error.to_string();
        assert!(matches!(error, DanmakuWsError::Protocol(_)));
        assert!(text.contains("认证回复"));
    }

    #[test]
    fn message_fields_survive_json_round_trip() {
        let value: Value = serde_json::from_str(DANMU_JSON).expect("测试夹具必须是合法 JSON");
        let json = serde_json::to_vec(&value).expect("应可序列化");
        let outcome =
            parse_frame(&frame(PROTOVER_PLAIN, OP_NOTIFICATION, &json)).expect("应可解析");
        assert_eq!(outcome.messages.len(), 1);
        assert_eq!(outcome.messages[0].text, "晚上好");
    }
}
