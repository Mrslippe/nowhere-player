//! Tauri 壳：装配两个窗口 + 全局快捷键，不放业务逻辑（见开发方案 §4.2）。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

const PLAYER_LABEL: &str = "player";
const TERMINAL_LABEL: &str = "terminal";
const APP_ENTRY: &str = "index.html";

/// 启动桌面壳。播放器窗启动即建；终端窗首次快捷键才建。
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed && *shortcut == toggle_shortcut() {
                        toggle_terminal(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            create_player_window(app.handle())?;
            if let Err(error) = app.global_shortcut().register(toggle_shortcut()) {
                eprintln!("[window] global shortcut register failed: {error}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Nowhere Player");
}

/// `Ctrl+Shift+P`（macOS 为 `Cmd+Shift+P`）。
fn toggle_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let primary = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let primary = Modifiers::CONTROL;

    Shortcut::new(Some(primary | Modifiers::SHIFT), Code::KeyP)
}

fn create_player_window(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, PLAYER_LABEL, WebviewUrl::App(APP_ENTRY.into()))
        .title("Nowhere Player")
        .inner_size(340.0, 120.0)
        .min_inner_size(300.0, 104.0)
        .decorations(false)
        .resizable(true)
        .maximizable(false)
        .center()
        .build()?;
    println!("[window] player created");
    Ok(())
}

fn toggle_terminal(app: &AppHandle) {
    let result = match app.get_webview_window(TERMINAL_LABEL) {
        Some(window) => toggle_visibility(&window),
        None => create_terminal_window(app).and_then(|window| show_terminal(&window)),
    };

    if let Err(error) = result {
        eprintln!("[window] terminal toggle failed: {error}");
    }
}

fn create_terminal_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
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
