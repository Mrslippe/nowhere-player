//! 媒体库扫描、标签解析与检索。
//!
//! SQLite 只出现在这个 crate 里（ARCH-003 §1）。

mod db;
pub mod error;
mod schema;

pub use db::Database;
pub use error::{LibraryError, Result};
pub use schema::SCHEMA_VERSION;
