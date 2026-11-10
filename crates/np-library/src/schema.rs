//! 表结构与迁移。
//!
//! 契约见 `docs/20-架构/数据模型.md`（ARCH-003）。迁移只增不改，
//! 版本号存在 `PRAGMA user_version` 里。

use rusqlite::Connection;

use crate::error::Result;

/// 当前 schema 版本。
pub const SCHEMA_VERSION: u32 = 2;

/// v1 的完整 DDL。
///
/// 所有时间列都是 Unix 毫秒（INTEGER）；`0` 表示未知（`last_seen_at` 除外）。
const V1: &str = r#"
CREATE TABLE IF NOT EXISTS artists (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE IF NOT EXISTS albums (
    id              INTEGER PRIMARY KEY,
    title           TEXT NOT NULL,
    album_artist_id INTEGER REFERENCES artists(id),
    year            INTEGER NOT NULL DEFAULT 0,
    cover_key       TEXT,
    UNIQUE(title COLLATE NOCASE, album_artist_id)
);
CREATE INDEX IF NOT EXISTS idx_albums_album_artist ON albums(album_artist_id);

CREATE TABLE IF NOT EXISTS tracks (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL,
    path         TEXT NOT NULL,
    source       TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    title        TEXT NOT NULL,
    album_id     INTEGER REFERENCES albums(id),
    album        TEXT NOT NULL DEFAULT '',
    artist       TEXT NOT NULL DEFAULT '',
    album_artist TEXT NOT NULL DEFAULT '',
    track_no     INTEGER NOT NULL DEFAULT 0,
    disc_no      INTEGER NOT NULL DEFAULT 0,
    year         INTEGER NOT NULL DEFAULT 0,
    duration_ms  INTEGER NOT NULL DEFAULT 0,
    sample_rate  INTEGER NOT NULL DEFAULT 0,
    channels     INTEGER NOT NULL DEFAULT 0,
    bitrate      INTEGER NOT NULL DEFAULT 0,
    codec        TEXT NOT NULL DEFAULT '',
    cover_key    TEXT,
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    mtime_ms     INTEGER NOT NULL DEFAULT 0,
    added_at     INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    UNIQUE(kind, path)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_tracks_source
    ON tracks(source, source_id) WHERE source_id <> '';
CREATE INDEX IF NOT EXISTS idx_tracks_title ON tracks(title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks(album_id);
CREATE INDEX IF NOT EXISTS idx_tracks_last_seen ON tracks(last_seen_at);

CREATE TABLE IF NOT EXISTS track_artists (
    track_id  INTEGER NOT NULL REFERENCES tracks(id),
    artist_id INTEGER NOT NULL REFERENCES artists(id),
    position  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (track_id, artist_id)
);
CREATE INDEX IF NOT EXISTS idx_ta_artist ON track_artists(artist_id);

CREATE TABLE IF NOT EXISTS scan_roots (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    added_at     INTEGER NOT NULL,
    last_scan_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS play_history (
    id        INTEGER PRIMARY KEY,
    track_id  INTEGER NOT NULL REFERENCES tracks(id),
    played_at INTEGER NOT NULL,
    completed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_history_played ON play_history(played_at DESC);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// v2：歌单表（ARCH-003 §9）。只新增表，不改 v1。
const V2: &str = r#"
CREATE TABLE IF NOT EXISTS playlists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_items (
    id          INTEGER PRIMARY KEY,
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    path        TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL DEFAULT '',
    source_id   TEXT NOT NULL DEFAULT '',
    title       TEXT NOT NULL DEFAULT '',
    artist      TEXT NOT NULL DEFAULT '',
    album       TEXT NOT NULL DEFAULT '',
    duration_ms INTEGER NOT NULL DEFAULT 0,
    added_at    INTEGER NOT NULL,
    UNIQUE(playlist_id, position)
);
CREATE INDEX IF NOT EXISTS idx_items_playlist
    ON playlist_items(playlist_id, position);
"#;

/// 读取当前 `user_version`。
pub(crate) fn version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// 从 `current` 升到 [`SCHEMA_VERSION`]，返回升级后的版本。
///
/// 目前只有 v1；未来版本在这里追加 `if current < 2 { ... }`。
pub(crate) fn apply(conn: &Connection, current: u32) -> Result<u32> {
    let mut current = current;
    if current < 1 {
        conn.execute_batch(&format!("BEGIN;\n{V1}\nPRAGMA user_version = 1;\nCOMMIT;"))?;
        current = 1;
    }
    if current < 2 {
        conn.execute_batch(&format!("BEGIN;\n{V2}\nPRAGMA user_version = 2;\nCOMMIT;"))?;
    }
    version(conn)
}
