//! 封面缓存：缩放、JPEG 编码与落盘。
//!
//! 缓存位置是数据库同级的 `covers/`，文件名就是 [`crate::metadata::cover_key`]
//! 算出的内容哈希（`<key>.jpg`）。文件内容统一是 512px 长边、JPEG quality 82
//! 的 RGB 图；小图不放大（ARCH-003 §3）。
//!
//! 写入是原子的：同一目录下先写 `<key>.tmp`，再 `rename` 到 `<key>.jpg`，
//! 避免前端读到半截文件。编码在调用线程（扫描的 rayon 线程池）完成，只有
//! 落盘那一小段加锁，因此并行解析时不会串行化 CPU。

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader};

/// 封面长边上限（ARCH-003 §3）。
pub(crate) const COVER_EDGE: u32 = 512;

/// 输出 JPEG 质量（ARCH-003 §3）。
pub(crate) const JPEG_QUALITY: u8 = 82;

/// 源图长边上限；超过直接跳过，避免解码吃内存（任务卡 §11）。
pub(crate) const MAX_SOURCE_EDGE: u32 = 8000;

/// 同目录封面候选，按优先级排列。
pub(crate) const SIDECAR_NAMES: &[&str] = &["cover.jpg", "folder.jpg", "front.jpg"];

/// 一次封面写的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoverOutcome {
    /// 目标文件已存在（内容哈希相同，必然一致），没有重写。
    Skipped,
    /// 编码并落盘成功。
    Written,
    /// 不是图片、已损坏或源图过大；调用方计入 `failed`。
    Failed,
}

/// 只在“写临时文件 + rename”这一小段加锁，编码仍可并行。
///
/// 同一张封面可能被同一轮扫描的多个线程同时命中（专辑内多首曲目共享
/// `cover_key`），共享 `<key>.tmp` 之前必须先串行化。
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 把封面字节解码、缩放、编码成 JPEG 并原子写入 `dir`。
///
/// `key` 是**源字节**的内容哈希，不是重新编码后文件的哈希。
pub(crate) fn store(dir: &Path, key: &str, source: &[u8]) -> CoverOutcome {
    let target = dir.join(format!("{key}.jpg"));
    if target.is_file() {
        return CoverOutcome::Skipped;
    }

    let Some(encoded) = encode(source) else {
        return CoverOutcome::Failed;
    };

    if fs::create_dir_all(dir).is_err() {
        return CoverOutcome::Failed;
    }

    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // 等锁期间别的线程可能已经写好同一张封面。
    if target.is_file() {
        return CoverOutcome::Skipped;
    }

    let tmp = dir.join(format!("{key}.tmp"));
    if fs::write(&tmp, &encoded).is_err() {
        let _ = fs::remove_file(&tmp);
        return CoverOutcome::Failed;
    }

    if fs::rename(&tmp, &target).is_ok() {
        CoverOutcome::Written
    } else {
        let _ = fs::remove_file(&tmp);
        if target.is_file() {
            CoverOutcome::Skipped
        } else {
            CoverOutcome::Failed
        }
    }
}

/// 删除 `dir` 里不再被数据库引用的封面文件，返回删除数量。
///
/// 只处理本模块会写出的两类名字：`<key>.jpg` 与 `<key>.tmp`（上次崩溃
/// 留下的半截文件）；其他文件一概不碰。删除失败只跳过该文件，不让一轮
/// 扫描因为一个只读/占用文件整体失败。
pub(crate) fn remove_orphans(dir: &Path, referenced: &HashSet<String>) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let orphan = if let Some(stem) = name.strip_suffix(".jpg") {
            !referenced.contains(stem)
        } else if let Some(stem) = name.strip_suffix(".tmp") {
            // 只认本模块写出的 `<16 位 hex>.tmp`，别碰目录里的其他临时文件。
            is_cover_key(stem)
        } else {
            false
        };
        if orphan && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 封面缓存用量（T-0026 §6.3）：只统计 `*.jpg` 的文件数与总字节。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverCacheUsage {
    /// `.jpg` 文件数。
    pub files: u64,
    /// 这些文件的总字节数。
    pub bytes: u64,
}

/// 统计封面缓存目录 `dir` 的用量。
///
/// - 目录不存在（从未缓存过封面）返回全 0；
/// - 只读目录项与文件元数据，**不读文件内容**：文件再多也只是一次 `read_dir` 遍历；
/// - 不递归子目录，也不跟随符号链接（`DirEntry::file_type` 不解析链接，链接一律跳过）；
/// - 只认扩展名 `jpg`：缓存文件名固定是 `<16 位小写 hex>.jpg`，其他文件（含
///   崩溃残留的 `.tmp`）不计入。
pub fn cover_cache_usage(dir: &Path) -> CoverCacheUsage {
    let Ok(entries) = fs::read_dir(dir) else {
        return CoverCacheUsage::default();
    };

    let mut usage = CoverCacheUsage::default();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("jpg") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        usage.files = usage.files.saturating_add(1);
        usage.bytes = usage.bytes.saturating_add(metadata.len());
    }
    usage
}

/// `cover_key` 的形状：16 位小写 hex。
fn is_cover_key(key: &str) -> bool {
    key.len() == 16
        && key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// 解码 → 缩放 → JPEG 编码；任何一步失败都返回 `None`。
fn encode(source: &[u8]) -> Option<Vec<u8>> {
    let reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .ok()?;
    let decoder = reader.into_decoder().ok()?;

    // 先只读文件头拿尺寸，超大图在分配像素缓冲之前就放弃。
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_SOURCE_EDGE || height > MAX_SOURCE_EDGE {
        return None;
    }

    let image = DynamicImage::from_decoder(decoder).ok()?;
    let rgb = image.to_rgb8();
    let rgb = shrink(&rgb, width, height);

    let mut encoded = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY);
    encoder.encode_image(&rgb).ok()?;
    Some(encoded)
}

/// `min(512 / w, 512 / h, 1.0)`，小图原样返回。
fn shrink(image: &image::RgbImage, width: u32, height: u32) -> image::RgbImage {
    let scale = (f64::from(COVER_EDGE) / f64::from(width))
        .min(f64::from(COVER_EDGE) / f64::from(height))
        .min(1.0);
    if scale >= 1.0 {
        return image.clone();
    }
    let scaled_width = ((f64::from(width) * scale).round() as u32).max(1);
    let scaled_height = ((f64::from(height) * scale).round() as u32).max(1);
    image::imageops::resize(image, scaled_width, scaled_height, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use image::{Rgb, RgbImage};

    use super::{CoverCacheUsage, CoverOutcome, cover_cache_usage, store};
    use crate::db::Database;
    use crate::metadata::ParsedTrack;
    use crate::repo;
    use crate::scan::{NoopObserver, Scanner};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "np-covers-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// 现场生成一张纯色渐变 JPEG，不依赖任何二进制夹具。
    fn jpeg_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 251) as u8, (y % 251) as u8, 96])
        });
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
        encoder.encode_image(&image).expect("encode source jpeg");
        bytes
    }

    fn read_dimensions(path: &Path) -> (u32, u32) {
        let image = image::ImageReader::open(path)
            .expect("open jpeg")
            .decode()
            .expect("decode jpeg");
        (image.width(), image.height())
    }

    #[test]
    fn writes_scaled_jpeg() {
        let dir = TempDir::new("scaled");
        let source = jpeg_bytes(1024, 768);

        let outcome = store(dir.path(), "scaled", &source);

        assert_eq!(outcome, CoverOutcome::Written);
        let target = dir.path().join("scaled.jpg");
        assert!(target.is_file(), "cover file missing");
        assert_eq!(read_dimensions(&target), (512, 384));
    }

    #[test]
    fn small_image_is_not_upscaled() {
        let dir = TempDir::new("small");
        let source = jpeg_bytes(100, 100);

        let outcome = store(dir.path(), "small", &source);

        assert_eq!(outcome, CoverOutcome::Written);
        assert_eq!(read_dimensions(&dir.path().join("small.jpg")), (100, 100));
    }

    #[test]
    fn existing_cover_is_not_rewritten() {
        let dir = TempDir::new("dedupe");
        let source = jpeg_bytes(640, 480);
        assert_eq!(store(dir.path(), "same", &source), CoverOutcome::Written);
        let target = dir.path().join("same.jpg");
        let before = fs::read(&target).expect("read cover");

        let outcome = store(dir.path(), "same", &source);

        assert_eq!(outcome, CoverOutcome::Skipped);
        assert_eq!(fs::read(&target).expect("read cover again"), before);
    }

    #[test]
    fn broken_image_fails_without_writing() {
        let dir = TempDir::new("broken");

        let outcome = store(dir.path(), "broken", b"not an image at all");

        assert_eq!(outcome, CoverOutcome::Failed);
        assert!(!dir.path().join("broken.jpg").exists());
        assert!(!dir.path().join("broken.tmp").exists());
    }

    #[test]
    fn oversized_image_is_skipped() {
        let dir = TempDir::new("oversized");
        let source = jpeg_bytes(8001, 8);

        let outcome = store(dir.path(), "huge", &source);

        assert_eq!(outcome, CoverOutcome::Failed);
        assert!(!dir.path().join("huge.jpg").exists());
    }

    fn parsed_track(path: &Path) -> ParsedTrack {
        ParsedTrack {
            path: path.to_path_buf(),
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

    #[test]
    fn full_scan_removes_only_orphan_covers() {
        let dir = TempDir::new("cleanup");
        let music = dir.path().join("music");
        fs::create_dir_all(&music).expect("music dir");
        let db = Database::open(dir.path().join("library.db")).expect("open db");

        let mut track = parsed_track(&music.join("a.wav"));
        track.cover_key = Some("kept".to_string());
        repo::upsert_track(db.conn(), &track, 100).expect("insert track");

        let covers_dir = db.covers_dir().expect("covers dir").to_path_buf();
        fs::create_dir_all(&covers_dir).expect("create covers dir");
        let kept = covers_dir.join("kept.jpg");
        let orphan = covers_dir.join("orphan.jpg");
        let stale = covers_dir.join("0123456789abcdef.tmp");
        let unrelated = covers_dir.join("notes.tmp");
        fs::write(&kept, b"jpeg").expect("write kept");
        fs::write(&orphan, b"jpeg").expect("write orphan");
        fs::write(&stale, b"partial").expect("write tmp");
        fs::write(&unrelated, b"keep me").expect("write unrelated tmp");

        Scanner::new(&db, &NoopObserver)
            .full()
            .scan(&[&music])
            .expect("full scan");

        assert!(kept.is_file(), "referenced cover was removed");
        assert!(!orphan.exists(), "orphan cover survived full scan");
        assert!(!stale.exists(), "stale tmp survived full scan");
        assert!(unrelated.is_file(), "unrelated tmp must be untouched");

        // 增量扫描不清理：新出现的孤儿文件要留到下一次全量。
        let later_orphan = covers_dir.join("later.jpg");
        fs::write(&later_orphan, b"jpeg").expect("write later orphan");
        Scanner::new(&db, &NoopObserver)
            .scan(&[&music])
            .expect("incremental scan");
        assert!(later_orphan.is_file(), "incremental scan cleaned covers");

        // 删掉引用它的曲目后，下一次全量扫描把封面收走。
        db.conn()
            .execute("DELETE FROM tracks", [])
            .expect("delete tracks");
        db.conn()
            .execute("DELETE FROM albums", [])
            .expect("delete albums");
        Scanner::new(&db, &NoopObserver)
            .full()
            .scan(&[&music])
            .expect("second full scan");

        assert!(!kept.exists(), "unreferenced cover survived full scan");
        assert!(!later_orphan.exists(), "orphan survived second full scan");
    }

    #[test]
    fn cover_cache_usage_counts_only_top_level_jpg_files() {
        let dir = TempDir::new("usage");
        fs::write(dir.path().join("0123456789abcdef.jpg"), b"12345").expect("write jpg");
        fs::write(dir.path().join("fedcba9876543210.jpg"), b"1234567").expect("write jpg");
        // 非封面文件、崩溃残留与子目录里的 jpg 都不计入。
        fs::write(dir.path().join("0123456789abcdef.tmp"), b"tmp").expect("write tmp");
        fs::write(dir.path().join("cover.png"), b"png").expect("write png");
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).expect("nested dir");
        fs::write(nested.join("aaaaaaaaaaaaaaaa.jpg"), b"nested").expect("write nested");

        assert_eq!(
            cover_cache_usage(dir.path()),
            CoverCacheUsage {
                files: 2,
                bytes: 12,
            }
        );
    }

    #[test]
    fn cover_cache_usage_of_missing_dir_is_zero() {
        let dir = TempDir::new("usage-missing");
        let missing = dir.path().join("never-created");

        assert_eq!(cover_cache_usage(&missing), CoverCacheUsage::default());
    }

    #[cfg(unix)]
    #[test]
    fn cover_cache_usage_skips_symlinked_covers() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new("usage-link");
        let target = dir.path().join("target.jpg");
        fs::write(&target, b"1234").expect("write target");
        let covers = dir.path().join("covers");
        fs::create_dir_all(&covers).expect("covers dir");
        symlink(&target, covers.join("0123456789abcdef.jpg")).expect("symlink");

        // 只认普通文件：符号链接既不计文件数，也不跟随读取目标字节。
        assert_eq!(cover_cache_usage(&covers), CoverCacheUsage::default());
    }
}
