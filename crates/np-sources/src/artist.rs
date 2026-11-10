//! `--artist` 的平台相关匹配（T-0047）。
//!
//! B 站没有结构化艺人字段：上传者可能只是切片/搬运号，而用户要的人是
//! **标题里被提到的人**。因此 B 站按“上传者或标题”匹配，并让上传者命中排前；
//! 网易云的 `artists` 是结构化元数据，仍只按艺人字段匹配。

use crate::model::{SourceId, SourceTrack};

/// 一条结果对 `--artist` 的命中类型；顺序即排序优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// 上传者 / 艺人字段命中。
    Author,
    /// 标题命中（仅 B 站）。
    Title,
    /// 不命中。
    None,
}

/// 构造发给源的 query。
///
/// 探测显示 `艺人 关键词` 比 `关键词 艺人` 召回更多目标切片，所以 artist 放前面；
/// 只有 artist 时只发 artist，避免重复。
pub fn build_query(keyword: &str, artist: Option<&str>) -> String {
    let keyword = keyword.trim();
    let artist = artist.map(str::trim).filter(|name| !name.is_empty());

    match (artist, keyword.is_empty()) {
        (Some(artist), true) => artist.to_owned(),
        (Some(artist), false) if artist == keyword => artist.to_owned(),
        (Some(artist), false) => format!("{artist} {keyword}"),
        (None, _) => keyword.to_owned(),
    }
}

/// 按平台匹配“这个人的名字”。
///
/// 大小写不敏感、子串匹配；名字首尾空白会被忽略。
pub fn match_artist_name(source: SourceId, artist: &str, title: &str, name: &str) -> MatchKind {
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        return MatchKind::None;
    }

    let author_hit = artist.to_lowercase().contains(&wanted);
    match source {
        SourceId::Bilibili => {
            if author_hit {
                MatchKind::Author
            } else if title.to_lowercase().contains(&wanted) {
                MatchKind::Title
            } else {
                MatchKind::None
            }
        }
        // 网易云的 artists 是结构化字段，标题不参与。
        SourceId::Netease | SourceId::Ytdlp => {
            if author_hit {
                MatchKind::Author
            } else {
                MatchKind::None
            }
        }
    }
}

/// 便捷入口：按 `SourceTrack` 的 source / artist / title 匹配。
pub fn matches_artist(track: &SourceTrack, name: &str) -> MatchKind {
    match_artist_name(track.source, &track.artist, &track.title, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(source: SourceId, artist: &str, title: &str) -> SourceTrack {
        SourceTrack {
            source,
            source_id: "id".to_owned(),
            title: title.to_owned(),
            artist: artist.to_owned(),
            duration_ms: 60_000,
            album: None,
            cover_url: None,
        }
    }

    #[test]
    fn query_puts_artist_before_keyword() {
        assert_eq!(build_query("晴天", Some("周杰伦")), "周杰伦 晴天");
    }

    #[test]
    fn query_with_only_keyword_is_unchanged() {
        assert_eq!(build_query("晴天", None), "晴天");
    }

    #[test]
    fn query_with_only_artist_is_artist() {
        assert_eq!(build_query("", Some("周杰伦")), "周杰伦");
        assert_eq!(build_query("   ", Some(" 周杰伦 ")), "周杰伦");
    }

    #[test]
    fn query_deduplicates_artist_as_keyword() {
        assert_eq!(build_query("周杰伦", Some("周杰伦")), "周杰伦");
    }

    #[test]
    fn query_empty_artist_falls_back_to_keyword() {
        assert_eq!(build_query("晴天", Some("  ")), "晴天");
    }

    #[test]
    fn query_keeps_both_empty() {
        assert_eq!(build_query("", None), "");
        assert_eq!(build_query("  ", Some(" ")), "");
    }

    #[test]
    fn bilibili_author_match_is_author() {
        let track = track(SourceId::Bilibili, "明前奶绿Official", "翻唱");
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::Author);
    }

    #[test]
    fn bilibili_title_match_is_title() {
        let track = track(
            SourceId::Bilibili,
            "切片搬运号",
            "【明前奶绿】不为谁而作的歌",
        );
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::Title);
    }

    #[test]
    fn bilibili_author_wins_when_both_match() {
        let track = track(SourceId::Bilibili, "明前奶绿", "明前奶绿翻唱");
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::Author);
    }

    #[test]
    fn bilibili_none_when_neither_matches() {
        let track = track(SourceId::Bilibili, "别人", "别的歌");
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::None);
    }

    #[test]
    fn netease_ignores_title() {
        let track = track(SourceId::Netease, "别的艺人", "明前奶绿翻唱");
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::None);
    }

    #[test]
    fn netease_artist_substring_is_author() {
        let track = track(SourceId::Netease, "明前奶绿Official", "歌");
        assert_eq!(matches_artist(&track, "明前奶绿"), MatchKind::Author);
    }

    #[test]
    fn matching_is_case_insensitive_and_trims_name() {
        let track = track(SourceId::Bilibili, "Jay Chou", "晴天");
        assert_eq!(matches_artist(&track, "  jay  "), MatchKind::Author);
    }

    #[test]
    fn empty_name_never_matches() {
        let track = track(SourceId::Bilibili, "明前奶绿", "明前奶绿");
        assert_eq!(matches_artist(&track, "   "), MatchKind::None);
    }
}
