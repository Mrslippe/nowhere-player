//! Win32 宿主窗口：原生缩放边框 + 去掉 caption + 悬浮标题栏 + mpv 子窗口。
//!
//! 窗口结构（Z 序从下到上）：
//!
//! ```text
//! 顶层窗口（WS_OVERLAPPEDWINDOW，保留 THICKFRAME / MAXIMIZEBOX）
//!   └─ 视频子窗口（始终 = 客户区 Rect）
//!   └─ 标题栏 overlay（顶部 32px，只在显示时覆盖视频）
//! ```
//!
//! 显示 / 隐藏标题栏只动 overlay；视频子窗口的 Rect 只在客户区尺寸变化时更新。

mod paint;
mod win;

use std::cell::RefCell;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, HBRUSH, HFONT, InvalidateRect,
    MapWindowPoints, PAINTSTRUCT, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RDW_UPDATENOW,
    RedrawWindow, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GA_ROOT, GWLP_USERDATA, GetAncestor, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    HICON, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT,
    HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HTTRANSPARENT, HWND_TOP,
    IDC_ARROW, IsIconic, IsZoomed, LoadCursorW, MINMAXINFO, MSG, NCCALCSIZE_PARAMS, PostMessageW,
    PostQuitMessage, RegisterClassExW, SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
    SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY,
    WM_DPICHANGED, WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITMENULOOP,
    WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_PAINT, WM_SIZE, WM_TIMER, WNDCLASSEXW,
    WNDPROC, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    WindowFromPoint,
};
use windows::core::PCWSTR;

use crate::layout::{self, Hit, ResizeEdge, TitlebarState};
use crate::mpv;
use crate::protocol::{Event, Request};

const HOST_CLASS: &str = "NowherePlayerVideoHost";
const SURFACE_CLASS: &str = "NowherePlayerVideoSurface";
const OVERLAY_CLASS: &str = "NowherePlayerVideoTitleBar";
const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 50;

/// 顶层窗口：保留原生 THICKFRAME / MIN / MAX，并让背景不覆盖子窗口。
fn host_style() -> WINDOW_STYLE {
    WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN
}

/// 视频子窗口：**必须**带 WS_CLIPSIBLINGS，否则它重绘时会把上方的
/// titlebar overlay 盖掉（mpv 的子窗口也在它的客户区里）。
fn surface_style() -> WINDOW_STYLE {
    WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN
}

/// 标题栏 overlay：顶部 32px，初始不显示。
fn overlay_style() -> WINDOW_STYLE {
    WS_CHILD | WS_CLIPSIBLINGS
}

/// TUI 通过 stdin 发来的控制消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Quit,
}

/// 窗口结束的原因；`run` 返回给 main 决定最后一行协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    UserClosed,
    TuiQuit,
    MpvExited(Option<i32>),
    LoopError,
}

struct Host {
    hwnd: HWND,
    surface: HWND,
    overlay: HWND,
    title: String,
    dpi: u32,
    client_w: i32,
    client_h: i32,
    titlebar: TitlebarState,
    is_maximized: bool,
    mpv: Option<Child>,
    control: Receiver<Control>,
    closing: Option<CloseReason>,
    font: HFONT,
}

#[derive(Default)]
struct TimerAction {
    close: Option<CloseReason>,
    show: bool,
    hide: bool,
    repaint: bool,
}

/// 创建窗口、启动 mpv、跑消息循环。成功返回时窗口已销毁，mpv 已回收。
pub fn run(request: Request, control: Receiver<Control>) -> Result<CloseReason, String> {
    // 先解析 mpv，失败时不要先弹一个空窗口。
    let mpv_program = mpv::resolve_mpv(
        std::env::var_os(mpv::PLAYER_ENV).as_deref(),
        std::env::var_os("PATH").as_deref(),
    )?;

    // SAFETY: 窗口创建前调用；Per-Monitor V2 是进程级设置。
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // SAFETY: 纯查询。
    let system_dpi = unsafe { GetDpiForSystem() }.max(96);
    let (screen_w, screen_h) = win::screen_size();
    let window_rect = layout::initial_window_rect(screen_w, screen_h, system_dpi);

    // SAFETY: 取本模块句柄用于注册窗口类。
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("获取模块句柄失败：{}", error.message()))?
        .into();

    let host_class = win::wide(HOST_CLASS);
    let surface_class = win::wide(SURFACE_CLASS);
    let overlay_class = win::wide(OVERLAY_CLASS);
    // SAFETY: 画刷只作为类背景，进程退出时随进程回收。
    let background = unsafe { CreateSolidBrush(COLORREF(0)) };

    register_class(&host_class, instance, Some(host_proc), background)?;
    register_class(&surface_class, instance, Some(surface_proc), background)?;
    register_class(&overlay_class, instance, Some(overlay_proc), background)?;

    // 单窗口、单实例：同进程类名下已有宿主就拒绝。
    if unsafe { FindWindowW(win::pcwstr(&host_class), PCWSTR::null()) }.is_ok() {
        return Err("视频窗口已在运行".to_owned());
    }

    let title = layout::window_title(Some(request.display_title()));
    let title_wide = win::wide(&title);
    // SAFETY: 参数均为有效字符串/句柄；窗口类已注册。
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            win::pcwstr(&host_class),
            win::pcwstr(&title_wide),
            host_style(),
            window_rect.left,
            window_rect.top,
            window_rect.width(),
            window_rect.height(),
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("视频窗口创建失败：{}", error.message()))?;

    let (client_w, client_h) = win::client_size(hwnd);
    // SAFETY: 父窗口有效；子窗口类已注册。
    let surface = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            win::pcwstr(&surface_class),
            PCWSTR::null(),
            surface_style(),
            0,
            0,
            client_w,
            client_h,
            Some(hwnd),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("视频子窗口创建失败：{}", error.message()))?;

    let bar_height = layout::titlebar_height(system_dpi);
    // 初始隐藏：由 TitlebarState + 定时器在鼠标进入顶部 32px 时显示。
    // SAFETY: 同上。
    let overlay = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            win::pcwstr(&overlay_class),
            PCWSTR::null(),
            overlay_style(),
            0,
            0,
            client_w,
            bar_height,
            Some(hwnd),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("标题栏创建失败：{}", error.message()))?;

    // 保留 DWM 阴影 / 边框。原生 THICKFRAME 仍在，1px 扩展只托住阴影。
    let margins = MARGINS {
        cxLeftWidth: 1,
        cxRightWidth: 1,
        cyTopHeight: 1,
        cyBottomHeight: 1,
    };
    // SAFETY: hwnd 有效；margins 在本调用期间有效。
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }

    // SAFETY: 纯查询；窗口刚创建。0 视为查询失败，回退系统 DPI。
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => system_dpi,
        dpi => dpi.max(96),
    };
    let host = Host {
        hwnd,
        surface,
        overlay,
        title,
        dpi,
        client_w,
        client_h,
        titlebar: TitlebarState::new(),
        is_maximized: false,
        mpv: None,
        control,
        closing: None,
        font: HFONT(std::ptr::null_mut()),
    };
    let cell = RefCell::new(host);
    let cell_ptr = (&cell as *const RefCell<Host>) as isize;
    // SAFETY: 窗口一直活到 `run` 末尾；cell 在消息循环期间不会被移动。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cell_ptr);
        SetWindowLongPtrW(surface, GWLP_USERDATA, cell_ptr);
        SetWindowLongPtrW(overlay, GWLP_USERDATA, cell_ptr);
    }

    {
        let mut host = cell.borrow_mut();
        host.font = paint::create_font(host.dpi);
        host.resize_client(client_w, client_h);
        // SAFETY: 窗口标题在本调用期间有效。
        unsafe {
            let wide = win::wide(&host.title);
            let _ = SetWindowTextW(hwnd, win::pcwstr(&wide));
        }
    }

    // 让 WM_NCCALCSIZE 在 user data 就绪后按真实 DPI 重算一次客户区。
    // SAFETY: 句柄有效；只触发非客户区重算。
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    // 客户区可能因真实 DPI 的边框厚度变化；再同步一次子窗口，保证视频铺满客户区。
    let (client_w, client_h) = win::client_size(hwnd);
    cell.borrow_mut().resize_client(client_w, client_h);

    // SAFETY: 窗口有效；ShowWindow / SetForegroundWindow 不持有任何 Rust 借用。
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = UpdateWindow(hwnd);
    }

    // mpv 嵌入视频子窗口；URL 只写 stdin。
    match mpv::spawn_mpv(&mpv_program, surface.0 as usize, &request) {
        Ok(child) => cell.borrow_mut().mpv = Some(child),
        Err(error) => {
            // SAFETY: 窗口有效。
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(error);
        }
    }

    // SAFETY: 窗口有效；定时器回调走 WM_TIMER。
    unsafe {
        SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None);
    }

    win::emit(Event::Ready);

    let mut message = MSG::default();
    let mut loop_error = false;
    loop {
        // SAFETY: message 是有效的可写局部变量。
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 == 0 {
            break;
        }
        if result.0 == -1 {
            loop_error = true;
            break;
        }
        // SAFETY: message 由 GetMessageW 填充。
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    let mut host = cell.into_inner();
    host.shutdown();
    if loop_error {
        return Ok(CloseReason::LoopError);
    }
    Ok(host.closing.unwrap_or(CloseReason::LoopError))
}

fn register_class(
    name: &[u16],
    instance: HINSTANCE,
    proc: WNDPROC,
    background: HBRUSH,
) -> Result<(), String> {
    // SAFETY: LoadCursorW 使用系统资源；返回的 HCURSOR 由系统持有。
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: HICON(std::ptr::null_mut()),
        hCursor: cursor,
        hbrBackground: background,
        lpszMenuName: PCWSTR::null(),
        lpszClassName: win::pcwstr(name),
        hIconSm: HICON(std::ptr::null_mut()),
    };
    // SAFETY: class 在调用期间有效；class 名由 atom 复制。
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err("视频窗口类注册失败".to_owned());
    }
    Ok(())
}

impl Host {
    /// 客户区变化：视频子窗口跟随客户区，overlay 跟随顶部 32px。
    ///
    /// **只有这里会 SetWindowPos 视频子窗口。** 标题栏的显示 / 隐藏走
    /// [`Self::show_titlebar`] / [`Self::hide_titlebar`]，绝不 resize 视频。
    fn resize_client(&mut self, width: i32, height: i32) {
        self.client_w = width.max(0);
        self.client_h = height.max(0);
        let video = layout::video_rect(self.client_w, self.client_h);
        let overlay = layout::overlay_rect(self.client_w, self.dpi);
        // SAFETY: 子窗口句柄有效。
        unsafe {
            let _ = SetWindowPos(
                self.surface,
                None,
                video.left,
                video.top,
                video.width(),
                video.height(),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = SetWindowPos(
                self.overlay,
                Some(HWND_TOP),
                overlay.left,
                overlay.top,
                overlay.width(),
                overlay.height(),
                SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.overlay), None, false);
        }
        // surface 和 mpv 的 `--wid` 子窗口是父子关系：surface resize 后必须
        // 把 mpv 子窗口也拉到客户区大小，否则视频按旧尺寸居中、四周出现黑边。
        // mpv 还没创建子窗口时返回 false，下一次 WM_SIZE 会补上。
        win::resize_mpv_child(self.surface, self.client_w, self.client_h);
    }

    fn show_titlebar(&self) {
        // 硬约束 2：只 ShowWindow/SWP overlay，视频子窗口 Rect 完全不动。
        let overlay = layout::overlay_rect(self.client_w, self.dpi);
        // SAFETY: overlay 有效。
        unsafe {
            let _ = ShowWindow(self.overlay, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                self.overlay,
                Some(HWND_TOP),
                overlay.left,
                overlay.top,
                overlay.width(),
                overlay.height(),
                SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.overlay), None, false);
        }
    }

    fn hide_titlebar(&self) {
        // SAFETY: overlay 有效；只隐藏 overlay。
        unsafe {
            let _ = ShowWindow(self.overlay, SW_HIDE);
            // 子窗口隐藏不会自动重绘被它盖住的兄弟区域；强制视频子窗口刷新，
            // 避免静态画面（或暂停视频）上残留 32px 标题栏。
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_ERASE,
            );
        }
    }

    fn set_dpi(&mut self, dpi: u32) {
        let dpi = dpi.max(96);
        if dpi == self.dpi {
            return;
        }
        self.dpi = dpi;
        let old = self.font;
        self.font = paint::create_font(dpi);
        if !old.0.is_null() {
            // SAFETY: 旧字体由本窗口拥有，替换后删除。
            unsafe {
                let _ = DeleteObject(old.into());
            }
        }
        let overlay = layout::overlay_rect(self.client_w, dpi);
        // SAFETY: 子窗口有效。
        unsafe {
            let _ = SetWindowPos(
                self.overlay,
                Some(HWND_TOP),
                overlay.left,
                overlay.top,
                overlay.width(),
                overlay.height(),
                SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.overlay), None, false);
        }
    }

    fn tick(&mut self) -> TimerAction {
        let mut action = TimerAction::default();
        if self.closing.is_some() {
            return action;
        }
        match self.control.try_recv() {
            Ok(Control::Quit) => {
                action.close = Some(CloseReason::TuiQuit);
                return action;
            }
            Err(_) => {}
        }

        if let Some(child) = self.mpv.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.mpv = None;
                    action.close = Some(CloseReason::MpvExited(status.code()));
                    return action;
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }

        let cursor = self.cursor_client();
        let update = self
            .titlebar
            .update(cursor, Instant::now(), self.client_w, self.dpi);
        if update.visibility_changed {
            if self.titlebar.is_visible() {
                action.show = true;
            } else {
                action.hide = true;
            }
        }
        action.repaint = update.hover_changed || update.visibility_changed;
        action
    }

    /// 鼠标在客户区内的坐标；鼠标在窗口外 / 最小化 / 被其他窗口盖住时 `None`。
    fn cursor_client(&self) -> Option<(i32, i32)> {
        // SAFETY: 纯查询。
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                return None;
            }
            let mut point = POINT::default();
            if GetCursorPos(&mut point).is_err() {
                return None;
            }
            let root = GetAncestor(WindowFromPoint(point), GA_ROOT);
            if root != self.hwnd {
                return None;
            }
            let mut client = point;
            MapWindowPoints(None, Some(self.hwnd), std::slice::from_mut(&mut client));
            if client.x < 0
                || client.y < 0
                || client.x >= self.client_w
                || client.y >= self.client_h
            {
                return None;
            }
            Some((client.x, client.y))
        }
    }

    fn shutdown(&mut self) {
        if let Some(mut child) = self.mpv.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if !self.font.0.is_null() {
            // SAFETY: 字体由本窗口创建，退出时释放。
            unsafe {
                let _ = DeleteObject(self.font.into());
            }
            self.font = HFONT(std::ptr::null_mut());
        }
    }

    fn set_closing_once(&mut self, reason: CloseReason) {
        if self.closing.is_none() {
            self.closing = Some(reason);
        }
    }
}

/// 从窗口的 GWLP_USERDATA 取回 `RefCell<Host>`；窗口创建早期为 null。
///
/// SAFETY: 指针由 `run` 在消息循环期间持有，窗口销毁前一直有效。
unsafe fn host_cell(hwnd: HWND) -> Option<&'static RefCell<Host>> {
    // SAFETY: 调用方保证窗口仍属于本次消息循环。
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Host>;
    if pointer.is_null() {
        None
    } else {
        // SAFETY: 指针来自 `run` 里的 RefCell，生命周期覆盖消息循环。
        Some(unsafe { &*pointer })
    }
}

fn with_host_mut<R>(hwnd: HWND, f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    // SAFETY: 见 host_cell。
    let cell = unsafe { host_cell(hwnd)? };
    let mut host = cell.try_borrow_mut().ok()?;
    Some(f(&mut host))
}

fn with_host_ref<R>(hwnd: HWND, f: impl FnOnce(&Host) -> R) -> Option<R> {
    // SAFETY: 见 host_cell。
    let cell = unsafe { host_cell(hwnd)? };
    let host = cell.try_borrow().ok()?;
    Some(f(&host))
}

/// 屏幕坐标是否落在宿主客户区的 resize 边缘内。
///
/// 宿主、surface、overlay 的客户区原点都在宿主客户区 (0,0)，所以同一个
/// 判断可以给三个 window proc 共用。最大化 / 窗口数据尚未就绪时返回 `None`。
fn resize_hit(hwnd: HWND, lparam: LPARAM) -> Option<ResizeEdge> {
    let (host, dpi, width, height) = with_host_ref(hwnd, |host| {
        (host.hwnd, host.dpi, host.client_w, host.client_h)
    })?;
    let mut point = POINT {
        x: win::loword(lparam.0),
        y: win::hiword(lparam.0),
    };
    // SAFETY: 屏幕坐标转窗口客户区坐标。
    unsafe {
        MapWindowPoints(None, Some(hwnd), std::slice::from_mut(&mut point));
    }
    // SAFETY: 纯查询。
    if unsafe { IsZoomed(host) }.as_bool() {
        return None;
    }
    layout::resize_edge(point.x, point.y, width, height, win::resize_border(dpi))
}

/// 把纯几何的 resize 方向映射成 Win32 命中码。
fn edge_code(edge: ResizeEdge) -> isize {
    let code = match edge {
        ResizeEdge::Left => HTLEFT,
        ResizeEdge::Right => HTRIGHT,
        ResizeEdge::Top => HTTOP,
        ResizeEdge::Bottom => HTBOTTOM,
        ResizeEdge::TopLeft => HTTOPLEFT,
        ResizeEdge::TopRight => HTTOPRIGHT,
        ResizeEdge::BottomLeft => HTBOTTOMLEFT,
        ResizeEdge::BottomRight => HTBOTTOMRIGHT,
    };
    code as isize
}

unsafe extern "system" fn surface_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            // 客户区铺满整个窗口后，surface 会盖住边缘。它不是 disabled
            // 窗口，会吃掉 WM_NCHITTEST，所以边缘必须返回 HTTRANSPARENT，
            // 让同线程的宿主拿到并返回 HTLEFT/HTTOP 等（mpv 的 --wid 子窗口
            // 是 disabled 的，本身不参与 hit test）。
            if resize_hit(hwnd, lparam).is_some() {
                return LRESULT(HTTRANSPARENT as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        // SAFETY: 所有参数来自系统；DefWindowProcW 是默认处理。
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCALCSIZE => {
            if wparam.0 == 0 {
                // 客户端区域查询：交给默认处理。
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            // SAFETY: wParam=TRUE 时 lParam 指向 NCCALCSIZE_PARAMS。
            let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
            // SAFETY: 纯查询，无指针参数。
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                if let Some(work) = win::monitor_work_area(hwnd) {
                    params.rgrc[0] = win::to_rect(layout::maximized_client_rect(work));
                }
            } else {
                // 正常窗口：客户区 = 整个窗口矩形。原生 caption / 8px
                // 边框不再占位，顶部白线因此消失；边缘 resize 由
                // WM_NCHITTEST 恢复。
                params.rgrc[0] =
                    win::to_rect(layout::normal_client_rect(win::from_rect(params.rgrc[0])));
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            // 客户区 = 整个窗口后 DefWindowProc 会把边缘也判成 HTCLIENT，
            // 这里显式恢复原生缩放 / 吸附的边缘命中。
            match resize_hit(hwnd, lparam) {
                Some(edge) => LRESULT(edge_code(edge)),
                None => LRESULT(HTCLIENT as isize),
            }
        }
        WM_GETMINMAXINFO => {
            let dpi = with_host_ref(hwnd, |host| host.dpi).unwrap_or(96);
            let (width, height) = layout::min_track_size(dpi);
            // SAFETY: WM_GETMINMAXINFO 的 lParam 指向 MINMAXINFO。
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT {
                x: width,
                y: height,
            };
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            with_host_mut(hwnd, |host| host.set_dpi(dpi));
            // SAFETY: WM_DPICHANGED 的 lParam 指向建议 RECT。
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: 句柄有效；使用系统建议的尺寸。
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_SIZE => {
            if wparam.0 == 0 {
                // SIZE_MINIMIZED
                return LRESULT(0);
            }
            let (width, height) = win::client_size(hwnd);
            with_host_mut(hwnd, |host| {
                // SAFETY: 纯查询。
                host.is_maximized = unsafe { IsZoomed(hwnd) }.as_bool();
                host.resize_client(width, height);
            });
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 != TIMER_ID {
                return LRESULT(0);
            }
            let action = with_host_mut(hwnd, |host| host.tick());
            let Some(action) = action else {
                return LRESULT(0);
            };
            if let Some(reason) = action.close {
                with_host_mut(hwnd, |host| host.set_closing_once(reason));
                // SAFETY: 不持有 Rust 借用；销毁窗口会让消息循环退出。
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            if action.show {
                with_host_ref(hwnd, Host::show_titlebar);
            }
            if action.hide {
                with_host_ref(hwnd, Host::hide_titlebar);
            }
            if action.repaint {
                with_host_ref(hwnd, |host| {
                    // SAFETY: overlay 有效。
                    unsafe {
                        let _ = InvalidateRect(Some(host.overlay), None, false);
                    }
                });
            }
            LRESULT(0)
        }
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            with_host_mut(hwnd, |host| host.titlebar.set_interacting(true));
            LRESULT(0)
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            with_host_mut(hwnd, |host| host.titlebar.set_interacting(false));
            LRESULT(0)
        }
        WM_NCLBUTTONDOWN => match wparam.0 as u32 {
            HTCAPTION => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
            HTMAXBUTTON => {
                toggle_maximized(hwnd);
                LRESULT(0)
            }
            HTMINBUTTON => {
                // SAFETY: 句柄有效。
                unsafe {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                LRESULT(0)
            }
            HTCLOSE => {
                // SAFETY: 句柄有效；异步关闭，避免在消息里重入 DestroyWindow。
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        },
        WM_NCLBUTTONDBLCLK => {
            if wparam.0 as u32 == HTCAPTION {
                toggle_maximized(hwnd);
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
        }
        WM_CLOSE => {
            with_host_mut(hwnd, |host| host.set_closing_once(CloseReason::UserClosed));
            // SAFETY: 不持有 Borrow；DestroyWindow 会触发 WM_DESTROY。
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: 线程消息循环退出。
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if with_host_ref(hwnd, |host| {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: 句柄有效；paint 是本闭包局部变量。
                let hdc = unsafe { BeginPaint(host.hwnd, &mut paint) };
                if !hdc.is_invalid() {
                    // SAFETY: 与 BeginPaint 配对。
                    unsafe {
                        let _ = EndPaint(host.hwnd, &paint);
                    }
                }
            })
            .is_none()
            {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            if with_host_ref(hwnd, paint::overlay).is_none() {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => {
            // 顶部 DWM/缩放边框优先还给宿主；overlay 不能把边缘 resize 吃掉。
            if resize_hit(hwnd, lparam).is_some() {
                return LRESULT(HTTRANSPARENT as isize);
            }
            // 只有最大化按钮返回 HTMAXBUTTON（尽力支持 Win11 Snap Layouts）；
            // 其余区域保持 HTCLIENT，让 overlay 收到普通鼠标消息。
            let mut point = POINT {
                x: win::loword(lparam.0),
                y: win::hiword(lparam.0),
            };
            // SAFETY: 屏幕坐标转 overlay 客户区坐标。
            unsafe {
                MapWindowPoints(None, Some(hwnd), std::slice::from_mut(&mut point));
            }
            let hit = with_host_ref(hwnd, |host| {
                layout::hit_test(point.x, point.y, host.client_w, host.dpi, true)
            });
            if hit == Some(Hit::Maximize) {
                LRESULT(HTMAXBUTTON as isize)
            } else {
                LRESULT(HTCLIENT as isize)
            }
        }
        WM_NCLBUTTONDOWN => {
            let code = wparam.0 as u32;
            if matches!(code, HTCAPTION | HTMAXBUTTON | HTMINBUTTON | HTCLOSE) {
                // 子窗口只负责转交，原生移动 / 最大化 / 最小化 / 关闭由宿主执行。
                forward_to_host(hwnd, WM_NCLBUTTONDOWN, code as usize, lparam);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            // WM_LBUTTONDOWN 的 lParam 已经是客户区坐标。
            let point = POINT {
                x: win::loword(lparam.0),
                y: win::hiword(lparam.0),
            };
            let hit = with_host_ref(hwnd, |host| {
                layout::hit_test(point.x, point.y, host.client_w, host.dpi, true)
            });
            match hit {
                Some(Hit::Caption) => {
                    forward_to_host(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, LPARAM(0));
                }
                Some(Hit::Minimize) => {
                    if let Some(host) = with_host_ref(hwnd, |host| host.hwnd) {
                        // SAFETY: 宿主句柄有效。
                        unsafe {
                            let _ = ShowWindow(host, SW_MINIMIZE);
                        }
                    }
                }
                Some(Hit::Maximize) => {
                    if let Some(host) = with_host_ref(hwnd, |host| host.hwnd) {
                        toggle_maximized(host);
                    }
                }
                Some(Hit::Close) => {
                    if let Some(host) = with_host_ref(hwnd, |host| host.hwnd) {
                        // SAFETY: 宿主句柄有效；异步关闭。
                        unsafe {
                            let _ = PostMessageW(Some(host), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            // WM_LBUTTONDBLCLK 的 lParam 同样是客户区坐标。
            let point = POINT {
                x: win::loword(lparam.0),
                y: win::hiword(lparam.0),
            };
            let hit = with_host_ref(hwnd, |host| {
                layout::hit_test(point.x, point.y, host.client_w, host.dpi, true)
            });
            if hit == Some(Hit::Caption)
                && let Some(host) = with_host_ref(hwnd, |host| host.hwnd)
            {
                toggle_maximized(host);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn forward_to_host(overlay: HWND, message: u32, code: usize, lparam: LPARAM) {
    // SAFETY: 释放 overlay 可能持有的捕获，再让宿主执行原生移动 / 最大化。
    unsafe {
        let _ = ReleaseCapture();
    }
    if let Some(host) = with_host_ref(overlay, |host| host.hwnd) {
        // SAFETY: 宿主句柄有效；SendMessageW 同步交给宿主 WndProc。
        unsafe {
            SendMessageW(host, message, Some(WPARAM(code)), Some(lparam));
        }
    }
}

fn toggle_maximized(hwnd: HWND) {
    // SAFETY: 句柄有效；只切换普通 / 最大化。
    unsafe {
        if IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_protocol_names_are_stable() {
        // 窗口类名 / 高度与 layout 绑定；改名会破坏单实例检测。
        assert!(HOST_CLASS.starts_with("NowherePlayer"));
        assert_eq!(layout::titlebar_height(96), 32);
        assert_eq!(TIMER_INTERVAL_MS, 50);
    }

    #[test]
    fn video_surface_and_host_clip_siblings_so_the_overlay_stays_on_top() {
        // 真实 bug：surface 不带 WS_CLIPSIBLINGS 时，它的黑色背景会把上方的
        // titlebar overlay 盖掉；这里锁住修复。
        let surface = surface_style();
        assert_ne!(
            surface.0 & WS_CLIPSIBLINGS.0,
            0,
            "surface must clip the titlebar overlay when it repaints"
        );
        assert_ne!(surface.0 & WS_CLIPCHILDREN.0, 0);
        assert_ne!(host_style().0 & WS_CLIPCHILDREN.0, 0);
        assert_ne!(overlay_style().0 & WS_CLIPSIBLINGS.0, 0);
    }

    #[test]
    fn host_keeps_native_resize_and_maximize_styles() {
        use windows::Win32::UI::WindowsAndMessaging::{WS_MAXIMIZEBOX, WS_THICKFRAME};

        // T-0106：客户区铺满整个窗口，但原生 THICKFRAME / MAXIMIZEBOX
        // 必须保留，边缘缩放和 Aero Snap 靠它们 + WM_NCHITTEST。
        let style = host_style();
        assert_ne!(style.0 & WS_THICKFRAME.0, 0);
        assert_ne!(style.0 & WS_MAXIMIZEBOX.0, 0);
    }

    #[test]
    fn resize_edges_map_to_native_hit_codes() {
        assert_eq!(edge_code(ResizeEdge::Left), HTLEFT as isize);
        assert_eq!(edge_code(ResizeEdge::Right), HTRIGHT as isize);
        assert_eq!(edge_code(ResizeEdge::Top), HTTOP as isize);
        assert_eq!(edge_code(ResizeEdge::Bottom), HTBOTTOM as isize);
        assert_eq!(edge_code(ResizeEdge::TopLeft), HTTOPLEFT as isize);
        assert_eq!(edge_code(ResizeEdge::TopRight), HTTOPRIGHT as isize);
        assert_eq!(edge_code(ResizeEdge::BottomLeft), HTBOTTOMLEFT as isize);
        assert_eq!(edge_code(ResizeEdge::BottomRight), HTBOTTOMRIGHT as isize);
    }
}
