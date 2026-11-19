//! Win32 小工具：宽字符串、RECT 转换、客户区 / 边框 / 显示器信息。

use std::io::Write;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME,
};
use windows::core::PCWSTR;

use crate::layout::{FrameMetrics, Rect};
use crate::protocol::Event;

pub(super) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) fn pcwstr(text: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(text.as_ptr())
}

pub(super) fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    // SAFETY: `hwnd` 来自本进程创建的消息循环；`rect` 是有效的可写局部变量。
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    (
        (rect.right - rect.left).max(0),
        (rect.bottom - rect.top).max(0),
    )
}

/// 可缩放窗口的边框厚度（`SM_CXSIZEFRAME + SM_CXPADDEDBORDER`）。
pub(super) fn frame_metrics(dpi: u32) -> FrameMetrics {
    // SAFETY: 纯查询，无指针/生命周期参数。
    let frame_x = unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) }
        + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    let frame_y = unsafe { GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) }
        + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    FrameMetrics {
        x: frame_x.max(0),
        y: frame_y.max(0),
    }
}

/// 当前显示器工作区；失败返回 `None`，调用方回退到按边框内缩。
pub(super) fn monitor_work_area(hwnd: HWND) -> Option<Rect> {
    // SAFETY: `hwnd` 有效；`MONITORINFO` 由系统按 cbSize 填充。
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        Some(from_rect(info.rcWork))
    }
}

pub(super) fn from_rect(rect: RECT) -> Rect {
    Rect::new(rect.left, rect.top, rect.right, rect.bottom)
}

pub(super) fn to_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// 屏幕坐标 / 命中 lParam 的低 16 位（有符号）。
pub(super) fn loword(value: isize) -> i32 {
    (value as u32 & 0xFFFF) as u16 as i16 as i32
}

/// 屏幕坐标 / 命中 lParam 的高 16 位（有符号）。
pub(super) fn hiword(value: isize) -> i32 {
    ((value as u32 >> 16) & 0xFFFF) as u16 as i16 as i32
}

/// 屏幕尺寸（主显示器，物理像素）。
pub(super) fn screen_size() -> (i32, i32) {
    // SAFETY: 纯查询。
    unsafe {
        let width = GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN);
        let height = GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN);
        (width.max(1), height.max(1))
    }
}

/// 只写协议行到 stdout；任何错误都不影响窗口生命周期。
pub(super) fn emit(event: Event) {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let _ = writeln!(stdout, "{}", event.line());
    let _ = stdout.flush();
}
