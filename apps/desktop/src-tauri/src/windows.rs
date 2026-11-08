//! 窗口创建、显隐，以及"关闭播放器窗 = 隐藏到托盘"。

use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};

use crate::shortcut;

pub(crate) const PLAYER_LABEL: &str = "player";
const TERMINAL_LABEL: &str = "terminal";
const APP_ENTRY: &str = "index.html";

/// 创建播放器小窗；启动即建，之后只隐藏、不销毁。
///
/// 外壳参数（T-0032）：
/// - `decorations(false)`：桌面挂件不需要系统标题栏。
/// - `resizable(false)`：产品不需要改宽高。Win32 层同时去掉 `WS_THICKFRAME`
///   （tao 的 `WM_NCHITTEST` 只在 `RESIZABLE` 时处理边缘拖拽），因此边缘
///   resize 热区与 Windows 11 给厚边框窗口画的 1px 边框/圆角一起消失。
/// - `shadow(false)`：无边框窗不要 DWM 阴影，避免阴影内缩带来的视觉描边。
///
/// 这里**没有**启用 `transparent`：透明窗有合成开销与点击穿透风险，先按
/// 「`resizable(false)` 已移除 THICKFRAME」验证；真机若仍有 1px 亮边再加。
pub(crate) fn create_player(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, PLAYER_LABEL, WebviewUrl::App(APP_ENTRY.into()))
        .title("Nowhere Player")
        .inner_size(340.0, 120.0)
        .decorations(false)
        .resizable(false)
        .shadow(false)
        .maximizable(false)
        .center()
        .build()?;
    println!("[window] player created");
    Ok(())
}

/// 显示并聚焦播放器窗（托盘、单实例、快捷键共用）。
pub(crate) fn show_player(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PLAYER_LABEL) else {
        if let Err(error) = create_player(app) {
            eprintln!("[window] player create failed: {error}");
        }
        return;
    };

    if let Err(error) = window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus())
    {
        eprintln!("[window] player show failed: {error}");
    }
}

/// 隐藏播放器窗，进程保留。
pub(crate) fn hide_player(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PLAYER_LABEL) else {
        return;
    };

    match window.hide() {
        Ok(()) => println!("[window] player hidden to tray"),
        Err(error) => eprintln!("[window] player hide failed: {error}"),
    }
}

/// 播放器关窗拦截为隐藏；焦点变化时切换窗口级快捷键的注册状态。
pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != PLAYER_LABEL {
        return;
    }

    match event {
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide_player(window.app_handle());
        }
        WindowEvent::Focused(focused) => {
            shortcut::set_player_focused(window.app_handle(), *focused);
        }
        _ => {}
    }
}

/// `Ctrl+Shift+P` 切换终端窗（首次按需创建）。
pub(crate) fn toggle_terminal(app: &AppHandle) {
    let result = match app.get_webview_window(TERMINAL_LABEL) {
        Some(window) => toggle_visibility(&window),
        None => create_terminal(app).and_then(|window| show_terminal(&window)),
    };

    if let Err(error) = result {
        eprintln!("[window] terminal toggle failed: {error}");
    }
}

fn create_terminal(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, TERMINAL_LABEL, WebviewUrl::App(APP_ENTRY.into()))
        .title("Nowhere Player — Terminal")
        .inner_size(880.0, 560.0)
        .decorations(true)
        .visible(false)
        .center()
        .build()
}

fn show_terminal(window: &WebviewWindow) -> tauri::Result<()> {
    window.show()?;
    window.set_focus()?;
    println!("[window] terminal shown");
    Ok(())
}

fn toggle_visibility(window: &WebviewWindow) -> tauri::Result<()> {
    if window.is_visible()? {
        window.hide()?;
        println!("[window] terminal hidden");
    } else {
        show_terminal(window)?;
    }
    Ok(())
}
