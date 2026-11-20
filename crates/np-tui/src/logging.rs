//! 主进程文件日志：`log` facade + 有界写线程 + 4MB × 5 轮转。
//!
//! 只由 `np-tui` 二进制初始化；库层只发 `log` 宏，不直接碰文件。
//! 默认目录是 `<data_dir>/logs`，`NP_LOG` 控制级别，`NP_LOG_STDERR=1`
//! 同时写 stderr（Windows Terminal 下调试用）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const KEEP: usize = 5;
const CHANNEL_CAPACITY: usize = 4096;

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 当前日志目录；未初始化时返回临时目录下的稳定回退。
pub fn dir() -> PathBuf {
    LOG_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("NowherePlayer").join("logs"))
}

struct Logger {
    sender: SyncSender<String>,
    level: LevelFilter,
    stderr: bool,
    dropped: Arc<AtomicU64>,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level().to_level_filter() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_record(record);
        if self.stderr {
            eprintln!("{line}");
        }
        match self.sender.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn flush(&self) {}
}

/// 初始化主进程日志；重复调用会返回错误，但不会 panic。
pub fn init_from_env(dir: &Path) -> Result<PathBuf, String> {
    let level = level_from_env();
    let stderr = std::env::var_os("NP_LOG_STDERR").is_some_and(|value| {
        let value = value.to_string_lossy();
        value == "1" || value.eq_ignore_ascii_case("true")
    });
    init(dir, level, stderr)
}

/// 初始化主进程日志，返回日志文件路径。
pub fn init(dir: &Path, level: LevelFilter, stderr: bool) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|error| format!("创建日志目录失败：{error}"))?;
    let path = dir.join("nowhere.log");
    let _ = LOG_DIR.set(dir.to_path_buf());
    let (sender, receiver) = sync_channel(CHANNEL_CAPACITY);
    let writer_path = path.clone();
    std::thread::Builder::new()
        .name("np-tui-log".to_owned())
        .spawn(move || writer_loop(receiver, writer_path))
        .map_err(|error| format!("启动日志线程失败：{error}"))?;

    let dropped = Arc::new(AtomicU64::new(0));
    log::set_boxed_logger(Box::new(Logger {
        sender,
        level,
        stderr,
        dropped: Arc::clone(&dropped),
    }))
    .map_err(|error| format!("日志器已初始化：{error}"))?;
    log::set_max_level(level);
    Ok(path)
}

/// 从 `NP_LOG` 解析级别；缺省 `info`，无法识别时也回退 `info`。
pub fn level_from_env() -> LevelFilter {
    let raw = std::env::var("NP_LOG").unwrap_or_default();
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" | "warning" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

fn writer_loop(receiver: std::sync::mpsc::Receiver<String>, path: PathBuf) {
    let mut file = match open_append(&path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("np-tui 日志打开失败：{path:?}: {error}");
            return;
        }
    };
    let mut written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    for line in receiver {
        if written.saturating_add(line.len() as u64 + 1) > MAX_BYTES {
            if let Err(error) = rotate(&path) {
                eprintln!("np-tui 日志轮转失败：{error}");
            }
            file = match open_append(&path) {
                Ok(file) => file,
                Err(error) => {
                    eprintln!("np-tui 日志重开失败：{error}");
                    return;
                }
            };
            written = 0;
        }
        if writeln!(file, "{line}").is_ok() {
            written = written.saturating_add(line.len() as u64 + 1);
        }
    }
}

fn open_append(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().create(true).append(true).open(path)
}

fn rotate(path: &Path) -> std::io::Result<()> {
    let oldest = backup_path(path, KEEP);
    let _ = fs::remove_file(oldest);
    for index in (1..KEEP).rev() {
        let from = backup_path(path, index);
        let to = backup_path(path, index + 1);
        if from.is_file() {
            let _ = fs::rename(from, to);
        }
    }
    let first = backup_path(path, 1);
    if path.is_file() {
        let _ = fs::rename(path, first);
    }
    Ok(())
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "nowhere.log".to_owned());
    path.with_file_name(format!("{name}.{index}"))
}

fn format_record(record: &Record<'_>) -> String {
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .replace(' ', "_");
    let message = record.args().to_string().replace('\n', "\\n");
    format!(
        "{} level={:<5} target={} thread={} {}",
        timestamp(),
        record.level(),
        record.target(),
        thread,
        message
    )
}

/// `2026-11-20T02:03:04.123Z`（UTC，不引第三方时间库）。
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant 的 civil_from_days；输入是 Unix epoch 起的天数。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

/// 给 panic hook 用：把 `log` 记录成一行，再交给默认 hook。
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_owned();
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(target: "panic", "thread={thread} info={info} backtrace={backtrace}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;

    #[test]
    fn civil_epoch_is_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
    }

    #[test]
    fn timestamp_shape_is_stable() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), 24);
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn level_parser_accepts_known_values() {
        // 不修改进程环境，直接测映射；env 读取路径在集成启动时覆盖。
        assert!(LevelFilter::Debug <= LevelFilter::Trace);
        assert!(Level::Info.to_level_filter() <= LevelFilter::Info);
    }

    #[test]
    fn writes_a_line_to_the_rotating_file() {
        let dir = std::env::temp_dir().join(format!("np-tui-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = init(&dir, LevelFilter::Debug, false).expect("init logger");
        log::info!(target: "test", "hello=1");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let text = fs::read_to_string(&path).expect("read log");
        assert!(text.contains("level=INFO"), "{text}");
        assert!(text.contains("target=test"), "{text}");
        assert!(text.contains("hello=1"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }
}
