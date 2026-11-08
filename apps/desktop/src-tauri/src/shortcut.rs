//! 快捷键：`Ctrl+Shift+P` 切终端（全局）；`Ctrl+Q` 退出、`Ctrl+W` 收起到托盘（仅播放器聚焦时注册）。

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::windows;

/// 主修饰键：Windows/Linux 用 Ctrl，macOS 用 Cmd。
#[cfg(target_os = "macos")]
const PRIMARY: Modifiers = Modifiers::SUPER;
#[cfg(not(target_os = "macos"))]
const PRIMARY: Modifiers = Modifiers::CONTROL;

/// 全局快捷键插件；所有快捷键都在这里分发。
pub(crate) fn plugin() -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }

            if *shortcut == terminal_shortcut() {
                windows::toggle_terminal(app);
            } else if *shortcut == hide_player_shortcut() {
                windows::hide_player(app);
            } else if *shortcut == quit_shortcut() {
                println!("[app] quit from Ctrl+Q");
                app.exit(0);
            }
        })
        .build()
}

/// 启动时注册常驻的全局快捷键。
pub(crate) fn register(app: &AppHandle) {
    if let Err(error) = app.global_shortcut().register(terminal_shortcut()) {
        eprintln!("[shortcut] Ctrl+Shift+P register failed: {error}");
    }
}

/// 播放器获得/失去焦点时，注册或注销窗口级快捷键，避免抢占其他应用的 Ctrl+W / Ctrl+Q。
pub(crate) fn set_player_focused(app: &AppHandle, focused: bool) {
    for shortcut in [hide_player_shortcut(), quit_shortcut()] {
        let registered = app.global_shortcut().is_registered(shortcut);
        let result = match (focused, registered) {
            (true, false) => app.global_shortcut().register(shortcut),
            (false, true) => app.global_shortcut().unregister(shortcut),
            _ => continue,
        };

        if let Err(error) = result {
            eprintln!("[shortcut] window shortcut toggle failed: {error}");
        }
    }
}

fn terminal_shortcut() -> Shortcut {
    Shortcut::new(Some(PRIMARY | Modifiers::SHIFT), Code::KeyP)
}

fn hide_player_shortcut() -> Shortcut {
    Shortcut::new(Some(PRIMARY), Code::KeyW)
}

fn quit_shortcut() -> Shortcut {
    Shortcut::new(Some(PRIMARY), Code::KeyQ)
}
