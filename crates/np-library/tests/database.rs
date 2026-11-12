//! 迁移与连接测试。

mod common;

use np_library::{Database, SCHEMA_VERSION};

#[test]
fn empty_db_migrates_to_current_version() {
    let dir = common::TempDir::new("migrate-empty");
    let db = Database::open(dir.db_path()).expect("open db");
    assert_eq!(SCHEMA_VERSION, 3);
    assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
    drop(db);
    let conn = rusqlite::Connection::open(dir.db_path()).expect("raw db");
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(tracks)")
        .expect("pragma")
        .query_map([], |row| row.get(1))
        .expect("columns")
        .collect::<Result<_, _>>()
        .expect("column names");
    assert!(!columns.iter().any(|column| column == "cover_key"));
}

#[test]
fn migration_is_idempotent() {
    let dir = common::TempDir::new("migrate-idempotent");
    let db = Database::open(dir.db_path()).expect("open db");
    assert_eq!(db.migrate().expect("migrate again"), 3);
    assert_eq!(db.migrate().expect("migrate third time"), 3);
    drop(db);

    let reopened = Database::open(dir.db_path()).expect("reopen db");
    assert_eq!(reopened.schema_version().expect("version"), 3);
}

#[test]
fn v1_database_gains_playlists_without_losing_old_data() {
    let dir = common::TempDir::new("migrate-v1-real-shape");
    let path = dir.db_path();
    {
        let conn = rusqlite::Connection::open(&path).expect("raw v1 db");
        conn.execute_batch(
            "CREATE TABLE scan_roots (
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 added_at INTEGER NOT NULL,
                 last_scan_at INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO scan_roots (path, added_at) VALUES ('/music', 100);
             PRAGMA user_version = 1;",
        )
        .expect("seed v1");
    }

    let db = Database::open(&path).expect("upgrade v1");
    assert_eq!(db.schema_version().expect("version"), 3);
    assert_eq!(
        db.list_roots().expect("old data"),
        vec!["/music".to_owned()]
    );
    assert!(db.list_playlists().expect("playlists empty").is_empty());
    assert!(
        path.with_extension("db.bak").exists(),
        "v1 upgrade must back up"
    );
}

#[test]
fn m5_bench_database_copy_migrates_without_losing_data() {
    let source =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/bench/library.db");
    if !source.is_file() {
        return;
    }
    let dir = common::TempDir::new("migrate-m5-real");
    let target = dir.db_path();
    std::fs::copy(&source, &target).expect("copy M5 db");
    let db = Database::open(&target).expect("migrate M5 copy");

    assert_eq!(db.schema_version().expect("version"), 3);
    assert!(
        db.stats().expect("stats").tracks > 0,
        "old tracks must survive"
    );
    assert!(db.list_playlists().expect("playlists").is_empty());
    assert!(target.with_extension("db.bak").exists(), "backup before v2");
}
