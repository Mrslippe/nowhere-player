//! 扫描、增量、缺失、失败与取消的行为测试。

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use np_library::{Database, ScanObserver, Scanner};

#[derive(Default)]
struct Observer {
    progress: Mutex<Vec<(usize, usize, PathBuf)>>,
    parsed: Mutex<Vec<PathBuf>>,
    cancel_after: AtomicUsize,
}

impl Observer {
    fn cancelling_after_first() -> Self {
        let observer = Self::default();
        observer.cancel_after.store(1, Ordering::SeqCst);
        observer
    }

    fn progress(&self) -> Vec<(usize, usize, PathBuf)> {
        self.progress.lock().expect("progress lock").clone()
    }

    fn parsed(&self) -> Vec<PathBuf> {
        self.parsed.lock().expect("parsed lock").clone()
    }
}

impl ScanObserver for Observer {
    fn on_progress(&self, scanned: usize, total: usize, current_path: &Path) {
        self.progress.lock().expect("progress lock").push((
            scanned,
            total,
            current_path.to_path_buf(),
        ));
    }

    fn on_parsed(&self, path: &Path) {
        self.parsed
            .lock()
            .expect("parsed lock")
            .push(path.to_path_buf());
    }

    fn is_cancelled(&self) -> bool {
        let threshold = self.cancel_after.load(Ordering::SeqCst);
        threshold > 0 && self.progress.lock().expect("progress lock").len() >= threshold
    }
}

fn write_wavs(dir: &Path, names: &[&str]) {
    for name in names {
        common::write_wav(&dir.join(name), 800);
    }
}

fn scan(db: &Database, observer: &Observer, root: &Path) -> np_library::ScanReport {
    Scanner::new(db, observer)
        .scan(&[root])
        .expect("scan succeeds")
}

#[test]
fn scan_three_wavs_indexes_three_tracks() {
    let dir = common::TempDir::new("scan-basic");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    let observer = Observer::default();

    let report = scan(&db, &observer, dir.path());

    assert_eq!(report.scanned, 3);
    assert_eq!(report.indexed, 3);
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failed, 0);
    assert!(!report.cancelled);
    let tracks = db.list_recent(10).expect("tracks");
    assert_eq!(tracks.len(), 3);
    assert!(tracks.iter().all(|track| track.kind == "local"));
}

#[test]
fn rescan_skips_unchanged_files_and_keeps_row_identity() {
    let dir = common::TempDir::new("scan-rescan");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    let first = Observer::default();
    scan(&db, &first, dir.path());
    let before = db.list_recent(10).expect("tracks");
    let ids: Vec<i64> = before.iter().map(|track| track.id).collect();

    let second = Observer::default();
    let report = scan(&db, &second, dir.path());

    assert_eq!(report.scanned, 3);
    assert_eq!(report.indexed, 0);
    assert_eq!(report.skipped, 3);
    assert!(second.parsed().is_empty());
    let after = db.list_recent(10).expect("tracks");
    assert_eq!(after.len(), 3);
    let after_ids: Vec<i64> = after.iter().map(|track| track.id).collect();
    assert_eq!(ids, after_ids);
    assert!(after[0].last_seen_at >= before[0].last_seen_at);
}

#[test]
fn changed_mtime_reparses_only_that_file() {
    let dir = common::TempDir::new("scan-incremental");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, &Observer::default(), dir.path());

    common::set_mtime(&dir.path().join("b.wav"), 1_500_000_000_000);
    let observer = Observer::default();
    let report = scan(&db, &observer, dir.path());

    assert_eq!(report.indexed, 1);
    assert_eq!(report.skipped, 2);
    let parsed = observer.parsed();
    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].file_name().and_then(|name| name.to_str()),
        Some("b.wav")
    );
}

#[test]
fn deleted_file_is_marked_missing_but_not_removed() {
    let dir = common::TempDir::new("scan-missing");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    scan(&db, &Observer::default(), dir.path());

    let before = db
        .list_recent(10)
        .expect("tracks")
        .into_iter()
        .find(|track| track.path.ends_with("c.wav"))
        .expect("c.wav indexed");
    fs::remove_file(dir.path().join("c.wav")).expect("delete file");

    let report = scan(&db, &Observer::default(), dir.path());

    assert_eq!(report.missing, 1);
    let after = db
        .list_recent(10)
        .expect("tracks")
        .into_iter()
        .find(|track| track.path.ends_with("c.wav"))
        .expect("missing row kept");
    assert_eq!(after.id, before.id);
    assert_eq!(after.last_seen_at, before.last_seen_at);
}

#[test]
fn broken_file_counts_as_failed_and_others_still_index() {
    let dir = common::TempDir::new("scan-broken");
    common::write_wav(&dir.path().join("good.wav"), 800);
    fs::write(dir.path().join("broken.mp3"), b"this is not audio").expect("write broken file");
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, &Observer::default(), dir.path());

    assert_eq!(report.scanned, 2);
    assert_eq!(report.indexed, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(db.list_recent(10).expect("tracks").len(), 1);
}

#[test]
fn cancellation_keeps_the_part_already_written() {
    let dir = common::TempDir::new("scan-cancel");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    let observer = Observer::cancelling_after_first();

    let report = scan(&db, &observer, dir.path());

    assert!(report.cancelled);
    assert_eq!(report.indexed, 1);
    assert!((1..=3).contains(&report.scanned));
    assert_eq!(db.list_recent(10).expect("tracks").len(), 1);
}

#[test]
fn progress_reports_total_and_every_file() {
    let dir = common::TempDir::new("scan-progress");
    write_wavs(dir.path(), &["a.wav", "b.wav", "c.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");
    let observer = Observer::default();

    scan(&db, &observer, dir.path());

    let progress = observer.progress();
    assert_eq!(progress.len(), 3);
    assert_eq!(
        progress
            .iter()
            .map(|(scanned, ..)| *scanned)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(progress.iter().all(|(_, total, _)| *total == 3));
    let mut names: Vec<String> = progress
        .iter()
        .map(|(_, _, path)| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.wav", "b.wav", "c.wav"]);
}

#[test]
fn hidden_and_dependency_directories_are_skipped() {
    let dir = common::TempDir::new("scan-skip");
    common::write_wav(&dir.path().join("visible.wav"), 800);
    fs::create_dir_all(dir.path().join(".hidden")).expect("hidden dir");
    common::write_wav(&dir.path().join(".hidden/inside.wav"), 800);
    fs::create_dir_all(dir.path().join("node_modules")).expect("node_modules dir");
    common::write_wav(&dir.path().join("node_modules/inside.wav"), 800);
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, &Observer::default(), dir.path());

    assert_eq!(report.scanned, 1);
    assert_eq!(report.indexed, 1);
}

#[test]
fn untagged_wav_uses_filename_and_audio_properties() {
    let dir = common::TempDir::new("scan-wav");
    common::write_wav(&dir.path().join("song.wav"), 800);
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, &Observer::default(), dir.path());

    let track = db.list_recent(1).expect("tracks").remove(0);
    assert_eq!(track.title, "song");
    assert_eq!(track.codec, "wav");
    assert_eq!(track.sample_rate, 8_000);
    assert_eq!(track.channels, 1);
    assert!(
        (50..=200).contains(&track.duration_ms),
        "duration was {}",
        track.duration_ms
    );
    assert!(track.size_bytes > 0);
    assert!(track.mtime_ms > 0);
}

#[test]
fn scan_records_root_and_stats() {
    let dir = common::TempDir::new("scan-stats");
    write_wavs(dir.path(), &["a.wav", "b.wav"]);
    let db = Database::open(dir.db_path()).expect("open db");

    scan(&db, &Observer::default(), dir.path());

    let stats = db.stats().expect("stats");
    assert_eq!(stats.tracks, 2);
    assert!(stats.last_scan_at > 0);
}

#[test]
fn empty_root_is_a_noop() {
    let dir = common::TempDir::new("scan-empty");
    let db = Database::open(dir.db_path()).expect("open db");

    let report = scan(&db, &Observer::default(), dir.path());

    assert_eq!(report, np_library::ScanReport::default());
}
