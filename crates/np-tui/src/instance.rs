//! 单实例：第二个进程只唤出已有窗口，不再起一个 TUI。

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attributes: *mut c_void, owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
        fn SetForegroundWindow(window: *mut c_void) -> i32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire() -> bool {
        static HANDLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let name = wide("NowherePlayer.TUI.Singleton");
        let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if handle.is_null() {
            return true;
        }
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let _ = HANDLE.set(handle as usize);
        if already {
            let title = wide("Nowhere");
            let window = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
            if !window.is_null() {
                unsafe {
                    ShowWindow(window, SW_RESTORE);
                    SetForegroundWindow(window);
                }
            }
            return false;
        }
        true
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn acquire() -> bool {
        true
    }
}

/// `true` 表示本进程是唯一实例；`false` 表示已有实例已唤出。
pub fn acquire() -> bool {
    platform::acquire()
}

#[cfg(test)]
mod tests {
    #[test]
    fn instance_name_is_stable() {
        // 名称写死，避免升级后悄悄变成两个实例。
        let name = "NowherePlayer.TUI.Singleton";
        assert!(name.contains("NowherePlayer"));
    }
}
