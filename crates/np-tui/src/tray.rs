//! Windows 托盘 + 全局热键。托盘线程独立，主循环只通过 mpsc 收动作。
#![cfg(windows)]

use std::ffi::c_void;
use std::sync::mpsc::Sender;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Toggle,
    Next,
    Prev,
    Quit,
    HotkeyFailed,
}

#[repr(C)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: [i32; 2],
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn GetConsoleWindow() -> *mut c_void;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
}

const WM_QUIT: u32 = 0x0012;
const SW_RESTORE: i32 = 9;

fn media_action(code: Code) -> Option<TrayAction> {
    match code {
        Code::MediaPlayPause => Some(TrayAction::Toggle),
        Code::MediaTrackNext => Some(TrayAction::Next),
        Code::MediaTrackPrevious => Some(TrayAction::Prev),
        _ => None,
    }
}

pub fn spawn(sender: Sender<TrayAction>) {
    std::thread::spawn(move || run(sender));
}

fn run(sender: Sender<TrayAction>) {
    let menu = Menu::new();
    let show = MenuItem::new("显示窗口", true, None);
    let toggle = MenuItem::new("播放 / 暂停", true, None);
    let next = MenuItem::new("下一首", true, None);
    let quit = MenuItem::new("退出", true, None);
    if menu.append_items(&[&show, &toggle, &next, &quit]).is_err() {
        let _ = sender.send(TrayAction::HotkeyFailed);
        return;
    }

    let icon = icon_rgba();
    let Ok(icon) = Icon::from_rgba(icon, 16, 16) else {
        return;
    };
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Nowhere Player")
        .with_icon(icon)
        .build();
    if tray.is_err() {
        return;
    }
    let _tray = tray;

    let manager = GlobalHotKeyManager::new().ok();
    let mut hotkeys = Vec::new();
    if let Some(manager) = &manager {
        for (code, modifiers, action) in [
            (
                Code::Space,
                Some(Modifiers::CONTROL | Modifiers::ALT),
                TrayAction::Toggle,
            ),
            (
                Code::ArrowLeft,
                Some(Modifiers::CONTROL | Modifiers::ALT),
                TrayAction::Prev,
            ),
            (
                Code::ArrowRight,
                Some(Modifiers::CONTROL | Modifiers::ALT),
                TrayAction::Next,
            ),
            (
                Code::MediaPlayPause,
                None,
                media_action(Code::MediaPlayPause).unwrap_or(TrayAction::Toggle),
            ),
            (
                Code::MediaTrackNext,
                None,
                media_action(Code::MediaTrackNext).unwrap_or(TrayAction::Next),
            ),
            (
                Code::MediaTrackPrevious,
                None,
                media_action(Code::MediaTrackPrevious).unwrap_or(TrayAction::Prev),
            ),
        ] {
            let hotkey = HotKey::new(modifiers, code);
            if manager.register(hotkey).is_err() {
                let _ = sender.send(TrayAction::HotkeyFailed);
            } else {
                hotkeys.push((hotkey.id(), action));
            }
        }
    }

    // 托盘/热键要求同线程 Win32 消息泵；所有 UI 交互只回传枚举。
    unsafe {
        let mut msg = Msg {
            hwnd: std::ptr::null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            pt: [0, 0],
        };
        loop {
            let result = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if result <= 0 || msg.message == WM_QUIT {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);

            while let Ok(event) = MenuEvent::receiver().try_recv() {
                let id = event.id().as_ref();
                let action = if id == show.id().as_ref() {
                    Some(TrayAction::Show)
                } else if id == toggle.id().as_ref() {
                    Some(TrayAction::Toggle)
                } else if id == next.id().as_ref() {
                    Some(TrayAction::Next)
                } else if id == quit.id().as_ref() {
                    Some(TrayAction::Quit)
                } else {
                    None
                };
                if let Some(action) = action {
                    let _ = sender.send(action);
                }
            }
            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state() != HotKeyState::Pressed {
                    continue;
                }
                if let Some((_, action)) = hotkeys.iter().find(|(id, _)| *id == event.id()) {
                    let _ = sender.send(*action);
                }
            }
        }
    }
}

pub fn focus_console() {
    unsafe {
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

fn icon_rgba() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let inside = (3..13).contains(&x) && (3..13).contains(&y);
            let note = (7..9).contains(&x) && (5..11).contains(&y);
            let color = if inside {
                if note {
                    [13u8, 15, 18, 255]
                } else {
                    [255u8, 180, 84, 255]
                }
            } else {
                [0, 0, 0, 0]
            };
            pixels.extend_from_slice(&color);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_keys_map_to_queue_and_toggle() {
        assert_eq!(media_action(Code::MediaPlayPause), Some(TrayAction::Toggle));
        assert_eq!(media_action(Code::MediaTrackNext), Some(TrayAction::Next));
        assert_eq!(
            media_action(Code::MediaTrackPrevious),
            Some(TrayAction::Prev)
        );
    }
}
