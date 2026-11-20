//! B 站扫码登录与凭证存储（T-0092a）。
//!
//! 登录态只用于 B 站直播 API / 取流，不用于音乐源。本模块：
//!
//! - 通过 [`generate_qr`] 取二维码登录 URL 与 `qrcode_key`；
//! - 通过 [`poll_qr`] 轮询扫码状态，成功后从 `Set-Cookie` 提取登录 cookie；
//! - 通过 [`AuthStore`] 在 Windows 上用 DPAPI 加密后原子写入调用方给定的路径，
//!   其它平台只保存在进程内存中，不落盘。
//!
//! 不在日志、`Display` 或 `Debug` 中输出 cookie 值、`Set-Cookie` 原文或二维码
//! 登录 URL。错误信息只描述失败类型。

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use np_sources::{SourceError, Transport};
use serde::Deserialize;
#[cfg(windows)]
use serde::Serialize;
use serde::de::DeserializeOwned;

/// 二维码生成接口。
const QR_GENERATE_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
/// 二维码状态轮询接口（追加 `qrcode_key`）。
const QR_POLL_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";
/// 登录接口要求的来源页；不带也可能成功，但带上更接近浏览器。
const BILI_REFERER: &str = "https://www.bilibili.com/";
/// 调用方轮询二维码状态建议的间隔（T-0092 §2.2）。
pub const QR_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// poll 未扫码。
const CODE_WAITING: i64 = 86101;
/// poll 已扫码待确认。
const CODE_SCANNED: i64 = 86090;
/// poll 二维码已过期。
const CODE_EXPIRED: i64 = 86038;
/// poll 成功。
const CODE_SUCCESS: i64 = 0;

/// 登录成功后需要保存的 cookie 名称（T-0092 §1）。
const SAVED_COOKIE_NAMES: [&str; 6] = [
    "SESSDATA",
    "bili_jct",
    "DedeUserID",
    "DedeUserID__ckMd5",
    "sid",
    "buvid3",
];

/// 扫码登录相关错误。
///
/// 各变体只包含状态码、固定说明或服务端业务消息，不携带 cookie 值、二维码
/// 登录 URL 或 `Set-Cookie` 原文。
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// 连接失败或超时。底层错误可能带 URL，因此只给固定说明。
    #[error("B 站登录网络请求失败")]
    Network,

    /// 登录接口返回非 2xx。
    #[error("B 站登录接口返回 HTTP {0}")]
    Http(u16),

    /// 风控拦截等临时不可用。
    #[error("B 站登录接口暂不可用")]
    Unavailable,

    /// 响应不是预期 JSON，或缺少必要字段。
    #[error("登录响应无效：{0}")]
    InvalidResponse(String),

    /// B 站接口业务错误。
    #[error("B 站登录接口错误（code={code}）：{message}")]
    Api { code: i64, message: String },

    /// 成功响应里缺少关键 cookie。
    #[error("登录成功响应缺少 {0}")]
    MissingCookie(&'static str),

    /// 没有可保存的 cookie。
    #[error("B 站凭证为空")]
    EmptyCredentials,

    /// 本地凭证文件读写 / 加解密失败。
    #[error("B 站凭证存储失败：{0}")]
    Storage(String),

    /// 其它传输层错误（下载、未实现等）。
    #[error("B 站登录请求失败：{0}")]
    Source(SourceError),
}

impl From<SourceError> for AuthError {
    fn from(error: SourceError) -> Self {
        match error {
            // reqwest 的 Display 会带请求 URL；登录 URL 同样不落到错误文本里。
            SourceError::Network(_) => Self::Network,
            SourceError::Http(status) => Self::Http(status),
            SourceError::Parse(_) => Self::InvalidResponse("响应不是有效的 JSON".to_owned()),
            SourceError::Unavailable(_) => Self::Unavailable,
            other => Self::Source(other),
        }
    }
}

/// 一次成功登录得到的凭证。
///
/// cookie 集合保持服务端返回顺序，`Debug` 只输出名称和 uid，不输出值。
#[derive(Clone, PartialEq, Eq)]
pub struct BiliAuth {
    cookies: Vec<(String, String)>,
    refresh_token: Option<String>,
}

impl BiliAuth {
    /// 构造凭证。空名称/空值的 cookie 会被丢弃，空 refresh token 视为没有。
    pub fn new(cookies: Vec<(String, String)>, refresh_token: Option<String>) -> Self {
        let cookies = cookies
            .into_iter()
            .filter(|(name, value)| !name.is_empty() && !value.is_empty())
            .collect();
        Self {
            cookies,
            refresh_token: refresh_token.filter(|token| !token.is_empty()),
        }
    }

    /// 已保存的 cookie 列表。
    pub fn cookies(&self) -> &[(String, String)] {
        &self.cookies
    }

    /// 按名称取 cookie 值。
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(cookie_name, _)| cookie_name == name)
            .map(|(_, value)| value.as_str())
    }

    /// 可选的 refresh token。
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// 从 `DedeUserID` 解析的 uid；缺失或格式不对时为 `None`。
    pub fn uid(&self) -> Option<u64> {
        self.cookie("DedeUserID")
            .and_then(|value| value.parse().ok())
    }

    /// 组装请求 `Cookie` 头。
    pub fn cookie_header(&self) -> String {
        let mut header = String::new();
        for (name, value) in &self.cookies {
            if !header.is_empty() {
                header.push_str("; ");
            }
            header.push_str(name);
            header.push('=');
            header.push_str(value);
        }
        header
    }

    /// 是否没有任何 cookie。
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

impl fmt::Debug for BiliAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.cookies.iter().map(|(name, _)| name.as_str()).collect();
        formatter
            .debug_struct("BiliAuth")
            .field("cookie_names", &names)
            .field("uid", &self.uid())
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// `generate` 的结果：二维码内容（登录 URL）与轮询 key。
#[derive(Clone, PartialEq, Eq)]
pub struct QrLogin {
    /// 二维码要编码的登录 URL。
    pub qr_url: String,
    /// 后续 `poll` 使用的 key。
    pub qrcode_key: String,
}

impl fmt::Debug for QrLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QrLogin")
            .field("qr_url", &"<redacted>")
            .field("qrcode_key", &"<redacted>")
            .finish()
    }
}

/// 一次 poll 的状态。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum QrStatus {
    /// 86101：尚未扫码。
    Waiting,
    /// 86090：已扫码，等待手机确认。
    Scanned,
    /// 86038：二维码过期。
    Expired,
    /// 0：登录成功，附带 cookie。
    Success(BiliAuth),
}

/// 取二维码登录 URL 与 `qrcode_key`。
///
/// 只发一次请求；轮询由调用方按 2s 间隔调用 [`poll_qr`]。
pub fn generate_qr(transport: &dyn Transport) -> Result<QrLogin, AuthError> {
    let response = transport.get_with_headers(QR_GENERATE_URL, &[("Referer", BILI_REFERER)])?;
    ensure_http_ok(response.status)?;
    let envelope: Envelope<GenerateData> = decode_json(&response.body, "二维码生成")?;
    ensure_code_ok(&envelope)?;
    let data = envelope
        .data
        .ok_or_else(|| invalid("二维码生成响应缺少 data"))?;
    if data.url.is_empty() || data.qrcode_key.is_empty() {
        return Err(invalid("二维码生成响应缺少 url 或 qrcode_key"));
    }
    Ok(QrLogin {
        qr_url: data.url,
        qrcode_key: data.qrcode_key,
    })
}

/// 轮询扫码状态。
///
/// 成功时从响应的 `Set-Cookie` 头中提取 [`SAVED_COOKIE_NAMES`]；缺少 SESSDATA
/// 视为无效成功响应。调用方负责 2s 间隔与超时。
pub fn poll_qr(transport: &dyn Transport, qrcode_key: &str) -> Result<QrStatus, AuthError> {
    if qrcode_key.is_empty() || !qrcode_key.bytes().all(is_url_safe) {
        return Err(invalid("二维码 key 无效"));
    }
    let url = format!("{QR_POLL_URL}?qrcode_key={qrcode_key}");
    let response = transport.get_with_headers(&url, &[("Referer", BILI_REFERER)])?;
    ensure_http_ok(response.status)?;
    let envelope: Envelope<PollData> = decode_json(&response.body, "扫码状态")?;

    // 少数网关会把状态码放在顶层 code；与 data.code 一样映射。
    if envelope.code != 0 {
        return match envelope.code {
            CODE_WAITING => Ok(QrStatus::Waiting),
            CODE_SCANNED => Ok(QrStatus::Scanned),
            CODE_EXPIRED => Ok(QrStatus::Expired),
            code => Err(AuthError::Api {
                code,
                message: normalized_message(&envelope.message),
            }),
        };
    }

    let data = envelope
        .data
        .ok_or_else(|| invalid("扫码状态响应缺少 data"))?;
    match data.code {
        CODE_WAITING => Ok(QrStatus::Waiting),
        CODE_SCANNED => Ok(QrStatus::Scanned),
        CODE_EXPIRED => Ok(QrStatus::Expired),
        CODE_SUCCESS => {
            let saved = select_saved_cookies(parse_set_cookie(&response.headers));
            if !saved
                .iter()
                .any(|(name, value)| name == "SESSDATA" && !value.is_empty())
            {
                return Err(AuthError::MissingCookie("SESSDATA"));
            }
            let refresh_token = (!data.refresh_token.is_empty()).then_some(data.refresh_token);
            Ok(QrStatus::Success(BiliAuth::new(saved, refresh_token)))
        }
        code => Err(AuthError::Api {
            code,
            message: normalized_message(&data.message),
        }),
    }
}

/// 解析所有 `Set-Cookie` 响应头，返回 `(name, value)`。
///
/// 支持多个同名头，也支持一个头里用逗号拼接多个 cookie（`Expires` 里的逗号
/// 不会被误拆）。只取第一个分号前的值，忽略 `Path` / `Domain` / `Expires` 等属性。
pub fn parse_set_cookie(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut cookies = Vec::new();
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        for segment in split_set_cookie_header(value) {
            let Some((cookie_name, cookie_value)) = segment.split_once('=') else {
                continue;
            };
            let cookie_name = cookie_name.trim();
            if cookie_name.is_empty() {
                continue;
            }
            let cookie_value = cookie_value.split(';').next().unwrap_or("").trim();
            cookies.push((cookie_name.to_owned(), cookie_value.to_owned()));
        }
    }
    cookies
}

/// 只保留登录需要的 cookie；同名 cookie 后出现的值覆盖先出现的值。
fn select_saved_cookies(parsed: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut saved: Vec<(String, String)> = Vec::new();
    for (name, value) in parsed {
        if !SAVED_COOKIE_NAMES.contains(&name.as_str()) {
            continue;
        }
        if let Some(existing) = saved
            .iter_mut()
            .find(|(existing_name, _)| *existing_name == name)
        {
            existing.1 = value;
        } else {
            saved.push((name, value));
        }
    }
    saved
}

/// 按逗号拆分一个 `Set-Cookie` 头，但只在逗号后确实跟着 `name=` 时拆。
fn split_set_cookie_header(header: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, byte) in header.bytes().enumerate() {
        if byte != b',' {
            continue;
        }
        let rest = header[index + 1..].trim_start();
        if looks_like_cookie_pair(rest) {
            segments.push(&header[start..index]);
            start = index + 1;
        }
    }
    segments.push(&header[start..]);
    segments
}

fn looks_like_cookie_pair(text: &str) -> bool {
    let Some((name, _)) = text.split_once('=') else {
        return false;
    };
    let name = name.trim_start();
    !name.is_empty() && name.bytes().all(is_cookie_name_byte)
}

fn is_cookie_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn is_url_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn ensure_http_ok(status: u16) -> Result<(), AuthError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(AuthError::Http(status))
    }
}

fn decode_json<T: DeserializeOwned>(body: &[u8], label: &str) -> Result<T, AuthError> {
    serde_json::from_slice(body)
        .map_err(|error| AuthError::InvalidResponse(format!("{label} JSON 解析失败：{error}")))
}

fn ensure_code_ok<T>(envelope: &Envelope<T>) -> Result<(), AuthError> {
    if envelope.code == 0 {
        Ok(())
    } else {
        Err(AuthError::Api {
            code: envelope.code,
            message: normalized_message(&envelope.message),
        })
    }
}

fn normalized_message(message: &str) -> String {
    if message.is_empty() {
        "登录接口请求失败".to_owned()
    } else {
        message.to_owned()
    }
}

fn invalid(message: &str) -> AuthError {
    AuthError::InvalidResponse(message.to_owned())
}

#[derive(Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Deserialize)]
struct GenerateData {
    url: String,
    qrcode_key: String,
}

#[derive(Deserialize)]
struct PollData {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    refresh_token: String,
}

/// DPAPI 加密的凭证存储。
///
/// 路径由调用方传入（主线使用 `%APPDATA%\com.nowhere.player\bili-auth.bin`）。
/// Windows 上 `save` 先加密再以临时文件 + rename 原子替换；`load` 解密并反序列化；
/// `clear` 删除文件。非 Windows 平台只在进程内存中保存，不访问文件系统。
pub struct AuthStore {
    path: PathBuf,
}

impl AuthStore {
    /// 以给定路径创建存储（非 Windows 上路径只作为内存键）。
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// 调用方给定的文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(windows)]
const STORAGE_VERSION: u8 = 1;

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
struct StoredAuth {
    version: u8,
    cookies: Vec<(String, String)>,
    refresh_token: Option<String>,
}

#[cfg(windows)]
impl AuthStore {
    /// 读取凭证；文件不存在时返回 `Ok(None)`。
    pub fn load(&self) -> Result<Option<BiliAuth>, AuthError> {
        match std::fs::read(&self.path) {
            Ok(blob) => {
                if blob.is_empty() {
                    return Err(AuthError::Storage("凭证文件为空".to_owned()));
                }
                let plain = dpapi::unprotect(&blob)?;
                decode_stored(&plain).map(Some)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AuthError::Storage(format!("读取凭证文件失败：{error}"))),
        }
    }

    /// 加密并原子写入凭证。
    pub fn save(&self, auth: &BiliAuth) -> Result<(), AuthError> {
        let plain = encode_stored(auth)?;
        let blob = dpapi::protect(&plain)?;
        write_atomic(&self.path, &blob)
    }

    /// 删除凭证文件；不存在时视为成功。
    pub fn clear(&self) -> Result<(), AuthError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AuthError::Storage(format!("删除凭证文件失败：{error}"))),
        }
    }
}

#[cfg(windows)]
fn encode_stored(auth: &BiliAuth) -> Result<Vec<u8>, AuthError> {
    if auth.is_empty() {
        return Err(AuthError::EmptyCredentials);
    }
    let stored = StoredAuth {
        version: STORAGE_VERSION,
        cookies: auth.cookies.clone(),
        refresh_token: auth.refresh_token.clone(),
    };
    serde_json::to_vec(&stored)
        .map_err(|error| AuthError::Storage(format!("凭证序列化失败：{error}")))
}

#[cfg(windows)]
fn decode_stored(plain: &[u8]) -> Result<BiliAuth, AuthError> {
    let stored: StoredAuth = serde_json::from_slice(plain)
        .map_err(|_| AuthError::Storage("凭证文件内容损坏".to_owned()))?;
    if stored.version != STORAGE_VERSION {
        return Err(AuthError::Storage("不支持的凭证文件版本".to_owned()));
    }
    let auth = BiliAuth::new(stored.cookies, stored.refresh_token);
    if auth.is_empty() {
        return Err(AuthError::EmptyCredentials);
    }
    Ok(auth)
}

#[cfg(windows)]
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| AuthError::Storage(format!("创建凭证目录失败：{error}")))?;

    let temporary = temporary_path(path);
    let result = write_temporary_then_rename(&temporary, path, bytes);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(AuthError::Storage(format!("写入凭证文件失败：{error}")));
    }
    Ok(())
}

#[cfg(windows)]
fn write_temporary_then_rename(
    temporary: &Path,
    target: &Path,
    bytes: &[u8],
) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, target)
}

#[cfg(windows)]
fn temporary_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map_or_else(
        || "bili-auth.bin".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temporary = format!(".{name}.tmp-{}-{sequence}", std::process::id());
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(temporary),
        _ => PathBuf::from(temporary),
    }
}

#[cfg(windows)]
mod dpapi {
    use std::io;

    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use super::AuthError;

    /// 用当前用户的 DPAPI 主密钥加密。
    pub fn protect(data: &[u8]) -> Result<Vec<u8>, AuthError> {
        if data.is_empty() {
            return Err(AuthError::Storage("凭证为空，拒绝加密".to_owned()));
        }
        let length = u32::try_from(data.len())
            .map_err(|_| AuthError::Storage("凭证过大，无法加密".to_owned()))?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: length,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: `input` 的 `pbData` 指向 `data`，在本调用期间有效且未被移动；
        // 其余可选参数按文档传空；`output` 已初始化为零，成功时由 DPAPI 填充
        // 一块必须用 `LocalFree` 释放的内存。
        let result = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if result == 0 {
            return Err(last_error("DPAPI 加密"));
        }
        copy_and_free(output)
    }

    /// 用当前用户的 DPAPI 主密钥解密。
    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, AuthError> {
        if data.is_empty() {
            return Err(AuthError::Storage("凭证文件为空".to_owned()));
        }
        let length = u32::try_from(data.len())
            .map_err(|_| AuthError::Storage("凭证过大，无法解密".to_owned()))?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: length,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        // SAFETY: `input` 的 `pbData` 指向 `data`，在本调用期间有效且未被移动；
        // 描述字符串指针按文档传空；`output` 已初始化为零，成功时由 DPAPI 填充
        // 一块必须用 `LocalFree` 释放的内存。
        let result = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if result == 0 {
            return Err(last_error("DPAPI 解密"));
        }
        copy_and_free(output)
    }

    fn copy_and_free(output: CRYPT_INTEGER_BLOB) -> Result<Vec<u8>, AuthError> {
        if output.pbData.is_null() {
            return Err(AuthError::Storage("DPAPI 返回空指针".to_owned()));
        }
        let data = if output.cbData == 0 {
            Vec::new()
        } else {
            // SAFETY: DPAPI 成功后保证 `pbData` 指向 `cbData` 个可读字节；这里立即
            // 复制，不复用指针。调用期间没有其它线程持有该缓冲区。
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
        };
        // SAFETY: 该缓冲区由 `CryptProtectData` / `CryptUnprotectData` 分配，
        // 文档要求用 `LocalFree` 释放；每个成功返回的指针只释放一次。
        unsafe {
            let _ = LocalFree(output.pbData as HLOCAL);
        }
        if data.is_empty() {
            return Err(AuthError::Storage("DPAPI 返回空数据".to_owned()));
        }
        Ok(data)
    }

    fn last_error(action: &str) -> AuthError {
        let error = io::Error::last_os_error();
        let detail = match error.raw_os_error() {
            Some(code) => format!("{action}失败（Windows 错误 {code}）"),
            None => format!("{action}失败：{error}"),
        };
        AuthError::Storage(detail)
    }
}

#[cfg(not(windows))]
impl AuthStore {
    /// 从进程内存读取凭证，不访问文件系统。
    pub fn load(&self) -> Result<Option<BiliAuth>, AuthError> {
        let store = memory_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(store.get(&self.path).cloned())
    }

    /// 只写入进程内存，不落盘。
    pub fn save(&self, auth: &BiliAuth) -> Result<(), AuthError> {
        if auth.is_empty() {
            return Err(AuthError::EmptyCredentials);
        }
        let mut store = memory_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store.insert(self.path.clone(), auth.clone());
        Ok(())
    }

    /// 从进程内存移除，不访问文件系统。
    pub fn clear(&self) -> Result<(), AuthError> {
        let mut store = memory_store()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store.remove(&self.path);
        Ok(())
    }
}

#[cfg(not(windows))]
fn memory_store() -> &'static std::sync::Mutex<std::collections::BTreeMap<PathBuf, BiliAuth>> {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    static STORE: OnceLock<Mutex<BTreeMap<PathBuf, BiliAuth>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use np_sources::{HttpResponse, SourceError};

    use super::*;

    #[derive(Clone)]
    struct RecordedRequest {
        url: String,
        headers: Vec<(String, String)>,
    }

    struct FakeTransport {
        responses: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<RecordedRequest>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn last_request(&self) -> (String, Vec<(String, String)>) {
            let request = self
                .requests
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("FakeTransport 应记录请求");
            (request.url, request.headers)
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, _url: &str) -> Result<Vec<u8>, SourceError> {
            Err(SourceError::NotImplemented(
                "FakeTransport 只实现 get_with_headers".to_owned(),
            ))
        }

        fn get_with_headers(
            &self,
            url: &str,
            headers: &[(&str, &str)],
        ) -> Result<HttpResponse, SourceError> {
            self.requests.lock().unwrap().push(RecordedRequest {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            });
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| SourceError::NotImplemented("FakeTransport 响应已耗尽".to_owned()))
        }
    }

    fn http_response(status: u16, headers: &[(&str, &str)], body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn sample_auth() -> BiliAuth {
        BiliAuth::new(
            vec![
                ("SESSDATA".to_owned(), "sess-secret".to_owned()),
                ("bili_jct".to_owned(), "csrf-secret".to_owned()),
                ("DedeUserID".to_owned(), "42".to_owned()),
                ("DedeUserID__ckMd5".to_owned(), "ck-secret".to_owned()),
                ("sid".to_owned(), "sid-secret".to_owned()),
                ("buvid3".to_owned(), "buvid-secret".to_owned()),
            ],
            Some("refresh-secret".to_owned()),
        )
    }

    fn success_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "set-cookie",
                "SESSDATA=sess-secret; Path=/; Domain=.bilibili.com; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Secure; HttpOnly",
            ),
            ("Set-Cookie", "bili_jct=csrf-secret; Path=/"),
            ("set-cookie", "DedeUserID=42; Path=/"),
            ("set-cookie", "DedeUserID__ckMd5=ck-secret; Path=/"),
            ("set-cookie", "sid=sid-secret; Path=/"),
            ("set-cookie", "buvid3=buvid-secret; Path=/"),
            ("set-cookie", "buvid4=not-saved; Path=/"),
            ("content-type", "application/json"),
        ]
    }

    fn success_body() -> &'static str {
        r#"{"code":0,"message":"0","data":{"url":"https://passport.bilibili.com/h5-app/passport/login/scan?navhide=1","refresh_token":"refresh-secret","timestamp":123,"code":0,"message":""}}"#
    }

    #[test]
    fn generate_returns_url_and_key_without_logging_them() {
        let body = r#"{"code":0,"message":"0","data":{"url":"https://passport.bilibili.com/h5-app/passport/login/scan?navhide=1","qrcode_key":"qr-key-123"}}"#;
        let transport = FakeTransport::new(vec![http_response(200, &[], body)]);

        let login = generate_qr(&transport).expect("generate 应成功");
        assert_eq!(
            login.qr_url,
            "https://passport.bilibili.com/h5-app/passport/login/scan?navhide=1"
        );
        assert_eq!(login.qrcode_key, "qr-key-123");
        let (url, headers) = transport.last_request();
        assert_eq!(url, QR_GENERATE_URL);
        assert!(
            headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("referer") && value == BILI_REFERER)
        );

        let debug = format!("{login:?}");
        assert!(!debug.contains("passport.bilibili.com"));
        assert!(!debug.contains("qr-key-123"));
    }

    #[test]
    fn poll_maps_waiting_scanned_and_expired() {
        for (code, expected) in [
            (CODE_WAITING, QrStatus::Waiting),
            (CODE_SCANNED, QrStatus::Scanned),
            (CODE_EXPIRED, QrStatus::Expired),
        ] {
            let body = format!(
                r#"{{"code":0,"message":"0","data":{{"code":{code},"message":"","refresh_token":""}}}}"#
            );
            let transport = FakeTransport::new(vec![http_response(200, &[], &body)]);
            assert_eq!(poll_qr(&transport, "qr-key-123").unwrap(), expected);
            let (url, _) = transport.last_request();
            assert_eq!(url, format!("{QR_POLL_URL}?qrcode_key=qr-key-123"));
        }
    }

    #[test]
    fn poll_success_saves_expected_cookies_only() {
        let transport =
            FakeTransport::new(vec![http_response(200, &success_headers(), success_body())]);

        let status = poll_qr(&transport, "qr-key-123").expect("poll 应成功");
        let QrStatus::Success(auth) = status else {
            panic!("应返回 Success，实际 {status:?}");
        };

        assert_eq!(auth.cookies().len(), 6);
        assert_eq!(auth.uid(), Some(42));
        assert_eq!(auth.refresh_token(), Some("refresh-secret"));
        assert_eq!(auth.cookie("buvid4"), None);
        assert_eq!(
            auth.cookie_header(),
            "SESSDATA=sess-secret; bili_jct=csrf-secret; DedeUserID=42; \
             DedeUserID__ckMd5=ck-secret; sid=sid-secret; buvid3=buvid-secret"
        );

        let debug = format!("{auth:?}");
        for secret in [
            "sess-secret",
            "csrf-secret",
            "ck-secret",
            "sid-secret",
            "buvid-secret",
            "refresh-secret",
        ] {
            assert!(!debug.contains(secret), "Debug 不应包含 {secret}");
        }
        assert!(debug.contains("SESSDATA"));
    }

    #[test]
    fn parses_multiple_and_combined_set_cookie_headers() {
        let headers = vec![
            (
                "Set-Cookie".to_owned(),
                "a=1; Path=/; Expires=Wed, 21 Oct 2026 07:28:00 GMT".to_owned(),
            ),
            (
                "set-cookie".to_owned(),
                "b=2; Path=/, c=3; Path=/; HttpOnly".to_owned(),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ];

        assert_eq!(
            parse_set_cookie(&headers),
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned()),
                ("c".to_owned(), "3".to_owned()),
            ]
        );
    }

    #[test]
    fn cookie_header_and_uid_handle_missing_values() {
        let auth = BiliAuth::new(
            vec![
                ("SESSDATA".to_owned(), "value".to_owned()),
                ("DedeUserID".to_owned(), "not-a-number".to_owned()),
            ],
            None,
        );
        assert_eq!(auth.uid(), None);
        assert_eq!(
            auth.cookie_header(),
            "SESSDATA=value; DedeUserID=not-a-number"
        );
        assert!(BiliAuth::new(Vec::new(), None).is_empty());
    }

    #[test]
    fn empty_body_and_bad_json_are_readable_errors() {
        let transport = FakeTransport::new(vec![http_response(200, &[], "")]);
        assert!(matches!(
            generate_qr(&transport),
            Err(AuthError::InvalidResponse(_))
        ));

        let transport = FakeTransport::new(vec![http_response(200, &[], "{not json")]);
        assert!(matches!(
            poll_qr(&transport, "qr-key-123"),
            Err(AuthError::InvalidResponse(_))
        ));
    }

    #[test]
    fn non_2xx_and_api_errors_do_not_leak_headers() {
        let transport = FakeTransport::new(vec![http_response(500, &[], "server error")]);
        assert!(matches!(generate_qr(&transport), Err(AuthError::Http(500))));

        let body = r#"{"code":-400,"message":"请求错误","data":null}"#;
        let transport = FakeTransport::new(vec![http_response(200, &[], body)]);
        match generate_qr(&transport) {
            Err(AuthError::Api { code, message }) => {
                assert_eq!(code, -400);
                assert_eq!(message, "请求错误");
            }
            other => panic!("应返回 Api 错误，实际 {other:?}"),
        }
    }

    #[test]
    fn success_without_sessdata_is_rejected() {
        let body = r#"{"code":0,"message":"0","data":{"code":0,"message":"","refresh_token":""}}"#;
        let headers = [("set-cookie", "bili_jct=csrf-secret; Path=/")];
        let transport = FakeTransport::new(vec![http_response(200, &headers, body)]);

        match poll_qr(&transport, "qr-key-123") {
            Err(error @ AuthError::MissingCookie("SESSDATA")) => {
                assert!(!error.to_string().contains("csrf-secret"));
            }
            other => panic!("应返回 MissingCookie，实际 {other:?}"),
        }
    }

    #[test]
    fn save_rejects_empty_credentials() {
        let store = AuthStore::new(std::env::temp_dir().join("np-live-empty-auth.bin"));
        assert!(matches!(
            store.save(&BiliAuth::new(Vec::new(), None)),
            Err(AuthError::EmptyCredentials)
        ));
    }

    fn unique_directory(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "np-live-auth-{}-{name}-{nanos}",
            std::process::id()
        ))
    }

    #[cfg(windows)]
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_save_load_clear() {
        let directory = unique_directory("roundtrip");
        let path = directory.join("bili-auth.bin");
        let store = AuthStore::new(&path);
        let auth = sample_auth();

        store.save(&auth).expect("DPAPI 保存应成功");
        assert!(path.exists());

        let raw = std::fs::read(&path).expect("应能读取加密文件");
        assert!(!contains(&raw, b"sess-secret"));
        assert!(!contains(&raw, b"csrf-secret"));
        assert!(!contains(&raw, b"refresh-secret"));

        let loaded = store.load().expect("DPAPI 读取应成功").expect("应有凭证");
        assert_eq!(loaded, auth);

        store.clear().expect("清除应成功");
        assert!(store.load().expect("清除后读取应成功").is_none());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(windows)]
    #[test]
    fn corrupted_and_empty_files_are_errors() {
        let directory = unique_directory("corrupt");
        let path = directory.join("bili-auth.bin");
        std::fs::create_dir_all(&directory).expect("应能创建测试目录");
        let store = AuthStore::new(&path);

        std::fs::write(&path, b"not a dpapi blob").expect("应能写入损坏文件");
        let error = store.load().expect_err("损坏文件应报错");
        assert!(matches!(error, AuthError::Storage(_)));
        assert!(!error.to_string().contains("not a dpapi blob"));

        std::fs::write(&path, b"").expect("应能写入空文件");
        assert!(matches!(store.load(), Err(AuthError::Storage(_))));

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(windows)]
    #[test]
    fn missing_file_is_none() {
        let directory = unique_directory("missing");
        let store = AuthStore::new(directory.join("bili-auth.bin"));
        assert!(store.load().expect("缺少文件不应报错").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_store_is_memory_only() {
        let path = unique_directory("memory").join("bili-auth.bin");
        let store = AuthStore::new(&path);
        let auth = sample_auth();

        store.save(&auth).expect("内存保存应成功");
        assert!(!path.exists(), "非 Windows 不得落盘");
        assert_eq!(store.load().expect("内存读取应成功"), Some(auth));
        store.clear().expect("内存清除应成功");
        assert!(store.load().expect("清除后读取应成功").is_none());
    }
}
