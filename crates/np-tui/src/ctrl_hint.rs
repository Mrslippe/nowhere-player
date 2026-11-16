//! Windows 下检测 Ctrl 是否按住。
//!
//! crossterm 在 Windows 上不会把单独的 Ctrl 作为 KeyCode 发出来（VK_CONTROL 被过滤），
//! 所以这里用 Win32 的异步键状态只读检测；不注册钩子、不吞按键。

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(v_key: i32) -> i16;
}

#[cfg(windows)]
pub(crate) fn control_down() -> bool {
    const VK_CONTROL: i32 = 0x11;
    // SAFETY: GetAsyncKeyState 只读当前线程桌面上的键状态，没有指针参数；
    // 返回值的最高位表示当前是否按下。
    unsafe { GetAsyncKeyState(VK_CONTROL) & i16::MIN != 0 }
}

#[cfg(not(windows))]
pub(crate) fn control_down() -> bool {
    false
}
