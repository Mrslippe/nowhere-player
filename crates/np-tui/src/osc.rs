//! OSC 11 终端背景探测/设置。WT 不支持时保持为 no-op，不能把用户终端改坏。

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatui::style::Color;

static ENABLED: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(kind: u32) -> *mut std::ffi::c_void;
    fn FlushConsoleInputBuffer(handle: *mut std::ffi::c_void) -> i32;
}

/// 清掉 OSC 查询可能残留的输入字节；启动时调用一次。
pub fn flush_input() {
    #[cfg(windows)]
    unsafe {
        let handle = GetStdHandle(0xFFFF_FFF6); // STD_INPUT_HANDLE
        FlushConsoleInputBuffer(handle);
    }
}

/// 启动时探测 OSC 11 查询是否有响应。只发送查询，不修改背景。
pub fn probe() -> bool {
    let mut out = io::stdout();
    if write!(out, "\x1b]11;?\x1b\\").is_err() || out.flush().is_err() {
        return false;
    }
    let supported = crossterm::event::poll(Duration::from_millis(120)).unwrap_or(false);
    if supported {
        let _ = crossterm::event::read();
        // 终端可能把 OSC 响应拆成多个事件；丢弃残余，避免启动后被当成用户输入。
        while crossterm::event::poll(Duration::from_millis(10)).unwrap_or(false) {
            let _ = crossterm::event::read();
        }
        ENABLED.store(true, Ordering::SeqCst);
    }
    supported
}

pub fn set_background(color: Option<Color>) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    match color {
        Some(Color::Rgb(r, g, b)) => {
            let _ = write!(out, "\x1b]11;#{r:02x}{g:02x}{b:02x}\x1b\\");
        }
        _ => {
            // 回到跟随终端：恢复终端原背景，但保持探测状态供后续主题再切换。
            let _ = write!(out, "\x1b]111\x1b\\");
        }
    }
    let _ = out.flush();
}

pub struct BackgroundGuard;

impl Drop for BackgroundGuard {
    fn drop(&mut self) {
        restore();
    }
}

pub fn guard() -> BackgroundGuard {
    BackgroundGuard
}

pub fn restore() {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]111\x1b\\");
    let _ = out.flush();
    ENABLED.store(false, Ordering::SeqCst);
}
