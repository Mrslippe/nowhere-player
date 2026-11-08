//! 测试公共设施：临时目录、WAV 生成、mtime 设置。
//!
//! 测试数据全部在系统临时目录里现场生成，不向仓库提交任何二进制。

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 用完自动删除的临时目录。
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        let unique = format!(
            "np-library-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn db_path(&self) -> PathBuf {
        self.path.join("library.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// 写一个 16-bit PCM / 单声道 / 8 kHz 的静音 WAV。
///
/// `frames` 是采样点数，因此时长 = `frames / 8` 毫秒。
pub fn write_wav(path: &Path, frames: u32) {
    write_wav_with_tags(path, frames, &[]);
}

/// 同上，额外写入 RIFF INFO 标签（`INAM` / `IART` / `IPRD` / `IPRT`）。
///
/// 这是 WAV 自带的标签容器，不需要为了测试另造 FLAC。
pub fn write_wav_with_tags(path: &Path, frames: u32, tags: &[(&str, &str)]) {
    write_wav_bytes(path, frames, tags, None);
}

/// 写一个内嵌封面的 WAV：封面放在 `ID3 ` chunk 的 ID3v2.3 `APIC` 帧里。
///
/// WAV 的 RIFF INFO 不支持图片，但 lofty 会读 `ID3 ` chunk（与音频标签
/// 同一套解析），因此用它模拟“标签内嵌图”，不需要 FLAC 编码器。
pub fn write_wav_with_cover(path: &Path, frames: u32, cover: &[u8]) {
    write_wav_bytes(path, frames, &[], Some(cover));
}

/// 现场生成一张渐变色 JPEG，测试图片不落仓库。
pub fn jpeg_bytes(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 251) as u8, 128])
    });
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
    encoder.encode_image(&image).expect("encode generated jpeg");
    bytes
}

/// 用 `image` 读回 JPEG 并返回尺寸，顺便断言文件可解析。
pub fn jpeg_dimensions(path: &Path) -> (u32, u32) {
    let image = image::ImageReader::open(path)
        .expect("open jpeg")
        .decode()
        .expect("decode jpeg");
    (image.width(), image.height())
}

/// FNV-1a 64 位内容哈希（与 `metadata::cover_key` 同一算法），测试侧独立实现。
pub fn content_key(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn write_wav_bytes(path: &Path, frames: u32, tags: &[(&str, &str)], cover: Option<&[u8]>) {
    let sample_rate: u32 = 8_000;
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = frames * u32::from(block_align);

    let mut chunks = Vec::new();

    // fmt chunk
    chunks.extend_from_slice(b"fmt ");
    chunks.extend_from_slice(&16u32.to_le_bytes());
    chunks.extend_from_slice(&1u16.to_le_bytes()); // PCM
    chunks.extend_from_slice(&channels.to_le_bytes());
    chunks.extend_from_slice(&sample_rate.to_le_bytes());
    chunks.extend_from_slice(&byte_rate.to_le_bytes());
    chunks.extend_from_slice(&block_align.to_le_bytes());
    chunks.extend_from_slice(&bits_per_sample.to_le_bytes());

    if !tags.is_empty() {
        chunks.extend_from_slice(&riff_info(tags));
    }

    if let Some(cover) = cover {
        chunks.extend_from_slice(&id3_apic_chunk(cover));
    }

    // data chunk
    chunks.extend_from_slice(b"data");
    chunks.extend_from_slice(&data_len.to_le_bytes());
    chunks.resize(chunks.len() + data_len as usize, 0);

    let mut bytes = Vec::with_capacity(12 + chunks.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(4 + chunks.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(&chunks);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create wav parent");
    }
    fs::write(path, bytes).expect("write wav");
}

/// 构造一个带单张 `APIC` 帧的 `ID3 ` RIFF chunk（ID3v2.3）。
fn id3_apic_chunk(cover: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(cover.len() + 32);
    body.push(0); // Latin-1
    body.extend_from_slice(b"image/jpeg\0");
    body.push(3); // front cover
    body.push(0); // 空描述
    body.extend_from_slice(cover);

    let mut frames = Vec::with_capacity(body.len() + 10);
    frames.extend_from_slice(b"APIC");
    frames.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frames.extend_from_slice(&[0, 0]); // flags
    frames.extend_from_slice(&body);

    let mut tag = Vec::with_capacity(frames.len() + 10);
    tag.extend_from_slice(b"ID3");
    tag.extend_from_slice(&[3, 0, 0]); // v2.3, flags 0
    tag.extend_from_slice(&syncsafe(frames.len() as u32));
    tag.extend_from_slice(&frames);

    let mut chunk = Vec::with_capacity(tag.len() + 8);
    chunk.extend_from_slice(b"ID3 ");
    chunk.extend_from_slice(&(tag.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&tag);
    if tag.len() % 2 == 1 {
        chunk.push(0); // RIFF chunk 对齐到偶数
    }
    chunk
}

fn syncsafe(value: u32) -> [u8; 4] {
    [
        ((value >> 21) & 0x7F) as u8,
        ((value >> 14) & 0x7F) as u8,
        ((value >> 7) & 0x7F) as u8,
        (value & 0x7F) as u8,
    ]
}

/// 构造一个 `LIST ....INFO` chunk。
fn riff_info(tags: &[(&str, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"INFO");
    for (key, value) in tags {
        let payload = value.as_bytes();
        let size = payload.len() as u32 + 1; // 含结尾 NUL
        body.extend_from_slice(key.as_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(payload);
        body.push(0);
        if !size.is_multiple_of(2) {
            body.push(0); // chunk 对齐到偶数
        }
    }
    let mut chunk = Vec::with_capacity(8 + body.len());
    chunk.extend_from_slice(b"LIST");
    chunk.extend_from_slice(&(body.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&body);
    chunk
}

/// 显式设置文件 mtime（Unix 毫秒），避免测试依赖真实时钟间隔。
pub fn set_mtime(path: &Path, unix_ms: u64) {
    let time = SystemTime::UNIX_EPOCH + Duration::from_millis(unix_ms);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open for set_times");
    file.set_times(fs::FileTimes::new().set_modified(time))
        .expect("set mtime");
}
