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
/// 外壳参数（T-0032 / T-0045 阶段 2 外投影 A）：
/// - `inner_size(380×160)`：卡片 340×120 居中，四周留 20px 透明边距，
///   CSS 单层外投影才能落在壁纸上，而不是被 340×120 窗口裁掉。
/// - `transparent(true)`：20px 边距必须透出壁纸；卡片/阴影由 CSS 画。
/// - `decorations(false)`：桌面挂件不需要系统标题栏。
/// - `shadow(false)`：**P2 白边的正解**。Tauri 文档明确写着：无边框窗
///   `shadow: true` 会画 1px 白边，Windows 11 上还会带圆角；`false` 才去掉。
///   外投影只用 CSS，绝不开原生 shadow。
/// - `resizable(false)`：产品不需要改宽高（P3）。Win32 层同时去掉
///   `WS_THICKFRAME`，tao 也只在 `RESIZABLE` 时于 `WM_NCHITTEST` 里给边缘
///   resize 热区；两者一起消失。
/// - 拖拽区只挂在 340×120 卡片上（`data-tauri-drag-region`），20px 透明
///   边距不可拖，避免用户拖到"空气"。
pub(crate) fn create_player(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, PLAYER_LABEL, WebviewUrl::App(APP_ENTRY.into()))
        .title("Nowhere Player")
        .inner_size(380.0, 160.0)
        .transparent(true)
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

/// 播放器关窗拦截为隐藏；终端窗的关闭同样收起到托盘。
pub(crate) fn on_window_event(window: &Window, event: &WindowEvent) {
    match window.label() {
        PLAYER_LABEL => match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_player(window.app_handle());
            }
            WindowEvent::Focused(focused) => {
                shortcut::set_player_focused(window.app_handle(), *focused);
            }
            _ => {}
        },
        // T-0032：终端窗没有系统标题栏，Alt+F4 / 系统关闭也按「隐藏到托盘」处理，
        // 保留终端历史；自绘关闭按钮走前端 window.hide()。
        TERMINAL_LABEL => {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    eprintln!("[window] terminal hide failed: {error}");
                }
            }
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
        // T-0032 §6.4：系统标题栏太突兀，改由页内顶栏自绘（34px）。
        // shadow(false) 同样是必须的：无边框窗开阴影会带 1px 白边（同播放器窗）。
        .decorations(false)
        .shadow(false)
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
