//! T-0044 歌单仓储：CRUD、重名、position 重排。

mod common;

use np_library::{
    Database, LibraryError, NewPlaylistItem, PlaylistItem, PlaylistSummary, validate_playlist_name,
};

fn local_item(title: &str) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "local".to_owned(),
        path: format!("/music/{title}.flac"),
        source: String::new(),
        source_id: String::new(),
        title: title.to_owned(),
        artist: "本地艺人".to_owned(),
        album: "本地专辑".to_owned(),
        duration_ms: 65_000,
    }
}

fn remote_item(title: &str) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "remote".to_owned(),
        path: String::new(),
        source: "bilibili".to_owned(),
        source_id: format!("BV{title}"),
        title: title.to_owned(),
        artist: "远程艺人".to_owned(),
        album: String::new(),
        duration_ms: 236_000,
    }
}

fn names(items: &[PlaylistItem]) -> Vec<(&str, i64)> {
    items
        .iter()
        .map(|item| (item.title.as_str(), item.position))
        .collect()
}

#[test]
fn create_and_list_playlists_with_counts() {
    let dir = common::TempDir::new("pl-create");
    let db = Database::open(dir.db_path()).expect("db");
    let a = db.create_playlist("周杰伦 情歌").expect("create a");
    db.create_playlist("Road Trip").expect("create b");
    db.add_playlist_item(a, &local_item("晴天")).expect("add a");
    db.add_playlist_item(a, &local_item("稻香")).expect("add b");

    let list = db.list_playlists().expect("list");
    assert_eq!(list.len(), 2);
    let summary: &PlaylistSummary = list
        .iter()
        .find(|playlist| playlist.name == "周杰伦 情歌")
        .expect("Chinese name");
    assert_eq!(summary.item_count, 2);
}

#[test]
fn duplicate_name_is_case_insensitive() {
    let dir = common::TempDir::new("pl-duplicate");
    let db = Database::open(dir.db_path()).expect("db");
    db.create_playlist("Mix").expect("first");
    let error = db.create_playlist("mix").expect_err("duplicate");
    assert!(matches!(error, LibraryError::Playlist(message) if message.contains("已存在")));
}

#[test]
fn invalid_names_are_rejected() {
    for name in ["", "   ", "a/b", "a.b", "x".repeat(49).as_str()] {
        assert!(
            validate_playlist_name(name).is_err(),
            "should reject {name:?}"
        );
    }
    assert_eq!(
        validate_playlist_name("  周杰伦  - 情歌_1  ").expect("valid"),
        "周杰伦  - 情歌_1"
    );
}

#[test]
fn rename_updates_name_and_preserves_items() {
    let dir = common::TempDir::new("pl-rename");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("旧名").expect("create");
    db.add_playlist_item(id, &local_item("歌")).expect("add");
    db.rename_playlist(id, "新名").expect("rename");

    let playlist = db
        .playlist_by_name("新名")
        .expect("by name")
        .expect("found");
    assert_eq!(playlist.id, id);
    assert_eq!(db.get_playlist(id).expect("get").1.len(), 1);
}

#[test]
fn rename_to_existing_name_fails() {
    let dir = common::TempDir::new("pl-rename-dup");
    let db = Database::open(dir.db_path()).expect("db");
    let a = db.create_playlist("A").expect("A");
    db.create_playlist("B").expect("B");
    let error = db.rename_playlist(a, "b").expect_err("duplicate");
    assert!(matches!(error, LibraryError::Playlist(_)));
}

#[test]
fn delete_removes_items_too() {
    let dir = common::TempDir::new("pl-delete");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    db.add_playlist_item(id, &local_item("歌")).expect("add");
    db.delete_playlist(id).expect("delete");
    assert!(db.list_playlists().expect("list").is_empty());
}

#[test]
fn add_assigns_contiguous_positions_and_preserves_remote_metadata() {
    let dir = common::TempDir::new("pl-add");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    let first = db
        .add_playlist_item(id, &local_item("本地"))
        .expect("local");
    let second = db
        .add_playlist_item(id, &remote_item("远程"))
        .expect("remote");

    assert_eq!((first.position, second.position), (0, 1));
    assert_eq!(second.kind, "remote");
    assert_eq!(second.source, "bilibili");
    assert_eq!(second.source_id, "BV远程");
    assert_eq!(second.title, "远程");
    assert_eq!(second.artist, "远程艺人");
    assert_eq!(second.duration_ms, 236_000);
}

#[test]
fn get_playlist_orders_by_position() {
    let dir = common::TempDir::new("pl-order");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    for title in ["一", "二", "三"] {
        db.add_playlist_item(id, &local_item(title)).expect("add");
    }
    let (playlist, items) = db.get_playlist(id).expect("get");
    assert_eq!(playlist.name, "A");
    assert_eq!(names(&items), vec![("一", 0), ("二", 1), ("三", 2)]);
}

#[test]
fn remove_reorders_positions_to_be_contiguous() {
    let dir = common::TempDir::new("pl-remove");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    for title in ["一", "二", "三", "四"] {
        db.add_playlist_item(id, &local_item(title)).expect("add");
    }
    db.remove_playlist_item(id, 1).expect("remove second");

    let (_, items) = db.get_playlist(id).expect("get");
    assert_eq!(names(&items), vec![("一", 0), ("三", 1), ("四", 2)]);
}

#[test]
fn repeated_removes_keep_positions_contiguous() {
    let dir = common::TempDir::new("pl-remove-many");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    for title in ["一", "二", "三", "四", "五"] {
        db.add_playlist_item(id, &local_item(title)).expect("add");
    }
    db.remove_playlist_item(id, 0).expect("remove first");
    db.remove_playlist_item(id, 1)
        .expect("remove current second");

    let (_, items) = db.get_playlist(id).expect("get");
    assert_eq!(names(&items), vec![("二", 0), ("四", 1), ("五", 2)]);
}

#[test]
fn remove_missing_position_is_playlist_error() {
    let dir = common::TempDir::new("pl-remove-missing");
    let db = Database::open(dir.db_path()).expect("db");
    let id = db.create_playlist("A").expect("A");
    let error = db.remove_playlist_item(id, 0).expect_err("missing");
    assert!(matches!(error, LibraryError::Playlist(message) if message.contains("没有第 1 首")));
}

#[test]
fn playlist_lookup_is_case_insensitive() {
    let dir = common::TempDir::new("pl-lookup");
    let db = Database::open(dir.db_path()).expect("db");
    db.create_playlist("Mix").expect("create");
    assert!(db.playlist_by_name("mix").expect("lookup").is_some());
    assert!(db.playlist_by_name("nope").expect("lookup").is_none());
}

#[test]
fn add_to_missing_playlist_fails() {
    let dir = common::TempDir::new("pl-add-missing");
    let db = Database::open(dir.db_path()).expect("db");
    let error = db
        .add_playlist_item(999, &local_item("歌"))
        .expect_err("missing playlist");
    assert!(matches!(error, LibraryError::Playlist(_)));
}
