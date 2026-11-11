//! 宿主端口。执行器只通过这些 trait 与外部世界通信。

use np_core::Block;

/// 事件出口：Tauri 发 `app.emit`，TUI 写入内部状态。
pub trait EventSink: Send + Sync {
    /// `terminal:block` 契约：同一个 `id` 覆盖更新。
    fn terminal_block(&self, block: &Block);
    /// `theme:changed` 契约：只带主题 id。
    fn theme_changed(&self, id: &str);
}
