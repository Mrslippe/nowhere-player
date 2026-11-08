//! 迁移与连接测试。

mod common;

use np_library::{Database, SCHEMA_VERSION};

#[test]
fn empty_db_migrates_to_version_one() {
    let dir = common::TempDir::new("migrate-empty");
    let db = Database::open(dir.db_path()).expect("open db");
    assert_eq!(SCHEMA_VERSION, 1);
    assert_eq!(db.schema_version().expect("version"), SCHEMA_VERSION);
}

#[test]
fn migration_is_idempotent() {
    let dir = common::TempDir::new("migrate-idempotent");
    let db = Database::open(dir.db_path()).expect("open db");
    assert_eq!(db.migrate().expect("migrate again"), 1);
    assert_eq!(db.migrate().expect("migrate third time"), 1);
    drop(db);

    let reopened = Database::open(dir.db_path()).expect("reopen db");
    assert_eq!(reopened.schema_version().expect("version"), 1);
}
