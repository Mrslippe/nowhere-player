//! 标签读取与封面哈希。
//!
//! 读取用 [`lofty`]，对上层只产出 [`ParsedTrack`]；封面只算内容哈希，
//! 不写缓存文件（T-0024 才落盘）。

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use lofty::tag::Tag;

use crate::error::{LibraryError, Result};

/// 扫描认的音频扩展名白名单（与 ARCH-003 §4 一致）。
pub(crate) const AUDIO_EXTENSIONS: &[&str] =
    &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif"];

/// 一个文件的解析结果（还没写库）。
#[derive(Debug, Clone)]
pub(crate) struct ParsedTrack {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: u32,
    pub duration_ms: i64,
    pub sample_rate: u32,
    pub channels: u32,
    pub bitrate: u32,
    pub codec: String,
    pub cover_key: Option<String>,
    pub size_bytes: i64,
    pub mtime_ms: i64,
}

/// 读取一个音频文件的标签与音频属性。
///
/// 任何解析失败都返回 [`LibraryError::Tag`]，由扫描器计入 `failed` 后继续。
pub(crate) fn parse_file(path: &Path, size_bytes: i64, mtime_ms: i64) -> Result<ParsedTrack> {
    let tagged = Probe::open(path)
        .map_err(|err| tag_error(path, err))?
        .read()
        .map_err(|err| tag_error(path, err))?;

    let properties = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title = tag
        .and_then(|tag| tag.title())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| file_stem(path));
    let artist = tag
        .and_then(|tag| tag.artist())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let album = tag
        .and_then(|tag| tag.album())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let album_artist = tag
        .and_then(|tag| tag.get_string(ItemKey::AlbumArtist))
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let year = tag.and_then(tag_year).unwrap_or(0);
    let track_no = tag.and_then(|tag| tag.track()).unwrap_or(0);
    let disc_no = tag.and_then(|tag| tag.disk()).unwrap_or(0);
    let cover_key = tag
        .and_then(|tag| tag.pictures().first())
        .filter(|picture| !picture.data().is_empty())
        .map(|picture| cover_key(picture.data()));

    Ok(ParsedTrack {
        path: path.to_path_buf(),
        title,
        artist,
        album,
        album_artist,
        track_no,
        disc_no,
        year,
        duration_ms: i64::try_from(properties.duration().as_millis()).unwrap_or(i64::MAX),
        sample_rate: properties.sample_rate().unwrap_or(0),
        channels: u32::from(properties.channels().unwrap_or(0)),
        bitrate: properties.audio_bitrate().unwrap_or(0),
        codec: codec_of(path),
        cover_key,
        size_bytes,
        mtime_ms,
    })
}

/// 文件名（不含扩展名）作为标题兜底。
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未命名".to_string())
}

/// 标签里的年份：优先 `Year` 文本，其次完整日期。
fn tag_year(tag: &Tag) -> Option<u32> {
    tag.get_string(ItemKey::Year)
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| tag.date().map(|date| u32::from(date.year)))
        .filter(|year| *year > 0)
}

/// 扩展名 → `codec` 列。
fn codec_of(path: &Path) -> String {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "aif" => "aiff".to_string(),
        "" => String::new(),
        other => other.to_string(),
    }
}

/// FNV-1a 64 位内容哈希，取 16 位 hex。
///
/// 必须确定性：同一份图片字节在任意进程里都得到同一个 key（ARCH-003 §3）。
pub(crate) fn cover_key(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn tag_error(path: &Path, err: impl std::fmt::Display) -> LibraryError {
    LibraryError::Tag {
        path: path.to_path_buf(),
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::cover_key;

    #[test]
    fn fnv1a_matches_reference_vector() {
        assert_eq!(cover_key(b"hello"), "a430d84680aabd0b");
    }

    #[test]
    fn cover_key_is_stable_and_short() {
        let bytes = [0u8, 1, 2, 3, 200, 255];
        let first = cover_key(&bytes);
        assert_eq!(first, cover_key(&bytes));
        assert_eq!(first.len(), 16);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn cover_key_changes_with_content() {
        assert_ne!(cover_key(b"cover-a"), cover_key(b"cover-b"));
    }
}
