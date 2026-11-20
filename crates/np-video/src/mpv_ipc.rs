//! 最小 mpv IPC 客户端：音量/暂停控制 + 缓存遥测 + speed 追帧 + 无缝换流。
//!
//! 连接成功后观察 `demuxer-cache-time` / `playback-time` 等属性，每秒把
//! 缓存差写进 `mpv-cache` 日志；缓存高于目标时用 `set_property speed` 轻微
//! 加速（不 seek、不重载），低于目标一半时降到 0.60x，速度只在档位变化时
//! 下发一次。mpv 音视频共用同一时钟，所以只调 speed。
//!
//! 清晰度切换走同一进程的 `loadfile <url> replace`：先记下请求 id，reader
//! 把 `{"request_id":N,"error":"..."}` 写进共享槽位，宿主线程轮询结果，决定
//! 保留新流还是回退旧 URL。URL 只出现在管道里，不写日志。
//!
//! mpv 通过 `--input-ipc-server=\\.\pipe\nowhere-video` 暴露命名管道。
//! 连接放在后台线程里重试，窗口线程不阻塞；命令排队，连接成功后依次写出。
//! reader 线程持有命令 channel 的克隆，用它给 writer 线程发 speed。

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// cache 遥测还没就绪时的哨兵值。
const NO_CACHE: u64 = u64::MAX;

/// 一次清晰度换流的结果槽；`None` 表示请求还没回复。
type LoadSlot = Arc<Mutex<Option<(u64, Result<(), String>)>>>;

#[derive(Clone)]
pub struct MpvIpc {
    sender: Sender<String>,
    cache_ms: Arc<AtomicU64>,
    load: LoadSlot,
}

impl MpvIpc {
    /// 立即返回；命名管道在后台连接，失败只记日志。
    ///
    /// `target_ms` 是追帧目标缓冲（`live.buffer_ms`），只影响 speed 决策。
    pub fn connect(target_ms: u64) -> Self {
        let (sender, receiver) = mpsc::channel();
        let speed_sender = sender.clone();
        let cache_ms = Arc::new(AtomicU64::new(NO_CACHE));
        let load: LoadSlot = Arc::new(Mutex::new(None));
        let thread_cache = cache_ms.clone();
        let thread_load = load.clone();
        std::thread::Builder::new()
            .name("np-video-mpv-ipc".to_owned())
            .spawn(move || run(receiver, speed_sender, target_ms, thread_cache, thread_load))
            .ok();
        Self {
            sender,
            cache_ms,
            load,
        }
    }

    pub fn set_volume(&self, volume: u8) {
        self.send(format!(
            r#"{{"command":["set_property","volume",{}]}}"#,
            volume.min(100)
        ));
    }

    pub fn set_paused(&self, paused: bool) {
        self.send(format!(
            r#"{{"command":["set_property","pause",{}]}}"#,
            if paused { "true" } else { "false" }
        ));
    }

    /// 让现有 mpv 进程换流；不重启进程、不重建窗口。
    ///
    /// `request_id` 由宿主递增分配，reader 只把带相同 id 的回复写回槽位。
    pub fn loadfile(&self, url: &str, request_id: u64) {
        if let Ok(mut slot) = self.load.lock() {
            *slot = None;
        }
        // 只记录请求 id / 长度，URL 不进日志。
        log::info!(
            target: "mpv-ipc",
            "event=loadfile request_id={request_id} bytes={}",
            url.len()
        );
        self.send(format!(
            r#"{{"command":["loadfile",{},"replace"],"request_id":{request_id}}}"#,
            json_string(url)
        ));
    }

    /// 取 `request_id` 对应的 `loadfile` 回复；还没回复或 id 不匹配时 `None`。
    pub fn load_result(&self, request_id: u64) -> Option<Result<(), String>> {
        let mut slot = self.load.lock().ok()?;
        if slot.as_ref().is_some_and(|(id, _)| *id == request_id) {
            slot.take().map(|(_, result)| result)
        } else {
            None
        }
    }

    /// 当前缓存余量（毫秒）；遥测还没就绪时为 `None`。
    pub fn cache_ms(&self) -> Option<u64> {
        let value = self.cache_ms.load(Ordering::Relaxed);
        (value != NO_CACHE).then_some(value)
    }

    fn send(&self, command: String) {
        if self.sender.send(command).is_err() {
            log::warn!(target: "mpv-ipc", "send failed: ipc thread stopped");
        }
    }
}

/// JSON 字符串转义：URL 只在这里进入 IPC，引号 / 反斜杠必须转义。
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control.is_control() => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// 只读缓存遥测；还没收到属性时为 `None`。
#[derive(Default)]
struct Telemetry {
    cache_end: Option<f64>,
    playback: Option<f64>,
    cache_duration: Option<f64>,
    speed: Option<f64>,
    buffering: Option<f64>,
    last_log: Option<Instant>,
}

impl Telemetry {
    fn update(&mut self, name: &str, value: Option<f64>) {
        match name {
            "demuxer-cache-time" => self.cache_end = value,
            "playback-time" => self.playback = value,
            "demuxer-cache-duration" => self.cache_duration = value,
            "speed" => self.speed = value,
            "cache-buffering-state" => self.buffering = value,
            _ => {}
        }
    }

    /// `loadfile` 后属性属于旧文件，全部清空，等新流重新上报。
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn should_log(&mut self) -> bool {
        let due = self
            .last_log
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(1));
        if due {
            self.last_log = Some(Instant::now());
        }
        due
    }

    /// 缓冲余量（毫秒）：优先用 `demuxer-cache-time - playback-time`，
    /// 属性还没到齐时退回 `demuxer-cache-duration`；都未知时 `None`。
    fn cache_ms(&self) -> Option<f64> {
        match (self.cache_end, self.playback) {
            (Some(end), Some(playback)) => Some((end - playback).max(0.0) * 1_000.0),
            _ => self.cache_duration.map(|duration| duration * 1_000.0),
        }
    }

    fn log(&self, target_ms: u64) {
        match self.cache_ms() {
            Some(cache_ms) => log::info!(
                target: "mpv-cache",
                "cache_ms={cache_ms:.0} target_ms={target_ms} cache_end={:?} playback={:?} cache_duration={:?} speed={:?} buffering={:?}",
                self.cache_end,
                self.playback,
                self.cache_duration,
                self.speed,
                self.buffering
            ),
            None => log::debug!(target: "mpv-cache", "cache properties not ready"),
        }
    }
}

/// 追帧速度档：与 np-live 的脚本策略一致，只调速、不 seek、不重载。
const NORMAL_SPEED: f64 = 1.0;
const SLOW_SPEED: f64 = 0.60;
const SOFT_SPEED: f64 = 1.10;
const MID_SPEED: f64 = 1.20;
const HIGH_SPEED: f64 = 1.30;
/// 缓冲超过 target+1s / target+2s 时再上一档。
const SPEEDUP_MID_MS: f64 = 1_000.0;
const SPEEDUP_HIGH_MS: f64 = 2_000.0;

/// 追帧速度：mpv 音视频同一时钟，只调 speed 就能同时追音画。
///
/// ```text
/// cache > target + 2s → 1.30
/// cache > target + 1s → 1.20
/// cache > target      → 1.10
/// cache < target / 2  → 0.60
/// 其余                → 1.00
/// ```
fn desired_speed(cache_ms: f64, target_ms: u64) -> f64 {
    let target = target_ms as f64;
    if cache_ms > target + SPEEDUP_HIGH_MS {
        HIGH_SPEED
    } else if cache_ms > target + SPEEDUP_MID_MS {
        MID_SPEED
    } else if cache_ms > target {
        SOFT_SPEED
    } else if cache_ms < target / 2.0 {
        SLOW_SPEED
    } else {
        NORMAL_SPEED
    }
}

/// 一次速度变化：`from != to`，只在档位变化时产生。
#[derive(Debug, Clone, Copy, PartialEq)]
struct SpeedChange {
    from: f64,
    to: f64,
    cache_ms: f64,
}

impl SpeedChange {
    /// 发往 mpv 的 `set_property` 命令，JSON 一行。
    fn command(&self) -> String {
        format!(r#"{{"command":["set_property","speed",{:.2}]}}"#, self.to)
    }

    fn log(&self, target_ms: u64) {
        log::info!(
            target: "mpv-cache",
            "event=speed_set from={:.2} to={:.2} cache_ms={:.0} target_ms={target_ms}",
            self.from,
            self.to,
            self.cache_ms
        );
    }
}

/// 每秒一次的速度决策；只在速度实际变化时下发，避免抖动。
struct SpeedTuner {
    target_ms: u64,
    speed: f64,
}

impl SpeedTuner {
    fn new(target_ms: u64) -> Self {
        Self {
            target_ms,
            speed: NORMAL_SPEED,
        }
    }

    /// `cache_ms` 未知时保持 mpv 默认 1.0，不改变速度。
    fn update(&mut self, cache_ms: Option<f64>) -> Option<SpeedChange> {
        let cache_ms = cache_ms?;
        let desired = desired_speed(cache_ms, self.target_ms);
        if desired == self.speed {
            return None;
        }
        let change = SpeedChange {
            from: self.speed,
            to: desired,
            cache_ms,
        };
        self.speed = desired;
        Some(change)
    }
}

/// 解析 mpv 的 `property-change` 事件；只支持数字属性。
fn parse_property_change(text: &str) -> Option<(&str, Option<f64>)> {
    if !text.contains(r#""event":"property-change""#) {
        return None;
    }
    let name_key = r#""name":""#;
    let name_start = text.find(name_key)? + name_key.len();
    let name_end = name_start + text[name_start..].find('"')?;
    let name = &text[name_start..name_end];

    let data_key = r#""data":"#;
    let data_start = text.find(data_key)? + data_key.len();
    let value = parse_json_number(&text[data_start..]);
    Some((name, value))
}

fn parse_json_number(text: &str) -> Option<f64> {
    let text = text.trim_start();
    if text.starts_with("null") {
        return None;
    }
    let end = text
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E')))
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    text[..end].parse::<f64>().ok()
}

/// 解析 mpv 的命令回复 `{"request_id":N,"error":"success"}`。
///
/// 回复里的 error 文本只是 mpv 的错误码（例如 `loading failed`），不含 URL。
fn parse_command_reply(text: &str) -> Option<(u64, Result<(), String>)> {
    let id_key = "\"request_id\":";
    let id_start = text.find(id_key)? + id_key.len();
    let id_end = id_start
        + text[id_start..]
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(text.len() - id_start);
    let request_id = text[id_start..id_end].parse::<u64>().ok()?;

    let error_key = "\"error\":\"";
    let error_start = text.find(error_key)? + error_key.len();
    let error_end = error_start + text[error_start..].find('"')?;
    let error = &text[error_start..error_end];
    if error == "success" {
        Some((request_id, Ok(())))
    } else {
        Some((request_id, Err(error.to_owned())))
    }
}

fn run(
    receiver: mpsc::Receiver<String>,
    speed_sender: Sender<String>,
    target_ms: u64,
    cache_ms: Arc<AtomicU64>,
    load: LoadSlot,
) {
    let Some(file) = connect_with_retry() else {
        log::error!(target: "mpv-ipc", "connect failed");
        return;
    };
    log::info!(target: "mpv-ipc", "speed chasing target_ms={target_ms}");
    let reader_file = match file.try_clone() {
        Ok(file) => Some(file),
        Err(error) => {
            log::warn!(target: "mpv-ipc", "clone failed: {error}");
            None
        }
    };
    if let Some(file) = reader_file {
        let thread_cache = cache_ms.clone();
        let thread_load = load.clone();
        std::thread::Builder::new()
            .name("np-video-mpv-ipc-read".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                let mut telemetry = Telemetry::default();
                let mut tuner = SpeedTuner::new(target_ms);
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    let text = line.trim();
                    if !text.is_empty() {
                        if text.contains(r#""event":"file-loaded""#) {
                            // 换流成功：旧文件的缓存 / 速度统计作废，新流重新上报。
                            telemetry.reset();
                            thread_cache.store(NO_CACHE, Ordering::Relaxed);
                            log::info!(target: "mpv-ipc", "event=file-loaded");
                        } else if let Some((name, value)) = parse_property_change(text) {
                            telemetry.update(name, value);
                            if let Some(cache) = telemetry.cache_ms() {
                                let clamped = cache.clamp(0.0, 60_000.0).round() as u64;
                                thread_cache.store(clamped, Ordering::Relaxed);
                            }
                            if telemetry.should_log() {
                                telemetry.log(target_ms);
                                if let Some(change) = tuner.update(telemetry.cache_ms()) {
                                    change.log(target_ms);
                                    if speed_sender.send(change.command()).is_err() {
                                        log::debug!(
                                            target: "mpv-ipc",
                                            "speed send failed: ipc thread stopped"
                                        );
                                    }
                                }
                            }
                        } else if let Some((request_id, result)) = parse_command_reply(text) {
                            if let Ok(mut slot) = thread_load.lock() {
                                *slot = Some((request_id, result));
                            }
                        } else {
                            log::debug!(target: "mpv-ipc", "reply received");
                        }
                    }
                    line.clear();
                }
            })
            .ok();
    }

    let mut writer = file;
    for (id, property) in [
        (1, "demuxer-cache-time"),
        (2, "playback-time"),
        (3, "demuxer-cache-duration"),
        (4, "speed"),
        (5, "cache-buffering-state"),
    ] {
        let command = format!(r#"{{"command":["observe_property",{id},"{property}"]}}"#);
        if writeln!(writer, "{command}")
            .and_then(|_| writer.flush())
            .is_err()
        {
            log::error!(target: "mpv-ipc", "observe failed property={property}");
            return;
        }
    }

    for command in receiver {
        if writeln!(writer, "{command}")
            .and_then(|_| writer.flush())
            .is_err()
        {
            log::error!(target: "mpv-ipc", "write failed");
            break;
        }
        // 不记录命令内容：loadfile 命令里带签名 URL。
        log::debug!(target: "mpv-ipc", "sent command bytes={}", command.len());
    }
}

fn connect_with_retry() -> Option<std::fs::File> {
    for attempt in 0..50 {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(crate::mpv::IPC_PIPE)
        {
            Ok(file) => {
                log::info!(target: "mpv-ipc", "connected attempt={attempt}");
                return Some(file);
            }
            Err(error) => {
                if attempt == 0 || attempt == 49 {
                    log::debug!(target: "mpv-ipc", "connect attempt={attempt} error={error}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numeric_property_change() {
        let line =
            r#"{"event":"property-change","id":1,"name":"demuxer-cache-time","data":12.345}"#;
        assert_eq!(
            parse_property_change(line),
            Some(("demuxer-cache-time", Some(12.345)))
        );
    }

    #[test]
    fn parses_null_and_ignores_other_events() {
        let line = r#"{"event":"property-change","id":2,"name":"playback-time","data":null}"#;
        assert_eq!(parse_property_change(line), Some(("playback-time", None)));
        assert_eq!(parse_property_change(r#"{"event":"file-loaded"}"#), None);
    }

    #[test]
    fn parses_integer_and_scientific_numbers() {
        assert_eq!(parse_json_number("100"), Some(100.0));
        assert_eq!(parse_json_number("1.5e2,"), Some(150.0));
        assert_eq!(parse_json_number("null"), None);
    }

    #[test]
    fn parses_loadfile_command_replies() {
        assert_eq!(
            parse_command_reply(r#"{"request_id":7,"error":"success"}"#),
            Some((7, Ok(())))
        );
        assert_eq!(
            parse_command_reply(r#"{"request_id":112,"error":"loading failed"}"#),
            Some((112, Err("loading failed".to_owned())))
        );
        assert_eq!(parse_command_reply(r#"{"event":"file-loaded"}"#), None);
        assert_eq!(parse_command_reply(r#"{"error":"success"}"#), None);
    }

    #[test]
    fn json_string_escapes_quotes_and_backslashes_for_loadfile() {
        assert_eq!(
            json_string("https://e.invalid/live.flv?token=a&b=1"),
            r#""https://e.invalid/live.flv?token=a&b=1""#
        );
        assert_eq!(
            json_string("https://e.invalid/a\"b\\c"),
            r#""https://e.invalid/a\"b\\c""#
        );
        assert_eq!(json_string("bad\nurl"), r#""bad\nurl""#);
        let command = format!(
            r#"{{"command":["loadfile",{},"replace"],"request_id":9}}"#,
            json_string("https://e.invalid/x?a=\"b\"")
        );
        assert!(command.contains("request_id"));
        assert!(command.contains(r#"\""#));
        assert!(!command.contains("\n"));
    }

    #[test]
    fn telemetry_reset_drops_old_file_numbers() {
        let mut telemetry = Telemetry::default();
        telemetry.update("demuxer-cache-time", Some(12.0));
        telemetry.update("playback-time", Some(10.0));
        assert!(telemetry.cache_ms().is_some());
        telemetry.reset();
        assert_eq!(telemetry.cache_ms(), None);
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn desired_speed_covers_the_thresholds() {
        // 目标 900ms：严格大于 / 严格小于，边界本身留在中间档。
        assert_eq!(desired_speed(900.0, 900), 1.0);
        assert_eq!(desired_speed(901.0, 900), 1.10);
        assert_eq!(desired_speed(1_900.0, 900), 1.10);
        assert_eq!(desired_speed(1_901.0, 900), 1.20);
        assert_eq!(desired_speed(2_900.0, 900), 1.20);
        assert_eq!(desired_speed(2_901.0, 900), 1.30);
        assert_eq!(desired_speed(450.0, 900), 1.0);
        assert_eq!(desired_speed(449.0, 900), 0.60);
        // 其它目标：各档跟着 target 平移。
        assert_eq!(desired_speed(499.0, 1_000), 0.60);
        assert_eq!(desired_speed(500.0, 1_000), 1.0);
        assert_eq!(desired_speed(1_001.0, 1_000), 1.10);
        assert_eq!(desired_speed(999.0, 2_000), 0.60);
        assert_eq!(desired_speed(1_000.0, 2_000), 1.0);
    }

    #[test]
    fn speed_tuner_only_emits_when_the_speed_changes() {
        let mut tuner = SpeedTuner::new(900);
        assert_eq!(
            tuner.update(None),
            None,
            "unknown cache must not touch speed"
        );
        let change = tuner.update(Some(1_500.0)).expect("speed up");
        assert_eq!((change.from, change.to), (1.0, 1.10));
        assert_eq!(
            change.command(),
            r#"{"command":["set_property","speed",1.10]}"#
        );
        assert_eq!(
            tuner.update(Some(1_600.0)),
            None,
            "same step must not resend"
        );
        let change = tuner.update(Some(900.0)).expect("back to normal");
        assert_eq!((change.from, change.to), (1.10, 1.0));
        let change = tuner.update(Some(300.0)).expect("slow down");
        assert_eq!((change.from, change.to), (1.0, 0.60));
    }

    #[test]
    fn telemetry_cache_ms_prefers_the_playback_gap() {
        let mut telemetry = Telemetry::default();
        assert_eq!(telemetry.cache_ms(), None);
        telemetry.update("demuxer-cache-duration", Some(1.2));
        assert_close(telemetry.cache_ms().expect("duration fallback"), 1_200.0);
        telemetry.update("demuxer-cache-time", Some(10.0));
        telemetry.update("playback-time", Some(8.6));
        assert_close(telemetry.cache_ms().expect("gap"), 1_400.0);
        telemetry.update("playback-time", Some(12.0));
        assert_eq!(
            telemetry.cache_ms(),
            Some(0.0),
            "negative gap must clamp to zero"
        );
    }
}
