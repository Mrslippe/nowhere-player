//! 宿主端口。执行器只通过这些 trait 与外部世界通信。

use np_core::Block;

/// 事件出口：Tauri 发 `app.emit`，TUI 写入内部状态。
pub trait EventSink: Send + Sync {
    /// 通用出口；事件名与负载格式必须保持原契约。
    fn emit(&self, event: &str, payload: serde_json::Value);

    /// `terminal:block` 契约：同一个 `id` 覆盖更新。
    fn terminal_block(&self, block: &Block) {
        self.emit(
            "terminal:block",
            serde_json::to_value(block).expect("Block must serialize"),
        );
    }

    /// `theme:changed` 契约：只带主题 id。
    fn theme_changed(&self, id: &str) {
        self.emit("theme:changed", serde_json::json!({ "id": id }));
    }
}
