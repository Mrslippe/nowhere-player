//! Win32 宿主窗口：原生缩放边框 + 去掉 caption + 悬浮标题栏 + 底部控制条 + mpv 子窗口。
//!
//! 窗口结构（Z 序从下到上）：
//!
//! ```text
//! 顶层窗口（WS_OVERLAPPEDWINDOW，保留 THICKFRAME / MAXIMIZEBOX）
//!   └─ 视频子窗口（始终 = 客户区 Rect）
//!   └─ 标题栏 overlay（顶部 32px，只在显示时覆盖视频）
//!   └─ 控制条 overlay（底部 40px，鼠标在客户区时覆盖视频）
//! ```
//!
//! 显示 / 隐藏两个 overlay 只动 overlay；视频子窗口的 Rect 只在客户区尺寸
//! 变化时更新。

mod icons;
mod paint;
mod win;

use std::cell::RefCell;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, HBRUSH, HFONT, InvalidateRect,
    MapWindowPoints, PAINTSTRUCT, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GA_ROOT, GWLP_USERDATA, GetAncestor,
    GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
    HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, HTTRANSPARENT, HWND_TOP, IDC_ARROW, IsIconic, IsZoomed, LoadCursorW, MF_CHECKED,
    MF_STRING, MINMAXINFO, MSG, NCCALCSIZE_PARAMS, PostMessageW, PostQuitMessage, RegisterClassExW,
    SW_HIDE, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TPM_LEFTALIGN,
    TPM_RETURNCMD, TPM_TOPALIGN, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ENTERMENULOOP, WM_ENTERSIZEMOVE,
    WM_ERASEBKGND, WM_EXITMENULOOP, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_NCLBUTTONDOWN, WM_PAINT, WM_SIZE, WM_SIZING, WM_TIMER, WNDCLASSEXW, WNDPROC, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WindowFromPoint,
};
use windows::core::PCWSTR;

use crate::layout::{
    self, ControlBarState, ControlHit, Hit, OverlaySlide, ResizeEdge, TitlebarState,
};
use crate::mpv;
use crate::protocol::{Event, Request, Theme};

const HOST_CLASS: &str = "NowherePlayerVideoHost";
const SURFACE_CLASS: &str = "NowherePlayerVideoSurface";
const OVERLAY_CLASS: &str = "NowherePlayerVideoTitleBar";
const CONTROL_CLASS: &str = "NowherePlayerVideoControls";
const TIMER_ID: usize = 1;
/// 16ms：滑入 / 滑出期间约 60fps 的 SetWindowPos；静止时一次 tick 也很轻。
const TIMER_INTERVAL_MS: u32 = 16;
/// 呼出 overlay 的动画时长（ease-out）。
const SHOW_DURATION: Duration = Duration::from_millis(160);
/// 收起 overlay 的动画时长（ease-in）；走完才 `SW_HIDE`，不留残影。
const HIDE_DURATION: Duration = Duration::from_millis(140);
/// 清晰度菜单命令 ID 起点；0 表示用户点了菜单外（TPM_RETURNCMD）。
const QUALITY_MENU_ID_BASE: usize = 1000;
/// `loadfile` 最多等多久；超时按失败回退，避免 TUI 一直停在旧状态。
const LOAD_TIMEOUT: Duration = Duration::from_millis(5_000);
/// 换流失败时回给 TUI 的固定提示；以“清晰度”开头，TUI 据此保留旧画面。
const LOAD_FAILED_MESSAGE: &str = "清晰度切换失败，已恢复原画面";

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

/// 底部控制条 overlay：盖在视频上，初始不显示。
fn control_style() -> WINDOW_STYLE {
    WS_CHILD | WS_CLIPSIBLINGS
}

/// TUI 通过 stdin 发来的控制消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Quit,
    Volume(u8),
    Pause(bool),
    /// 运行期主题变化：只重绘标题栏 / 控制条，不重启 mpv。
    Theme(Theme),
    /// 清晰度切换：TUI 已按 qn 取到新 URL，让现有 mpv 进程换流。
    Load(String),
}

/// 窗口结束的原因；`run` 返回给 main 决定最后一行协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    UserClosed,
    TuiQuit,
    MpvExited(Option<i32>),
    LoopError,
}

/// 一次等待 mpv 回复的换流请求。
struct PendingLoad {
    request_id: u64,
    url: String,
    previous: String,
    deadline: Instant,
}

struct Host {
    hwnd: HWND,
    surface: HWND,
    overlay: HWND,
    controls: HWND,
    title: String,
    dpi: u32,
    client_w: i32,
    client_h: i32,
    titlebar: TitlebarState,
    control_bar: ControlBarState,
    /// 标题栏滑入 / 滑出：隐藏偏移 -height，显示偏移 0。
    titlebar_slide: OverlaySlide,
    /// 控制条滑入 / 滑出：隐藏偏移 client_h，显示偏移 bar.top。
    controls_slide: OverlaySlide,
    is_maximized: bool,
    mpv: Option<Child>,
    ipc: Option<crate::mpv_ipc::MpvIpc>,
    control: Receiver<Control>,
    closing: Option<CloseReason>,
    font: HFONT,
    /// 控制条图标样式：优先系统图标字体，缺失时细分几何回退。
    icon_style: icons::IconStyle,
    /// 图标字体句柄；细线样式为空。DPI 变化时重建，退出时删除。
    icon_font: HFONT,
    /// TUI 传入的主题；标题栏 / 控制条都只从这里取色。
    theme: Theme,
    volume: u8,
    paused: bool,
    /// 控制条当前 hover 的控件；`None` = 鼠标不在可 hover 控件上。
    hovered_control: Option<ControlHit>,
    /// 静音前的音量；控制条上的喇叭按钮据此恢复。
    last_volume: u8,
    /// 当前实际在播（或最后一次成功）的 URL；只留在内存，不写日志。
    current_url: String,
    pending_load: Option<PendingLoad>,
    next_load_id: u64,
    qualities: Vec<(u64, String)>,
    current_qn: Option<u64>,
    dragging_volume: bool,
    last_cache_text: Option<String>,
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
    let control_class = win::wide(CONTROL_CLASS);
    // SAFETY: 画刷只作为类背景，进程退出时随进程回收。
    let background = unsafe { CreateSolidBrush(COLORREF(0)) };

    register_class(&host_class, instance, Some(host_proc), background)?;
    register_class(&surface_class, instance, Some(surface_proc), background)?;
    register_class(&overlay_class, instance, Some(overlay_proc), background)?;
    register_class(&control_class, instance, Some(control_proc), background)?;

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

    // 底部控制条：独立子窗口，盖在视频 surface 之上；初始隐藏。
    // SAFETY: 同上。
    let controls = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            win::pcwstr(&control_class),
            PCWSTR::null(),
            control_style(),
            0,
            0,
            client_w,
            0,
            Some(hwnd),
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("控制条创建失败：{}", error.message()))?;

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
        controls,
        title,
        dpi,
        client_w,
        client_h,
        titlebar: TitlebarState::new(),
        control_bar: ControlBarState::new(),
        titlebar_slide: OverlaySlide::new(
            layout::titlebar_hidden_offset(dpi),
            layout::titlebar_shown_offset(),
        ),
        controls_slide: OverlaySlide::new(
            layout::controls_hidden_offset(client_h),
            layout::controls_shown_offset(client_w, client_h, dpi),
        ),
        is_maximized: false,
        mpv: None,
        ipc: None,
        control,
        closing: None,
        font: HFONT(std::ptr::null_mut()),
        icon_style: icons::IconStyle::Line,
        icon_font: HFONT(std::ptr::null_mut()),
        theme: request.theme.unwrap_or_default(),
        volume: request.volume.min(100),
        paused: request.paused,
        hovered_control: None,
        last_volume: request.volume.clamp(1, 100),
        current_url: request.url.clone(),
        pending_load: None,
        next_load_id: QUALITY_MENU_ID_BASE as u64,
        qualities: request.qualities.clone(),
        current_qn: request.current_qn,
        dragging_volume: false,
        last_cache_text: None,
    };
    let cell = RefCell::new(host);
    let cell_ptr = (&cell as *const RefCell<Host>) as isize;
    // SAFETY: 窗口一直活到 `run` 末尾；cell 在消息循环期间不会被移动。
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cell_ptr);
        SetWindowLongPtrW(surface, GWLP_USERDATA, cell_ptr);
        SetWindowLongPtrW(overlay, GWLP_USERDATA, cell_ptr);
        SetWindowLongPtrW(controls, GWLP_USERDATA, cell_ptr);
    }

    {
        let mut host = cell.borrow_mut();
        host.font = paint::create_font(host.dpi);
        let (icon_style, icon_font) = icons::load(host.dpi);
        host.icon_style = icon_style;
        host.icon_font = icon_font;
        log::info!(
            target: "video",
            "event=icons style={} dpi={}",
            icon_style.as_str(),
            host.dpi
        );
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
        Ok(child) => {
            cell.borrow_mut().mpv = Some(child);
            cell.borrow_mut().ipc = Some(crate::mpv_ipc::MpvIpc::connect(request.buffer_ms));
        }
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
    log::info!(
        target: "video",
        "ready hwnd={} client={}x{} buffer_ms={}",
        hwnd.0 as usize,
        client_w,
        client_h,
        request.buffer_ms
    );

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
        log::error!(target: "video", "message loop error");
        return Ok(CloseReason::LoopError);
    }
    let reason = host.closing.unwrap_or(CloseReason::LoopError);
    log::info!(target: "video", "close reason={reason:?}");
    Ok(reason)
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
    /// 控制条图标绘制上下文；把样式 / 字体 / DPI 一次性交给 `icons`。
    fn icon_painter(&self) -> icons::Painter {
        icons::Painter {
            style: self.icon_style,
            font: self.icon_font,
            dpi: self.dpi,
        }
    }

    /// 客户区变化：视频子窗口跟随客户区，overlay 跟随各自区域。
    ///
    /// **只有这里会 SetWindowPos 视频子窗口。** 标题栏 / 控制条的滑入滑出走
    /// [`Self::advance_titlebar`] / [`Self::advance_controls`]，只移动 overlay，
    /// 绝不 resize 视频。
    fn resize_client(&mut self, width: i32, height: i32) {
        self.client_w = width.max(0);
        self.client_h = height.max(0);
        let video = layout::video_rect(self.client_w, self.client_h);
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
        }
        // 客户区变了：显示中的 overlay 直接落到最终位置，隐藏中的用新几何。
        self.sync_overlays();
        // surface 和 mpv 的 `--wid` 子窗口是父子关系：surface resize 后必须
        // 把 mpv 子窗口也拉到客户区大小，否则视频按旧尺寸居中、四周出现黑边。
        // mpv 还没创建子窗口时返回 false，下一次 WM_SIZE 会补上。
        win::resize_mpv_child(self.surface, self.client_w, self.client_h);
    }

    /// 客户区 / DPI 变化后同步两个 overlay。
    ///
    /// 正在显示的 overlay 直接落到最终位置；隐藏 / 正在收起的 overlay 用新
    /// 几何落到隐藏位置并 `SW_HIDE`，下次呼出不会从旧尺寸 / 旧偏移跳变。
    fn sync_overlays(&mut self) {
        // 几何先更新；显示中的直接落最终位置，隐藏 / 收起中的落隐藏位置。
        let titlebar_was_sliding = self.titlebar_slide.set_geometry(
            layout::titlebar_hidden_offset(self.dpi),
            layout::titlebar_shown_offset(),
        );
        if self.titlebar.is_visible() {
            self.show_titlebar_at(self.titlebar_slide.shown());
        } else {
            if titlebar_was_sliding {
                // 收起途中被 resize 打断：立即隐藏，避免停在半路。
                self.hide_titlebar();
            }
            self.position_titlebar(self.titlebar_slide.hidden());
        }

        let controls_were_sliding = self.controls_slide.set_geometry(
            layout::controls_hidden_offset(self.client_h),
            layout::controls_shown_offset(self.client_w, self.client_h, self.dpi),
        );
        if self.control_bar.is_visible() {
            self.show_controls_at(self.controls_slide.shown());
        } else {
            if controls_were_sliding {
                self.hide_controls();
                self.hovered_control = None;
            }
            self.position_controls(self.controls_slide.hidden());
        }
    }

    /// 把标题栏 overlay 放到客户区 y=`offset`；只动 overlay，视频 Rect 不变。
    fn position_titlebar(&self, offset: i32) {
        let overlay = layout::overlay_rect(self.client_w, self.dpi);
        // SAFETY: overlay 有效。
        unsafe {
            let _ = SetWindowPos(
                self.overlay,
                Some(HWND_TOP),
                overlay.left,
                offset,
                overlay.width(),
                overlay.height(),
                SWP_NOACTIVATE,
            );
        }
    }

    /// 只把标题栏标记为待重绘；真正的 WM_PAINT 在 tick 返回后发生。
    fn invalidate_titlebar(&self) {
        // SAFETY: overlay 有效。
        unsafe {
            let _ = InvalidateRect(Some(self.overlay), None, false);
        }
    }

    /// 只把控制条标记为待重绘；真正的 WM_PAINT 在 tick 返回后发生。
    fn invalidate_controls(&self) {
        // SAFETY: controls 有效。
        unsafe {
            let _ = InvalidateRect(Some(self.controls), None, false);
        }
    }

    /// 显示标题栏并放到指定偏移（滑入起点或终点）。
    fn show_titlebar_at(&self, offset: i32) {
        // 硬约束 2：只 ShowWindow/SWP overlay，视频子窗口 Rect 完全不动。
        // SAFETY: overlay 有效。
        unsafe {
            let _ = ShowWindow(self.overlay, SW_SHOWNOACTIVATE);
        }
        self.position_titlebar(offset);
        self.invalidate_titlebar();
    }

    fn hide_titlebar(&self) {
        // SAFETY: overlay 有效；只隐藏 overlay。
        unsafe {
            let _ = ShowWindow(self.overlay, SW_HIDE);
        }
        // 子窗口隐藏不会自动重绘被它盖住的兄弟区域；这里只标脏，让消息循环
        // 在 tick 返回后补画，避免在 &mut Host 借用期间重入 WM_PAINT。
        self.repaint_children();
    }

    /// 让视频 surface 补画被 overlay 盖过的区域（不含 UPDATENOW，避免重入）。
    fn repaint_children(&self) {
        // SAFETY: 宿主句柄有效。
        unsafe {
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE,
            );
        }
    }

    /// 把控制条 overlay 放到客户区 y=`offset`；只动 overlay，视频 Rect 不变。
    fn position_controls(&self, offset: i32) {
        let controls = layout::control_bar_rect(self.client_w, self.client_h, self.dpi);
        // SAFETY: controls 有效。
        unsafe {
            let _ = SetWindowPos(
                self.controls,
                Some(HWND_TOP),
                controls.left,
                offset,
                controls.width(),
                controls.height(),
                SWP_NOACTIVATE,
            );
        }
    }

    /// 显示控制条并放到指定偏移（滑入起点或终点）。
    fn show_controls_at(&self, offset: i32) {
        // 硬约束：只 ShowWindow/SWP 控制条 overlay，视频子窗口 Rect 完全不动。
        // SAFETY: controls 有效。
        unsafe {
            let _ = ShowWindow(self.controls, SW_SHOWNOACTIVATE);
        }
        self.position_controls(offset);
        self.invalidate_controls();
    }

    fn hide_controls(&self) {
        // SAFETY: controls 有效；只隐藏 overlay，视频 surface 不动。
        unsafe {
            let _ = ShowWindow(self.controls, SW_HIDE);
        }
        self.repaint_children();
    }

    /// 每 tick 推进一步；没有动画时什么都不做。
    fn advance_titlebar(&mut self, now: Instant) {
        let Some((offset, finished)) = self.titlebar_slide.advance(now) else {
            return;
        };
        if !finished {
            self.position_titlebar(offset);
        } else if self.titlebar.is_visible() {
            self.position_titlebar(self.titlebar_slide.shown());
        } else {
            self.hide_titlebar();
        }
    }

    /// 每 tick 推进一步；收起完成后才清 hover。
    fn advance_controls(&mut self, now: Instant) {
        let Some((offset, finished)) = self.controls_slide.advance(now) else {
            return;
        };
        if !finished {
            self.position_controls(offset);
        } else if self.control_bar.is_visible() {
            self.position_controls(self.controls_slide.shown());
        } else {
            self.hide_controls();
            self.hovered_control = None;
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
        // 图标字体字号随 DPI 变化；重建失败时真回退到细线，不留空样式。
        let old_icon = self.icon_font;
        self.icon_font = icons::reload(self.icon_style, dpi);
        if self.icon_style != icons::IconStyle::Line && self.icon_font.is_invalid() {
            self.icon_style = icons::IconStyle::Line;
            self.icon_font = HFONT(std::ptr::null_mut());
            log::warn!(target: "video", "event=icons fallback=line dpi={dpi}");
        }
        if !old_icon.0.is_null() {
            // SAFETY: 旧图标字体由本窗口拥有，替换后删除。
            unsafe {
                let _ = DeleteObject(old_icon.into());
            }
        }
        // 显示中的直接落位、隐藏中的用新几何；随后 WM_SIZE 会再同步一次。
        self.sync_overlays();
    }

    /// 每 16ms tick：处理控制消息、推进滑入滑出、刷新 overlay。
    ///
    /// 返回 `Some` 表示宿主该关闭了；动画只移动 overlay，不 resize 视频。
    fn tick(&mut self) -> Option<CloseReason> {
        if self.closing.is_some() {
            return None;
        }
        loop {
            match self.control.try_recv() {
                Ok(Control::Quit) => {
                    log::info!(target: "video", "control quit");
                    return Some(CloseReason::TuiQuit);
                }
                Ok(Control::Volume(volume)) => {
                    self.apply_volume(volume);
                    self.invalidate_controls();
                }
                Ok(Control::Pause(paused)) => {
                    self.apply_pause(paused);
                    self.invalidate_controls();
                }
                Ok(Control::Theme(theme)) => {
                    if theme != self.theme {
                        self.theme = theme;
                        // 标题栏和控制条一起换肤；视频画面与 mpv 不动。
                        self.invalidate_titlebar();
                        self.invalidate_controls();
                        log::info!(target: "video", "event=theme_updated");
                    }
                }
                Ok(Control::Load(url)) => self.start_load(url),
                Err(_) => break,
            }
        }

        if let Some(child) = self.mpv.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!(target: "video", "mpv exited code={:?}", status.code());
                    self.mpv = None;
                    return Some(CloseReason::MpvExited(status.code()));
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }

        if self.poll_load() {
            self.invalidate_controls();
        }

        let cursor = self.cursor_client();
        let now = Instant::now();
        let update = self.titlebar.update(cursor, now, self.client_w, self.dpi);
        if update.visibility_changed {
            if self.titlebar.is_visible() {
                let from = self.titlebar_slide.begin_show(now, SHOW_DURATION);
                self.show_titlebar_at(from);
            } else {
                self.titlebar_slide.begin_hide(now, HIDE_DURATION);
            }
        }
        if update.hover_changed || update.visibility_changed {
            self.invalidate_titlebar();
        }
        // 每 tick 推进一步；没有动画时这里什么都不做。
        self.advance_titlebar(now);

        let control_update =
            self.control_bar
                .update(cursor, now, self.client_w, self.client_h, self.dpi);
        if control_update.visibility_changed {
            if self.control_bar.is_visible() {
                let from = self.controls_slide.begin_show(now, SHOW_DURATION);
                self.show_controls_at(from);
            } else {
                self.controls_slide.begin_hide(now, HIDE_DURATION);
            }
        }
        // 收起动画走完才清 hover（在 advance_controls 内），动画中保持正常。
        self.advance_controls(now);

        // cache 小字每 tick 比对一次；只有显示文本变化才重绘控制条。
        let cache_text = self.cache_text();
        if self.last_cache_text.as_deref() != Some(cache_text.as_str()) {
            self.last_cache_text = Some(cache_text);
            self.invalidate_controls();
        }
        None
    }

    /// 本地暂停/继续 mpv 并同步控制条状态；TUI 侧事件由调用方发。
    fn apply_volume(&mut self, volume: u8) {
        let volume = volume.min(100);
        if volume > 0 {
            self.last_volume = volume;
        }
        self.volume = volume;
        if let Some(ipc) = &self.ipc {
            ipc.set_volume(volume);
        }
    }

    fn apply_pause(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(ipc) = &self.ipc {
            ipc.set_paused(paused);
        }
    }

    /// 控制条播放/暂停：本地改 mpv，并通知 TUI 更新状态（TUI 不回发 pause）。
    fn toggle_pause(&mut self) {
        let paused = !self.paused;
        self.apply_pause(paused);
        log::info!(target: "video", "control paused={paused}");
        win::emit(Event::Paused(paused));
    }

    /// 控制条音量：本地改 mpv + 发 `volume <n>`；TUI 只更新状态。
    fn control_volume(&mut self, volume: u8) {
        if volume == self.volume {
            return;
        }
        self.apply_volume(volume);
        log::info!(target: "video", "control volume={}", self.volume);
        win::emit(Event::Volume(self.volume));
    }

    fn toggle_mute(&mut self) {
        if self.volume == 0 {
            let restored = self.last_volume.max(1);
            self.control_volume(restored);
        } else {
            self.control_volume(0);
        }
    }

    /// 控制条刷新：TUI 重新取当前 qn 的流，再通过 `load` 发回来。
    fn request_refresh(&mut self) {
        log::info!(target: "video", "control refresh qn={:?}", self.current_qn);
        win::emit(Event::Refresh);
    }

    fn select_quality(&mut self, qn: u64) {
        if self.current_qn == Some(qn) {
            return;
        }
        self.current_qn = Some(qn);
        log::info!(target: "video", "control quality qn={qn}");
        win::emit(Event::Quality(qn));
    }

    /// 向现有 mpv 进程发 `loadfile <url> replace`；URL 不写日志。
    fn start_load(&mut self, url: String) {
        if crate::protocol::validate_stream_url(&url).is_err() {
            log::warn!(target: "video", "control load rejected");
            return;
        }
        self.next_load_id = self
            .next_load_id
            .wrapping_add(1)
            .max(QUALITY_MENU_ID_BASE as u64);
        let request_id = self.next_load_id;
        if let Some(ipc) = &self.ipc {
            ipc.loadfile(&url, request_id);
        }
        self.pending_load = Some(PendingLoad {
            request_id,
            url,
            previous: self.current_url.clone(),
            deadline: Instant::now() + LOAD_TIMEOUT,
        });
        log::info!(target: "video", "event=load_requested id={request_id}");
    }

    /// 检查 `loadfile` 回复；成功确认新 URL，失败回退旧 URL 并发 `error`。
    ///
    /// 返回是否重绘了控制条。
    fn poll_load(&mut self) -> bool {
        let Some(pending) = &self.pending_load else {
            return false;
        };
        let result = self
            .ipc
            .as_ref()
            .and_then(|ipc| ipc.load_result(pending.request_id));
        let expired = Instant::now() >= pending.deadline;
        if result.is_none() && !expired {
            return false;
        }
        let pending = self.pending_load.take().expect("pending checked above");
        match result {
            Some(Ok(())) => {
                self.current_url = pending.url;
                log::info!(target: "video", "event=load_ok id={}", pending.request_id);
                // loadfile 换流后重新应用音量 / 暂停，保持用户状态。
                if let Some(ipc) = &self.ipc {
                    ipc.set_volume(self.volume);
                    ipc.set_paused(self.paused);
                }
            }
            other => {
                if expired {
                    log::warn!(
                        target: "video",
                        "event=load_timeout id={}",
                        pending.request_id
                    );
                } else if other.is_some() {
                    // mpv 的错误文本不落日志：只记请求 id，避免任何 URL 变体。
                    log::warn!(
                        target: "video",
                        "event=load_failed id={}",
                        pending.request_id
                    );
                }
                // 回退上一 URL；结果不再等待，避免再次触发回退。
                self.next_load_id = self
                    .next_load_id
                    .wrapping_add(1)
                    .max(QUALITY_MENU_ID_BASE as u64);
                if let Some(ipc) = &self.ipc
                    && !pending.previous.is_empty()
                {
                    ipc.loadfile(&pending.previous, self.next_load_id);
                }
                win::emit(Event::Error(LOAD_FAILED_MESSAGE.to_owned()));
            }
        }
        true
    }

    fn cache_text(&self) -> String {
        format_cache_ms(self.ipc.as_ref().and_then(crate::mpv_ipc::MpvIpc::cache_ms))
    }

    /// 当前清晰度标签；没有菜单项时为空。
    fn quality_label(&self) -> &str {
        self.qualities
            .iter()
            .find(|(qn, _)| Some(*qn) == self.current_qn)
            .map(|(_, label)| label.as_str())
            .unwrap_or("")
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
        // 关闭 / 退出时立即停止滑入滑出，不等动画走完。
        self.titlebar_slide.stop();
        self.controls_slide.stop();
        self.ipc = None;
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
        if !self.icon_font.0.is_null() {
            // SAFETY: 图标字体由本窗口创建，退出时释放。
            unsafe {
                let _ = DeleteObject(self.icon_font.into());
            }
            self.icon_font = HFONT(std::ptr::null_mut());
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

/// cache 小字：`1.5s` / `--`；与 T-0112 遥测单位一致。
fn format_cache_ms(cache_ms: Option<u64>) -> String {
    match cache_ms {
        Some(ms) => format!("{:.1}s", ms as f64 / 1_000.0),
        None => "--".to_owned(),
    }
}

/// 原生清晰度 popup menu：当前 qn 打勾，返回用户选中的 qn。
///
/// 菜单期间不持有 `RefCell<Host>` 借用：`TrackPopupMenu` 会开嵌套消息循环，
/// 期间 WM_ENTERMENULOOP / WM_TIMER 仍需访问 Host。
fn popup_quality_menu(
    hwnd: HWND,
    qualities: &[(u64, String)],
    current_qn: Option<u64>,
) -> Option<u64> {
    if qualities.is_empty() {
        return None;
    }
    // SAFETY: 菜单由本函数创建，返回前销毁。
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    for (index, (qn, label)) in qualities.iter().enumerate() {
        let mut flags = MF_STRING;
        if Some(*qn) == current_qn {
            flags |= MF_CHECKED;
        }
        let label = win::wide(label);
        // SAFETY: 菜单有效；标签在调用期间有效（AppendMenuW 会复制）。
        unsafe {
            let _ = AppendMenuW(
                menu,
                flags,
                QUALITY_MENU_ID_BASE + index,
                win::pcwstr(&label),
            );
        }
    }
    let mut point = POINT::default();
    // SAFETY: 纯查询；把前台交给宿主，菜单失焦时会正常关闭。
    unsafe {
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
    }
    // SAFETY: 菜单 / 宿主有效；TPM_RETURNCMD 返回命令 ID 而不是发 WM_COMMAND。
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        )
    };
    // SAFETY: 菜单用完即销毁。
    unsafe {
        let _ = DestroyMenu(menu);
    }
    let command = command.0 as usize;
    if command < QUALITY_MENU_ID_BASE {
        return None;
    }
    qualities
        .get(command - QUALITY_MENU_ID_BASE)
        .map(|(qn, _)| *qn)
}

/// 把控制条子窗口坐标转成宿主客户区坐标。
fn control_point(hwnd: HWND, lparam: LPARAM) -> Option<POINT> {
    let mut point = POINT {
        x: win::loword(lparam.0),
        y: win::hiword(lparam.0),
    };
    let host = with_host_ref(hwnd, |host| host.hwnd)?;
    // SAFETY: 两个子窗口属于同一线程；只做坐标换算。
    unsafe {
        MapWindowPoints(Some(hwnd), Some(host), std::slice::from_mut(&mut point));
    }
    Some(point)
}

fn control_hit_at(hwnd: HWND, point: POINT) -> Option<ControlHit> {
    with_host_ref(hwnd, |host| {
        layout::control_hit(point.x, point.y, host.client_w, host.client_h, host.dpi)
    })?
}

unsafe extern "system" fn control_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            if with_host_ref(hwnd, paint::controls).is_none() {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => {
            // 控制条在底部 12px 边距之上；边框命中仍还给宿主。
            if resize_hit(hwnd, lparam).is_some() {
                return LRESULT(HTTRANSPARENT as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_LBUTTONDOWN => {
            let Some(point) = control_point(hwnd, lparam) else {
                return LRESULT(0);
            };
            let inside = with_host_ref(hwnd, |host| {
                layout::control_bar_contains(
                    point.x,
                    point.y,
                    host.client_w,
                    host.client_h,
                    host.dpi,
                )
            })
            .unwrap_or(false);
            if !inside {
                return LRESULT(0);
            }
            match control_hit_at(hwnd, point) {
                Some(ControlHit::PlayPause) => {
                    with_host_mut(hwnd, Host::toggle_pause);
                }
                Some(ControlHit::Refresh) => {
                    with_host_mut(hwnd, Host::request_refresh);
                }
                Some(ControlHit::Mute) => {
                    with_host_mut(hwnd, Host::toggle_mute);
                }
                Some(ControlHit::Volume) => {
                    // 按下滑杆：抓取鼠标，拖动期间持续更新音量。
                    with_host_mut(hwnd, |host| {
                        host.dragging_volume = true;
                        host.control_bar.set_interacting(true);
                        let volume =
                            layout::slider_volume(point.x, host.client_w, host.client_h, host.dpi);
                        host.control_volume(volume);
                    });
                    // SAFETY: 只抓取鼠标；WM_LBUTTONUP / WM_CAPTURECHANGED 释放。
                    unsafe {
                        SetCapture(hwnd);
                    }
                }
                Some(ControlHit::Quality) => {
                    let menu = with_host_ref(hwnd, |host| {
                        (host.hwnd, host.qualities.clone(), host.current_qn)
                    });
                    if let Some((host_hwnd, qualities, current_qn)) = menu
                        && let Some(qn) = popup_quality_menu(host_hwnd, &qualities, current_qn)
                    {
                        with_host_mut(hwnd, |host| host.select_quality(qn));
                    }
                }
                None => {}
            }
            // 点击后立即重绘（播放/暂停图标、滑杆、清晰度勾选）。
            with_host_ref(hwnd, |host| {
                // SAFETY: controls 有效。
                unsafe {
                    let _ = InvalidateRect(Some(host.controls), None, false);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let Some(point) = control_point(hwnd, lparam) else {
                return LRESULT(0);
            };
            // 注册离开通知；鼠标离开控制条时由 WM_MOUSELEAVE 清 hover。
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: track 是本函数局部变量；hwnd 有效。
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            with_host_mut(hwnd, |host| {
                if host.dragging_volume {
                    let volume =
                        layout::slider_volume(point.x, host.client_w, host.client_h, host.dpi);
                    host.control_volume(volume);
                }
                let hovered = if host.control_bar.is_visible() {
                    layout::control_hit(point.x, point.y, host.client_w, host.client_h, host.dpi)
                } else {
                    None
                };
                if host.hovered_control != hovered {
                    host.hovered_control = hovered;
                    // SAFETY: controls 有效。
                    unsafe {
                        let _ = InvalidateRect(Some(host.controls), None, false);
                    }
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_host_mut(hwnd, |host| {
                if host.hovered_control.is_some() {
                    host.hovered_control = None;
                    // SAFETY: controls 有效。
                    unsafe {
                        let _ = InvalidateRect(Some(host.controls), None, false);
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_host_mut(hwnd, |host| {
                host.dragging_volume = false;
                host.control_bar.set_interacting(false);
            });
            // SAFETY: 与 SetCapture 配对。
            unsafe {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            // 拖动中被别处抢走捕获：复位，避免控制条永不收起。
            with_host_mut(hwnd, |host| {
                host.dragging_volume = false;
                host.control_bar.set_interacting(false);
            });
            LRESULT(0)
        }
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
        WM_SIZING => {
            if unsafe { IsZoomed(hwnd) }.as_bool() {
                return LRESULT(0);
            }
            let dpi = with_host_ref(hwnd, |host| host.dpi).unwrap_or(96);
            let (min_w, min_h) = layout::min_track_size(dpi);
            // SAFETY: WM_SIZING 的 lParam 指向可写的窗口 RECT。
            let rect = unsafe { &mut *(lparam.0 as *mut RECT) };
            let next = layout::constrain_aspect(win::from_rect(*rect), wparam.0, min_w, min_h);
            *rect = win::to_rect(next);
            // 有些合成/拖动路径不会在拖动中及时派发 WM_SIZE；直接按新矩形同步
            // surface / overlay / mpv 子窗口，保证拖动时视频和标题栏跟手。
            with_host_mut(hwnd, |host| {
                host.resize_client(next.width(), next.height());
            });
            // 返回 TRUE，允许 Windows 使用修正后的矩形。
            LRESULT(1)
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
            let close = with_host_mut(hwnd, |host| host.tick());
            let Some(close) = close else {
                return LRESULT(0);
            };
            if let Some(reason) = close {
                with_host_mut(hwnd, |host| host.set_closing_once(reason));
                // SAFETY: 不持有 Rust 借用；销毁窗口会让消息循环退出。
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            with_host_mut(hwnd, |host| {
                host.titlebar.set_interacting(true);
                host.control_bar.set_interacting(true);
            });
            LRESULT(0)
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let (width, height) = win::client_size(hwnd);
            with_host_mut(hwnd, |host| {
                host.titlebar.set_interacting(false);
                host.control_bar.set_interacting(false);
                host.dragging_volume = false;
                host.resize_client(width, height);
            });
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
        assert!(CONTROL_CLASS.starts_with("NowherePlayer"));
        assert_eq!(layout::titlebar_height(96), 32);
        assert_eq!(layout::control_bar_height(96), 40);
        assert_eq!(TIMER_INTERVAL_MS, 16);
        assert_eq!(QUALITY_MENU_ID_BASE, 1000);
    }

    #[test]
    fn overlay_animation_constants_match_the_card() {
        // T-0120：呼出 160ms ease-out，收起 140ms；16ms 定时器约 60fps。
        assert_eq!(SHOW_DURATION, Duration::from_millis(160));
        assert_eq!(HIDE_DURATION, Duration::from_millis(140));
        assert_eq!(TIMER_INTERVAL_MS, 16);
        assert!(
            SHOW_DURATION.as_millis() / u128::from(TIMER_INTERVAL_MS) >= 10,
            "160ms should get about ten frames"
        );
        // 滑入终点与静止 Rect 一致；视频 Rect 不参与动画。
        let (width, height, dpi) = (960, 540, 96);
        assert_eq!(layout::titlebar_hidden_offset(dpi), -32);
        assert_eq!(layout::titlebar_shown_offset(), 0);
        assert_eq!(layout::controls_hidden_offset(height), height);
        assert_eq!(layout::controls_shown_offset(width, height, dpi), 488);
        assert_eq!(
            layout::overlay_rect(width, dpi).top,
            layout::titlebar_shown_offset()
        );
        assert_eq!(layout::video_rect(width, height).height(), height);
    }

    #[test]
    fn cache_text_shows_seconds_or_a_placeholder() {
        assert_eq!(format_cache_ms(None), "--");
        assert_eq!(format_cache_ms(Some(0)), "0.0s");
        assert_eq!(format_cache_ms(Some(1_500)), "1.5s");
        assert_eq!(format_cache_ms(Some(940)), "0.9s");
    }

    #[test]
    fn control_overlay_is_a_clipping_child_on_top_of_the_surface() {
        let controls = control_style();
        assert_ne!(controls.0 & WS_CHILD.0, 0);
        // 与 surface 一样裁剪兄弟窗口，避免 video 重绘时盖掉控制条。
        assert_ne!(controls.0 & WS_CLIPSIBLINGS.0, 0);
        assert_eq!(controls.0 & WS_VISIBLE.0, 0, "starts hidden");
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
