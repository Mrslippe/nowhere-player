//! 查询契约测试（ARCH-003 §5 的前四项）。

mod common;

use std::path::Path;

use np_library::{Database, NoopObserver, Scanner};

fn scan(db: &Database, root: &Path) {
    Scanner::new(db, &NoopObserver)
        .scan(&[root])
        .expect("scan succeeds");
}

#[test]
fn search_matches_title_artist_and_album_tags() {
    let dir = common::TempDir::new("query-search");
    common::write_wav_with_tags(
        &dir.path().join("01.wav"),
        800,
        &[
            ("INAM", "Airbag"),
            ("IART", "Radiohead"),
            ("IPRD", "OK Computer"),
        ],
    );
    common::write_wav_with_tags(
        &dir.path().join("02.wav"),
        800,
        &[
            ("INAM", "Paranoid Android"),
            ("IART", "Radiohead"),
            ("IPRD", "OK Computer"),
        ],
    );
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, dir.path());

    let by_title = db.search("airbag", 10).expect("search title");
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].title, "Airbag");
    assert_eq!(db.search("radiohead", 10).expect("artist").len(), 2);
    assert_eq!(db.search("computer", 10).expect("album").len(), 2);
    assert_eq!(db.search("RADIOHEAD", 10).expect("case").len(), 2);
    assert!(db.search("missing", 10).expect("miss").is_empty());
    assert_eq!(db.search("", 1).expect("limit").len(), 1);
}

#[test]
fn stats_and_tracks_by_album_use_tagged_rows() {
    let dir = common::TempDir::new("query-album");
    common::write_wav_with_tags(
        &dir.path().join("02.wav"),
        800,
        &[
            ("INAM", "Paranoid Android"),
            ("IART", "Radiohead"),
            ("IPRD", "OK Computer"),
            ("IPRT", "2"),
        ],
    );
    common::write_wav_with_tags(
        &dir.path().join("01.wav"),
        1600,
        &[
            ("INAM", "Airbag"),
            ("IART", "Radiohead"),
            ("IPRD", "OK Computer"),
            ("IPRT", "1"),
        ],
    );
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, dir.path());

    let stats = db.stats().expect("stats");
    assert_eq!(stats.tracks, 2);
    assert_eq!(stats.albums, 1);
    assert_eq!(stats.artists, 1);
    assert!(stats.last_scan_at > 0);

    let album_id = db
        .search("airbag", 1)
        .expect("search")
        .remove(0)
        .album_id
        .expect("album id");
    let tracks = db.tracks_by_album(album_id).expect("by album");
    assert_eq!(
        tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Airbag", "Paranoid Android"]
    );
    assert_eq!(
        stats.total_duration_ms,
        tracks
            .iter()
            .map(|track| u64::try_from(track.duration_ms).expect("duration"))
            .sum::<u64>()
    );
}

#[test]
fn list_recent_returns_newest_first() {
    let dir = common::TempDir::new("query-recent");
    common::write_wav(&dir.path().join("a.wav"), 800);
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, dir.path());
    common::write_wav(&dir.path().join("b.wav"), 800);
    scan(&db, dir.path());

    let recent = db.list_recent(1).expect("recent");
    assert_eq!(recent.len(), 1);
    assert!(recent[0].path.ends_with("b.wav"));
}

#[test]
fn artist_case_variants_share_one_row() {
    let dir = common::TempDir::new("query-case");
    common::write_wav_with_tags(
        &dir.path().join("a.wav"),
        800,
        &[("IART", "Radiohead"), ("IPRD", "OK Computer")],
    );
    common::write_wav_with_tags(
        &dir.path().join("b.wav"),
        800,
        &[("IART", "radiohead"), ("IPRD", "ok computer")],
    );
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, dir.path());

    let stats = db.stats().expect("stats");
    assert_eq!(stats.artists, 1);
    assert_eq!(stats.albums, 1);
}
