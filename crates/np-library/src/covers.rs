//! 封面缓存：缩放、JPEG 编码与落盘。
//!
//! 缓存位置是数据库同级的 `covers/`，文件名就是 [`crate::metadata::cover_key`]
//! 算出的内容哈希（`<key>.jpg`）。文件内容统一是 512px 长边、JPEG quality 82
//! 的 RGB 图；小图不放大（ARCH-003 §3）。
//!
//! 写入是原子的：同一目录下先写 `<key>.tmp`，再 `rename` 到 `<key>.jpg`，
//! 避免前端读到半截文件。编码在调用线程（扫描的 rayon 线程池）完成，只有
//! 落盘那一小段加锁，因此并行解析时不会串行化 CPU。

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

    use super::{CoverOutcome, store};

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
}
