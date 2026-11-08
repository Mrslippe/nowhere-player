//! 托盘图标与菜单：显示播放器 / 打开终端 / 开机自启 / 退出。

use tauri::menu::{CheckMenuItem, MenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};
use tauri_plugin_autostart::ManagerExt as AutostartExt;

use crate::windows;

const TRAY_ID: &str = "main";
const SHOW_PLAYER_ID: &str = "show-player";
const OPEN_TERMINAL_ID: &str = "open-terminal";
const AUTOSTART_ID: &str = "autostart";
const QUIT_ID: &str = "quit";

/// 建托盘：左键唤回播放器，右键出菜单，菜单文案见 T-0009 §6.3。
pub(crate) fn create(app: &AppHandle) -> tauri::Result<()> {
    let autostart = CheckMenuItem::with_id(
        app,
        AUTOSTART_ID,
        "开机自启",
        true,
        autostart_enabled(app),
        None::<&str>,
    )?;
    let menu = MenuBuilder::new(app)
        .text(SHOW_PLAYER_ID, "显示播放器")
        .text(OPEN_TERMINAL_ID, "打开终端 (Ctrl+Shift+P)")
        .separator()
        .item(&autostart)
        .text(QUIT_ID, "退出")
        .build()?;

    let autostart_for_events = autostart.clone();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Nowhere Player")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            SHOW_PLAYER_ID => windows::show_player(app),
            OPEN_TERMINAL_ID => windows::toggle_terminal(app),
            AUTOSTART_ID => toggle_autostart(app, &autostart_for_events),
            QUIT_ID => {
                println!("[tray] quit");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::show_player(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    println!("[tray] created (autostart = {})", autostart_enabled(app));
    Ok(())
}

fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 点击菜单项切换自启；勾选态一律用 `is_enabled()` 回读，不以点击为准。
fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<Wry>) {
    let manager = app.autolaunch();
    let before = manager.is_enabled().unwrap_or(false);
    let toggled = if before {
        manager.disable()
    } else {
        manager.enable()
    };

    match toggled.and_then(|()| manager.is_enabled()) {
        Ok(enabled) => {
            if let Err(error) = item.set_checked(enabled) {
                eprintln!("[tray] autostart check sync failed: {error}");
            }
            println!("[tray] autostart = {enabled} (was {before})");
        }
        Err(error) => eprintln!("[tray] autostart toggle failed: {error}"),
    }
}
