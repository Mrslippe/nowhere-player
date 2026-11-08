//! 封面缓存端到端测试：来源优先级、缩放、去重、损坏与缓存目录形状。
//!
//! 图片全部由 `common::jpeg_bytes` 现场生成，WAV 由 `common::write_wav_*`
//! 现场拼出（内嵌封面用 ID3v2.3 `APIC`），不向仓库提交任何二进制夹具。

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use np_library::{Database, NoopObserver, ScanReport, Scanner, Track};

fn scan(db: &Database, root: &Path, full: bool) -> ScanReport {
    let scanner = Scanner::new(db, &NoopObserver);
    let scanner = if full { scanner.full() } else { scanner };
    scanner.scan(&[root]).expect("scan succeeds")
}

fn track_for(db: &Database, name: &str) -> Track {
    db.list_recent(50)
        .expect("list tracks")
        .into_iter()
        .find(|track| track.path.ends_with(name))
        .unwrap_or_else(|| panic!("{name} not indexed"))
}

fn cover_path(db: &Database, key: &str) -> PathBuf {
    db.covers_dir()
        .expect("file-backed db has covers dir")
        .join(format!("{key}.jpg"))
}

fn jpg_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .expect("read covers dir")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jpg"))
        .count()
}

#[test]
fn embedded_cover_is_written_and_scaled_to_512() {
    let dir = common::TempDir::new("covers-embedded");
    let cover = common::jpeg_bytes(1024, 768);
    common::write_wav_with_cover(&dir.path().join("song.wav"), 800, &cover);
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, dir.path(), false);

    assert_eq!(report.indexed, 1);
    assert_eq!(report.failed, 0);
    let track = track_for(&db, "song.wav");
    let key = track.cover_key.expect("embedded cover key");
    assert_eq!(
        key,
        common::content_key(&cover),
        "key must hash source bytes"
    );
    let path = cover_path(&db, &key);
    assert!(path.is_file(), "cover was not written");
    assert_eq!(common::jpeg_dimensions(&path), (512, 384));
    assert_eq!(
        db.covers_dir(),
        Some(dir.path().join("covers").as_path()),
        "covers dir should be a sibling of library.db"
    );
}

#[test]
fn small_cover_is_not_upscaled() {
    let dir = common::TempDir::new("covers-small");
    let cover = common::jpeg_bytes(100, 100);
    common::write_wav_with_cover(&dir.path().join("song.wav"), 800, &cover);
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, dir.path(), false);

    let key = track_for(&db, "song.wav").cover_key.expect("cover key");
    assert_eq!(common::jpeg_dimensions(&cover_path(&db, &key)), (100, 100));
}

#[test]
fn sidecar_cover_is_used_when_no_embedded_image() {
    let dir = common::TempDir::new("covers-sidecar");
    common::write_wav(&dir.path().join("song.wav"), 800);
    let sidecar = common::jpeg_bytes(640, 480);
    fs::write(dir.path().join("cover.jpg"), &sidecar).expect("write cover.jpg");
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, dir.path(), false);

    let key = track_for(&db, "song.wav")
        .cover_key
        .expect("sidecar cover key");
    assert_eq!(key, common::content_key(&sidecar));
    assert_eq!(common::jpeg_dimensions(&cover_path(&db, &key)), (512, 384));
}

#[test]
fn sidecar_falls_back_to_folder_then_front() {
    let dir = common::TempDir::new("covers-folder");
    common::write_wav(&dir.path().join("song.wav"), 800);
    let folder = common::jpeg_bytes(300, 200);
    fs::write(dir.path().join("folder.jpg"), &folder).expect("write folder.jpg");
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, dir.path(), false);

    let key = track_for(&db, "song.wav").cover_key.expect("cover key");
    assert_eq!(key, common::content_key(&folder));
    assert_eq!(common::jpeg_dimensions(&cover_path(&db, &key)), (300, 200));
}

#[test]
fn embedded_cover_wins_over_sidecar() {
    let dir = common::TempDir::new("covers-priority");
    let embedded = common::jpeg_bytes(1024, 768);
    common::write_wav_with_cover(&dir.path().join("song.wav"), 800, &embedded);
    fs::write(dir.path().join("cover.jpg"), common::jpeg_bytes(100, 100)).expect("write cover.jpg");
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, dir.path(), false);

    let key = track_for(&db, "song.wav").cover_key.expect("cover key");
    assert_eq!(key, common::content_key(&embedded));
    assert_eq!(common::jpeg_dimensions(&cover_path(&db, &key)), (512, 384));
}

#[test]
fn identical_cover_is_written_once_and_not_rewritten() {
    let dir = common::TempDir::new("covers-dedupe");
    let cover = common::jpeg_bytes(800, 600);
    common::write_wav_with_cover(&dir.path().join("a.wav"), 800, &cover);
    common::write_wav_with_cover(&dir.path().join("b.wav"), 800, &cover);
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, dir.path(), false);

    assert_eq!(report.indexed, 2);
    let key_a = track_for(&db, "a.wav").cover_key.expect("a cover");
    let key_b = track_for(&db, "b.wav").cover_key.expect("b cover");
    assert_eq!(key_a, key_b);
    let covers_dir = db.covers_dir().expect("covers dir");
    assert_eq!(jpg_count(covers_dir), 1, "same key must share one file");

    // 全量扫描会重新解析（并尝试写封面）；已存在的文件必须原样保留。
    let path = cover_path(&db, &key_a);
    common::set_mtime(&path, 1_600_000_000_000);
    let report = scan(&db, dir.path(), true);
    assert_eq!(report.failed, 0);
    let modified_ms = fs::metadata(&path)
        .expect("cover metadata")
        .modified()
        .expect("mtime")
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after epoch")
        .as_millis();
    assert_eq!(modified_ms, 1_600_000_000_000, "cover was rewritten");
}

#[test]
fn corrupt_embedded_cover_counts_as_failed_but_indexes_track() {
    let dir = common::TempDir::new("covers-corrupt");
    common::write_wav_with_cover(&dir.path().join("broken.wav"), 800, b"not an image");
    common::write_wav_with_cover(
        &dir.path().join("good.wav"),
        800,
        &common::jpeg_bytes(400, 400),
    );
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, dir.path(), false);

    assert_eq!(report.indexed, 2);
    assert_eq!(report.failed, 1, "corrupt cover should count one failure");
    let broken = track_for(&db, "broken.wav");
    assert_eq!(
        broken.cover_key, None,
        "failed cover must not be referenced"
    );
    assert_eq!(jpg_count(db.covers_dir().expect("covers dir")), 1);
}

#[test]
fn oversized_cover_is_skipped_and_counted_as_failed() {
    let dir = common::TempDir::new("covers-huge");
    let huge = common::jpeg_bytes(8001, 8);
    common::write_wav_with_cover(&dir.path().join("huge.wav"), 800, &huge);
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, dir.path(), false);

    assert_eq!(report.indexed, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(track_for(&db, "huge.wav").cover_key, None);
    let covers_dir = db.covers_dir().expect("covers dir");
    assert!(!covers_dir.exists() || jpg_count(covers_dir) == 0);
}

#[test]
fn full_scan_removes_orphan_cover_files() {
    let dir = common::TempDir::new("covers-cleanup");
    let cover = common::jpeg_bytes(400, 400);
    common::write_wav_with_cover(&dir.path().join("song.wav"), 800, &cover);
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, dir.path(), false);

    let covers_dir = db.covers_dir().expect("covers dir");
    let orphan = covers_dir.join("0000000000000000.jpg");
    fs::write(&orphan, b"jpeg").expect("write orphan");
    let orphans_before = jpg_count(covers_dir);

    scan(&db, dir.path(), true);

    assert!(!orphan.exists(), "full scan should remove orphan cover");
    assert_eq!(jpg_count(covers_dir), orphans_before - 1);

    // 回归：仍是全量才会清理，引用中的封面不受影响。
    let track = track_for(&db, "song.wav");
    let key = track.cover_key.expect("cover key");
    let kept = cover_path(&db, &key);
    assert!(kept.is_file());
    let modified = fs::metadata(&kept)
        .expect("kept metadata")
        .modified()
        .expect("mtime");
    scan(&db, dir.path(), true);
    assert!(kept.is_file());
    assert_eq!(
        fs::metadata(&kept)
            .expect("kept metadata")
            .modified()
            .expect("mtime"),
        modified
    );
}

#[test]
fn cover_files_are_placed_next_to_database() {
    let dir = common::TempDir::new("covers-shape");
    let db = Database::open(dir.db_path()).expect("open db");

    assert_eq!(db.covers_dir(), Some(dir.path().join("covers").as_path()));

    let memory = Database::open_in_memory().expect("memory db");
    assert_eq!(memory.covers_dir(), None);
}
