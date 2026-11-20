//! `np-video` 会话日志。进程短命，单个文件追加/覆盖即可，不做轮转。

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

struct Logger {
    file: Mutex<File>,
    level: LevelFilter,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level().to_level_filter() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .replace(' ', "_");
        let message = record.args().to_string().replace('\n', "\\n");
        let line = format!(
            "{} level={:<5} target={} thread={} {}\n",
            timestamp(),
            record.level(),
            record.target(),
            thread,
            message
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    fn flush(&self) {}
}

/// 日志目录：`NP_LOG_DIR` > `%LOCALAPPDATA%\NowherePlayer\logs` > 临时目录。
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NP_LOG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(base).join("NowherePlayer").join("logs");
    }
    std::env::temp_dir().join("NowherePlayer").join("logs")
}

pub fn init_from_env(dir: &Path) -> Result<PathBuf, String> {
    let level = level_from_env();
    fs::create_dir_all(dir).map_err(|error| format!("创建日志目录失败：{error}"))?;
    let path = dir.join("np-video.log");
    let file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|error| format!("打开 np-video 日志失败：{error}"))?;
    log::set_boxed_logger(Box::new(Logger {
        file: Mutex::new(file),
        level,
    }))
    .map_err(|error| format!("日志器已初始化：{error}"))?;
    log::set_max_level(level);
    Ok(path)
}

pub fn level_from_env() -> LevelFilter {
    match std::env::var("NP_LOG")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "off" => LevelFilter::Off,
        "error" => LevelFilter::Error,
        "warn" | "warning" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

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
