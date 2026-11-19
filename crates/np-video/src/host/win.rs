//! Win32 小工具：宽字符串、RECT 转换、客户区 / 边框 / 显示器信息。

use std::io::Write;

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetClientRect, GetSystemMetrics, SM_CXPADDEDBORDER,
    SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
};
use windows::core::{BOOL, PCWSTR};

use crate::layout::{FrameMetrics, Rect};
use crate::protocol::Event;

/// mpv 通过 `--wid` 在父窗口下创建的子窗口类名（mpv `w32_common.c`）。
pub(super) const MPV_WINDOW_CLASS: &str = "mpv";

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

/// 客户区边缘的 resize 命中厚度；X/Y 取较大值，至少 1px。
pub(super) fn resize_border(dpi: u32) -> i32 {
    let frame = frame_metrics(dpi);
    frame.x.max(frame.y).max(1)
}

/// 在 `parent` 的子窗口里按类名查找；找到第一个匹配项就停止。
pub(super) fn find_child_by_class(parent: HWND, class: &str) -> Option<HWND> {
    let mut state = FindChildState { class, found: None };
    // SAFETY: parent 有效；state 在本次同步枚举期间一直有效。
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(find_child_proc),
            LPARAM((&mut state as *mut FindChildState<'_>) as isize),
        );
    }
    state.found
}

struct FindChildState<'a> {
    class: &'a str,
    found: Option<HWND>,
}

unsafe extern "system" fn find_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 来自 find_child_by_class 的同步调用，指向有效的 state。
    let state = unsafe { &mut *(lparam.0 as *mut FindChildState<'_>) };
    if state.found.is_some() {
        return BOOL(0);
    }
    let mut buffer = [0u16; 64];
    // SAFETY: buffer 是有效可写切片；hwnd 来自枚举。
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len > 0 && class_name_eq(&buffer[..len as usize], state.class) {
        state.found = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

fn class_name_eq(utf16: &[u16], expected: &str) -> bool {
    String::from_utf16_lossy(utf16).eq_ignore_ascii_case(expected)
}

/// 把窗口（surface 下的 mpv 子窗口）缩放到给定客户区尺寸。
///
/// 只改位置/尺寸，不碰 Z 序；mpv 在跨进程窗口上使用 `SWP_ASYNCWINDOWPOS`，
/// 避免宿主线程被 mpv 的窗口线程拖住。
pub(super) fn resize_child(hwnd: HWND, width: i32, height: i32) {
    // SAFETY: hwnd 来自 EnumChildWindows 或本进程创建；参数均为纯数据。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width.max(0),
            height.max(0),
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}

/// `WM_SIZE` 后把 surface 下的 mpv 子窗口缩放到客户区宽高。
///
/// mpv 自己按 `--keepaspect=yes` 在窗口内 letterbox；这里只负责让它
/// 和 surface 一样大，避免窗口缩放后四周留下黑边。
/// mpv 还没创建子窗口时返回 `false`，等下一次 `WM_SIZE` 再同步。
pub(super) fn resize_mpv_child(surface: HWND, width: i32, height: i32) -> bool {
    let Some(child) = find_child_by_class(surface, MPV_WINDOW_CLASS) else {
        return false;
    };
    resize_child(child, width, height);
    true
}

/// 当前显示器工作区；失败返回 `None`，调用方保留系统给的最大化窗口矩形。
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_VISIBLE,
    };

    /// 用系统 STATIC 类建一个隐藏窗口，不启动 mpv / 视频。
    fn create_window(class: &str, style: WINDOW_STYLE, parent: Option<HWND>) -> HWND {
        let class = wide(class);
        // SAFETY: 系统类已注册；父窗口在测试内保持存活。
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                pcwstr(&class),
                PCWSTR::null(),
                style,
                0,
                0,
                400,
                300,
                parent,
                None,
                None,
                None,
            )
        }
        .expect("测试窗口创建失败")
    }

    #[test]
    fn mpv_child_lookup_and_resize_helper_do_not_need_a_real_video() {
        // 类名与 mpv `w32_common.c` 的 `MPV_WINDOW_CLASS_NAME` 保持一致。
        assert_eq!(MPV_WINDOW_CLASS, "mpv");

        let parent = create_window("STATIC", WINDOW_STYLE(0), None);
        let child = create_window("STATIC", WS_CHILD | WS_VISIBLE, Some(parent));

        // 按类名查找真实子 HWND；mpv 的类名还不在时会返回 None。
        assert_eq!(find_child_by_class(parent, "STATIC"), Some(child));
        assert_eq!(find_child_by_class(parent, MPV_WINDOW_CLASS), None);
        assert!(!resize_mpv_child(parent, 320, 200));

        // 子窗口 resize 到客户区宽高（跨进程时 mpv 也一样处理）。
        resize_child(child, 320, 200);
        assert_eq!(client_size(child), (320, 200));

        // SAFETY: 窗口由本测试创建，销毁后不再使用。
        unsafe {
            let _ = DestroyWindow(child);
            let _ = DestroyWindow(parent);
        }
    }
}
