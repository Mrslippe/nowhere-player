//! 与宿主无关的命令执行层。
//!
//! 这一层只依赖 `np-core` / `np-library` / `np-sources` 与端口 trait；
//! Tauri 与未来的 TUI 都只是它的薄适配器。

pub mod command;
pub mod paths;
pub mod ports;

pub use command::*;
pub use paths::Paths;
pub use ports::EventSink;
