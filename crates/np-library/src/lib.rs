//! 媒体库扫描、标签解析与检索。
//!
//! SQLite 只出现在这个 crate 里（ARCH-003 §1）。

mod db;
pub mod error;
mod metadata;
mod repo;
mod scan;
mod schema;

pub use db::Database;
pub use error::{LibraryError, Result};
pub use repo::{
    NewPlaylistItem, Playlist, PlaylistItem, PlaylistSummary, Stats, Track, validate_playlist_name,
};
pub use scan::{NoopObserver, ScanObserver, ScanReport, Scanner};
pub use schema::SCHEMA_VERSION;
