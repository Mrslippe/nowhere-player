//! 落库与查询。
//!
//! 归一化顺序固定为 `artists` → `albums` → `tracks` → `track_artists`
//! （ARCH-003 §4）。写入只由扫描器的单线程 writer 调用。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::metadata::ParsedTrack;

const TRACK_COLUMNS: &str = "id, kind, path, source, source_id, title, album_id, album, artist, \
     album_artist, track_no, disc_no, year, duration_ms, sample_rate, channels, bitrate, codec, \
     cover_key, size_bytes, mtime_ms, added_at, last_seen_at";

/// 一首曲目。
///
/// 字段与 `tracks` 表一一对应；`album` / `artist` / `album_artist` 是冗余文本列，
/// 供展示与 `LIKE` 检索，id 才是权威（ARCH-003 §1）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub path: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub album_id: Option<i64>,
    pub album: String,
    pub artist: String,
    pub album_artist: String,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: u32,
    pub duration_ms: i64,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: u32,
    pub codec: String,
    pub cover_key: Option<String>,
    pub size_bytes: i64,
    pub mtime_ms: i64,
    pub added_at: i64,
    pub last_seen_at: i64,
}

/// `stats()` 的返回值。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
    pub total_duration_ms: u64,
    /// 最近一次扫描的起始时间；`0` 表示从未扫描。
    pub last_scan_at: i64,
}

/// 写入一首曲目：artists → albums → tracks → track_artists。
pub(crate) fn upsert_track(conn: &Connection, track: &ParsedTrack, now: i64) -> Result<()> {
    let artist_names = split_artists(&track.artist);
    let album_artist = if track.album_artist.trim().is_empty() {
        artist_names.first().copied().unwrap_or("").to_string()
    } else {
        track.album_artist.trim().to_string()
    };
    let album_artist_id = (!album_artist.is_empty())
        .then(|| get_or_create_artist(conn, &album_artist))
        .transpose()?;

    let album_id = if track.album.trim().is_empty() {
        None
    } else {
        Some(get_or_create_album(
            conn,
            track.album.trim(),
            album_artist_id,
            track.year,
            track.cover_key.as_deref(),
        )?)
    };

    let track_id: i64 = conn.query_row(
        "INSERT INTO tracks (
             kind, path, source, source_id, title, album_id, album, artist, album_artist,
             track_no, disc_no, year, duration_ms, sample_rate, channels, bitrate, codec,
             cover_key, size_bytes, mtime_ms, added_at, last_seen_at
         ) VALUES (
             'local', ?1, 'local', '', ?2, ?3, ?4, ?5, ?6,
             ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
             ?15, ?16, ?17, ?18, ?18
         )
         ON CONFLICT(kind, path) DO UPDATE SET
             title = excluded.title,
             album_id = excluded.album_id,
             album = excluded.album,
             artist = excluded.artist,
             album_artist = excluded.album_artist,
             track_no = excluded.track_no,
             disc_no = excluded.disc_no,
             year = excluded.year,
             duration_ms = excluded.duration_ms,
             sample_rate = excluded.sample_rate,
             channels = excluded.channels,
             bitrate = excluded.bitrate,
             codec = excluded.codec,
             cover_key = COALESCE(excluded.cover_key, tracks.cover_key),
             size_bytes = excluded.size_bytes,
             mtime_ms = excluded.mtime_ms,
             last_seen_at = excluded.last_seen_at
         RETURNING id",
        params![
            track.path.to_string_lossy(),
            track.title,
            album_id,
            track.album,
            track.artist,
            album_artist,
            track.track_no,
            track.disc_no,
            track.year,
            track.duration_ms,
            track.sample_rate,
            track.channels,
            track.bitrate,
            track.codec,
            track.cover_key,
            track.size_bytes,
            track.mtime_ms,
            now,
        ],
        |row| row.get(0),
    )?;

    conn.execute("DELETE FROM track_artists WHERE track_id = ?1", [track_id])?;
    let mut linked: Vec<i64> = Vec::with_capacity(artist_names.len());
    for (position, name) in artist_names.iter().enumerate() {
        let artist_id = get_or_create_artist(conn, name)?;
        if linked.contains(&artist_id) {
            continue;
        }
        linked.push(artist_id);
        conn.execute(
            "INSERT INTO track_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)",
            params![
                track_id,
                artist_id,
                i64::try_from(position).unwrap_or(i64::MAX)
            ],
        )?;
    }

    Ok(())
}

/// 增量扫描命中：只刷新 `last_seen_at`。
pub(crate) fn mark_seen(conn: &Connection, path: &Path, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET last_seen_at = ?2 WHERE kind = 'local' AND path = ?1",
        params![path.to_string_lossy(), now],
    )?;
    Ok(())
}

/// 扫描开始前缓存 `(path, size, mtime)`，供解析线程判断是否增量命中。
pub(crate) fn load_index(conn: &Connection) -> Result<HashMap<String, (i64, i64)>> {
    let mut statement =
        conn.prepare("SELECT path, size_bytes, mtime_ms FROM tracks WHERE kind = 'local'")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?;
    let mut index = HashMap::new();
    for row in rows {
        let (path, fingerprint) = row?;
        index.insert(path, fingerprint);
    }
    Ok(index)
}

/// 记录/刷新一个扫描根。
pub(crate) fn touch_root(conn: &Connection, root: &Path, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO scan_roots (path, added_at, last_scan_at) VALUES (?1, ?2, ?2)
         ON CONFLICT(path) DO UPDATE SET last_scan_at = excluded.last_scan_at",
        params![root.to_string_lossy(), now],
    )?;
    Ok(())
}

/// 登记一个扫描根；已存在时**不覆盖** `added_at` / `last_scan_at`，返回 `false`。
///
/// 幂等由 `scan_roots.path` 的 UNIQUE 约束保证；调用方用返回值区分
/// “新添加”与“重复添加”。
pub(crate) fn add_root(conn: &Connection, root: &Path, now: i64) -> Result<bool> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO scan_roots (path, added_at, last_scan_at) VALUES (?1, ?2, 0)",
        params![root.to_string_lossy(), now],
    )?;
    Ok(inserted > 0)
}

/// 全部扫描根，按添加顺序（`id`）返回绝对路径。
pub(crate) fn list_roots(conn: &Connection) -> Result<Vec<String>> {
    let mut statement = conn.prepare("SELECT path FROM scan_roots ORDER BY id")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 本轮扫描未见到的旧记录数（只统计，不删除）。
pub(crate) fn count_missing(conn: &Connection, scan_started_at: i64) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks WHERE kind = 'local' AND last_seen_at < ?1",
        [scan_started_at],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// 在 `title` / `artist` / `album` 上做不区分大小写的 `LIKE`，按标题排序。
pub(crate) fn search(conn: &Connection, keyword: &str, limit: usize) -> Result<Vec<Track>> {
    let pattern = like_pattern(keyword);
    let mut statement = conn.prepare(&format!(
        "SELECT {TRACK_COLUMNS} FROM tracks
         WHERE title LIKE ?1 ESCAPE '\\'
            OR artist LIKE ?1 ESCAPE '\\'
            OR album LIKE ?1 ESCAPE '\\'
         ORDER BY title COLLATE NOCASE, id
         LIMIT ?2"
    ))?;
    let rows = statement.query_map(params![pattern, sql_limit(limit)], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 最近入库的曲目（`added_at DESC`）。
pub(crate) fn list_recent(conn: &Connection, limit: usize) -> Result<Vec<Track>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {TRACK_COLUMNS} FROM tracks ORDER BY added_at DESC, id DESC LIMIT ?1"
    ))?;
    let rows = statement.query_map([sql_limit(limit)], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 一张专辑的曲目，按 `disc_no, track_no` 排序。
pub(crate) fn tracks_by_album(conn: &Connection, album_id: i64) -> Result<Vec<Track>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {TRACK_COLUMNS} FROM tracks WHERE album_id = ?1
         ORDER BY disc_no, track_no, title COLLATE NOCASE, id"
    ))?;
    let rows = statement.query_map([album_id], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 一位艺人的曲目，经 `track_artists` 连接，按专辑/碟/轨排序。
pub(crate) fn tracks_by_artist(conn: &Connection, artist_id: i64) -> Result<Vec<Track>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {TRACK_COLUMNS} FROM tracks
         JOIN track_artists ON track_artists.track_id = tracks.id
         WHERE track_artists.artist_id = ?1
         ORDER BY album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE, id"
    ))?;
    let rows = statement.query_map([artist_id], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 曲目数 / 专辑数 / 艺人数 / 总时长 / 最近扫描。
pub(crate) fn stats(conn: &Connection) -> Result<Stats> {
    let (tracks, total_duration_ms): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let albums: i64 = conn.query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))?;
    let artists: i64 = conn.query_row("SELECT COUNT(*) FROM artists", [], |row| row.get(0))?;
    let last_scan_at: i64 = conn.query_row(
        "SELECT COALESCE(MAX(last_scan_at), 0) FROM scan_roots",
        [],
        |row| row.get(0),
    )?;
    Ok(Stats {
        tracks: u64::try_from(tracks).unwrap_or(u64::MAX),
        albums: u64::try_from(albums).unwrap_or(u64::MAX),
        artists: u64::try_from(artists).unwrap_or(u64::MAX),
        total_duration_ms: u64::try_from(total_duration_ms).unwrap_or(u64::MAX),
        last_scan_at,
    })
}

fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        kind: row.get(1)?,
        path: row.get(2)?,
        source: row.get(3)?,
        source_id: row.get(4)?,
        title: row.get(5)?,
        album_id: row.get(6)?,
        album: row.get(7)?,
        artist: row.get(8)?,
        album_artist: row.get(9)?,
        track_no: row.get(10)?,
        disc_no: row.get(11)?,
        year: row.get(12)?,
        duration_ms: row.get(13)?,
        sample_rate: row.get(14)?,
        channels: row.get(15)?,
        bitrate: row.get(16)?,
        codec: row.get(17)?,
        cover_key: row.get(18)?,
        size_bytes: row.get(19)?,
        mtime_ms: row.get(20)?,
        added_at: row.get(21)?,
        last_seen_at: row.get(22)?,
    })
}

fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// 把关键字包成 `LIKE` 模式，转义 `\` / `%` / `_`。
fn like_pattern(keyword: &str) -> String {
    let mut pattern = String::with_capacity(keyword.len() + 2);
    pattern.push('%');
    for character in keyword.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

/// 多艺人按 `,` / `;` 切分，去空白与空项。
fn split_artists(value: &str) -> Vec<&str> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}

/// 按 name 去重（`COLLATE NOCASE`）取 id，没有就建。
fn get_or_create_artist(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute("INSERT OR IGNORE INTO artists (name) VALUES (?1)", [name])?;
    Ok(
        conn.query_row("SELECT id FROM artists WHERE name = ?1", [name], |row| {
            row.get(0)
        })?,
    )
}

/// 按 `(title, album_artist_id)` 取 id，没有就建；已有专辑只补空的 year / cover。
fn get_or_create_album(
    conn: &Connection,
    title: &str,
    album_artist_id: Option<i64>,
    year: u32,
    cover_key: Option<&str>,
) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM albums WHERE title = ?1 AND album_artist_id IS ?2",
            params![title, album_artist_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE albums SET
                 cover_key = COALESCE(cover_key, ?2),
                 year = CASE WHEN year = 0 THEN ?3 ELSE year END
             WHERE id = ?1",
            params![id, cover_key, year],
        )?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO albums (title, album_artist_id, year, cover_key) VALUES (?1, ?2, ?3, ?4)",
        params![title, album_artist_id, year, cover_key],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::db::Database;

    fn sample(path: &str) -> ParsedTrack {
        ParsedTrack {
            path: PathBuf::from(path),
            title: "Song".to_string(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            track_no: 0,
            disc_no: 0,
            year: 0,
            duration_ms: 0,
            sample_rate: 8_000,
            channels: 1,
            bitrate: 128,
            codec: "wav".to_string(),
            cover_key: None,
            cover: None,
            size_bytes: 100,
            mtime_ms: 1,
        }
    }

    fn insert(db: &Database, track: &ParsedTrack, now: i64) {
        upsert_track(db.conn(), track, now).expect("upsert track");
    }

    fn count(db: &Database, sql: &str) -> i64 {
        db.conn()
            .query_row(sql, [], |row| row.get(0))
            .expect("count query")
    }

    #[test]
    fn same_album_creates_single_album_row() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/a.wav");
        first.title = "A".to_string();
        first.artist = "Artist".to_string();
        first.album = "Album".to_string();
        let mut second = sample("/music/b.wav");
        second.title = "B".to_string();
        second.artist = "Artist".to_string();
        second.album = "Album".to_string();

        insert(&db, &first, 100);
        insert(&db, &second, 100);

        assert_eq!(count(&db, "SELECT COUNT(*) FROM albums"), 1);
        let tracks = search(db.conn(), "", 10).expect("search all");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].album_id, tracks[1].album_id);
        assert_eq!(tracks[0].album, "Album");
    }

    #[test]
    fn artist_dedup_ignores_case() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/a.wav");
        first.artist = "Radiohead".to_string();
        let mut second = sample("/music/b.wav");
        second.artist = "radiohead".to_string();

        insert(&db, &first, 100);
        insert(&db, &second, 100);

        assert_eq!(count(&db, "SELECT COUNT(*) FROM artists"), 1);
        let linked: Vec<i64> = {
            let mut statement = db
                .conn()
                .prepare("SELECT artist_id FROM track_artists ORDER BY track_id")
                .expect("prepare");
            statement
                .query_map([], |row| row.get(0))
                .expect("query")
                .collect::<rusqlite::Result<Vec<_>>>()
                .expect("collect")
        };
        assert_eq!(linked.len(), 2);
        assert_eq!(linked[0], linked[1]);
    }

    #[test]
    fn multiple_artists_get_positions_and_links() {
        let db = Database::open_in_memory().expect("memory db");
        let mut track = sample("/music/a.wav");
        track.artist = "Alice; Bob".to_string();

        insert(&db, &track, 100);

        assert_eq!(count(&db, "SELECT COUNT(*) FROM artists"), 2);
        let mut statement = db
            .conn()
            .prepare(
                "SELECT artists.name, track_artists.position
                 FROM track_artists JOIN artists ON artists.id = track_artists.artist_id
                 ORDER BY track_artists.position",
            )
            .expect("prepare");
        let rows: Vec<(String, i64)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect");
        assert_eq!(rows, vec![("Alice".to_string(), 0), ("Bob".to_string(), 1)]);
        let track_row = search(db.conn(), "Song", 1).expect("search");
        assert_eq!(track_row[0].artist, "Alice; Bob");
    }

    #[test]
    fn album_artist_groups_tracks_from_different_artists() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/a.wav");
        first.artist = "Alice".to_string();
        first.album = "Compilation".to_string();
        first.album_artist = "Various Artists".to_string();
        let mut second = sample("/music/b.wav");
        second.artist = "Bob".to_string();
        second.album = "Compilation".to_string();
        second.album_artist = "Various Artists".to_string();

        insert(&db, &first, 100);
        insert(&db, &second, 100);

        assert_eq!(count(&db, "SELECT COUNT(*) FROM albums"), 1);
        let tracks = search(db.conn(), "Song", 10).expect("search");
        assert_eq!(tracks[0].album_id, tracks[1].album_id);
    }

    #[test]
    fn search_matches_title_artist_and_album() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/01.wav");
        first.title = "Kid A".to_string();
        first.artist = "Radiohead".to_string();
        first.album = "OK Computer".to_string();
        let mut second = sample("/music/02.wav");
        second.title = "Paranoid Android".to_string();
        second.artist = "Radiohead".to_string();
        second.album = "OK Computer".to_string();

        insert(&db, &first, 100);
        insert(&db, &second, 200);

        assert_eq!(search(db.conn(), "kid", 10).expect("title").len(), 1);
        assert_eq!(search(db.conn(), "radio", 10).expect("artist").len(), 2);
        assert_eq!(search(db.conn(), "computer", 10).expect("album").len(), 2);
        assert_eq!(search(db.conn(), "RADIOHEAD", 10).expect("case").len(), 2);
        assert_eq!(
            search(db.conn(), "kid", 10).expect("limit")[0].title,
            "Kid A"
        );
    }

    #[test]
    fn search_escapes_like_wildcards() {
        let db = Database::open_in_memory().expect("memory db");
        let mut track = sample("/music/a.wav");
        track.title = "100% Pure".to_string();
        insert(&db, &track, 100);

        assert_eq!(search(db.conn(), "100%", 10).expect("percent").len(), 1);
        assert_eq!(search(db.conn(), "%", 10).expect("percent only").len(), 1);
        assert_eq!(search(db.conn(), "not there", 10).expect("miss").len(), 0);
    }

    #[test]
    fn list_recent_orders_by_added_at() {
        let db = Database::open_in_memory().expect("memory db");
        let mut older = sample("/music/older.wav");
        older.title = "Older".to_string();
        let mut newer = sample("/music/newer.wav");
        newer.title = "Newer".to_string();

        insert(&db, &older, 100);
        insert(&db, &newer, 200);

        let tracks = list_recent(db.conn(), 10).expect("recent");
        assert_eq!(tracks[0].title, "Newer");
        assert_eq!(tracks[1].title, "Older");
    }

    #[test]
    fn tracks_by_album_orders_by_disc_and_track() {
        let db = Database::open_in_memory().expect("memory db");
        let mut second = sample("/music/02.wav");
        second.album = "Album".to_string();
        second.disc_no = 1;
        second.track_no = 2;
        let mut first = sample("/music/01.wav");
        first.album = "Album".to_string();
        first.disc_no = 1;
        first.track_no = 1;
        let mut other_disc = sample("/music/03.wav");
        other_disc.album = "Album".to_string();
        other_disc.disc_no = 2;
        other_disc.track_no = 1;

        insert(&db, &second, 100);
        insert(&db, &first, 100);
        insert(&db, &other_disc, 100);

        let album_id = search(db.conn(), "", 10).expect("search")[0]
            .album_id
            .expect("album id");
        let tracks = tracks_by_album(db.conn(), album_id).expect("by album");
        assert_eq!(
            tracks
                .iter()
                .map(|track| (track.disc_no, track.track_no))
                .collect::<Vec<_>>(),
            vec![(1, 1), (1, 2), (2, 1)]
        );
    }

    #[test]
    fn stats_counts_rows_and_duration() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/a.wav");
        first.artist = "Artist".to_string();
        first.album = "Album".to_string();
        first.duration_ms = 1_500;
        let mut second = sample("/music/b.wav");
        second.artist = "Artist".to_string();
        second.album = "Album".to_string();
        second.duration_ms = 2_500;

        insert(&db, &first, 100);
        insert(&db, &second, 100);

        let stats = stats(db.conn()).expect("stats");
        assert_eq!(stats.tracks, 2);
        assert_eq!(stats.albums, 1);
        assert_eq!(stats.artists, 1);
        assert_eq!(stats.total_duration_ms, 4_000);
        assert_eq!(stats.last_scan_at, 0);
    }

    #[test]
    fn album_keeps_first_cover_and_backfills_year() {
        let db = Database::open_in_memory().expect("memory db");
        let mut first = sample("/music/a.wav");
        first.album = "Album".to_string();
        first.cover_key = Some("aaaaaaaaaaaaaaaa".to_string());
        first.year = 0;
        let mut second = sample("/music/b.wav");
        second.album = "Album".to_string();
        second.cover_key = Some("bbbbbbbbbbbbbbbb".to_string());
        second.year = 1999;

        insert(&db, &first, 100);
        insert(&db, &second, 100);

        let (cover, year): (Option<String>, u32) = db
            .conn()
            .query_row("SELECT cover_key, year FROM albums", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("album row");
        assert_eq!(cover.as_deref(), Some("aaaaaaaaaaaaaaaa"));
        assert_eq!(year, 1999);
    }

    #[test]
    fn reupsert_same_path_updates_without_duplicates() {
        let db = Database::open_in_memory().expect("memory db");
        let mut track = sample("/music/a.wav");
        track.title = "First".to_string();
        track.artist = "Artist".to_string();
        insert(&db, &track, 100);

        track.title = "Second".to_string();
        insert(&db, &track, 200);

        assert_eq!(count(&db, "SELECT COUNT(*) FROM tracks"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM artists"), 1);
        assert_eq!(count(&db, "SELECT COUNT(*) FROM track_artists"), 1);
        let tracks = list_recent(db.conn(), 10).expect("recent");
        assert_eq!(tracks[0].title, "Second");
        assert_eq!(tracks[0].last_seen_at, 200);
        assert_eq!(tracks[0].added_at, 100);
    }

    #[test]
    fn add_root_is_idempotent_and_reports_duplicates() {
        let db = Database::open_in_memory().expect("memory db");
        let root = std::path::absolute("music-library").expect("absolute");

        assert!(db.add_root(&root).expect("first add"), "first add inserts");
        assert!(
            !db.add_root(&root).expect("second add"),
            "second add must report the duplicate"
        );

        assert_eq!(count(&db, "SELECT COUNT(*) FROM scan_roots"), 1);
        assert_eq!(
            db.list_roots().expect("roots"),
            vec![root.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn list_roots_keeps_insertion_order() {
        let db = Database::open_in_memory().expect("memory db");
        let first = std::path::absolute("music-first").expect("absolute");
        let second = std::path::absolute("music-second").expect("absolute");

        db.add_root(&second).expect("second root");
        db.add_root(&first).expect("first root");

        assert_eq!(
            db.list_roots().expect("roots"),
            vec![
                second.to_string_lossy().into_owned(),
                first.to_string_lossy().into_owned()
            ]
        );
    }

    #[test]
    fn tracks_by_artist_joins_track_artists() {
        let db = Database::open_in_memory().expect("memory db");
        let mut alice = sample("/music/alice.wav");
        alice.title = "Alice Song".to_string();
        alice.artist = "Alice".to_string();
        let mut bob = sample("/music/bob.wav");
        bob.title = "Bob Song".to_string();
        bob.artist = "Bob".to_string();
        insert(&db, &alice, 100);
        insert(&db, &bob, 100);

        let artist_id: i64 = db
            .conn()
            .query_row("SELECT id FROM artists WHERE name = 'Alice'", [], |row| {
                row.get(0)
            })
            .expect("artist id");
        let tracks = db.tracks_by_artist(artist_id).expect("by artist");

        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "Alice Song");
        assert!(db.tracks_by_artist(999).expect("unknown artist").is_empty());
    }
}
