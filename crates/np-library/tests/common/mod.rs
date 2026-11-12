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
    write_wav_bytes(path, frames, tags);
}

fn write_wav_bytes(path: &Path, frames: u32, tags: &[(&str, &str)]) {
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
