//! `--artist` 的平台相关匹配（T-0047）。
//!
//! B 站没有结构化艺人字段：上传者可能只是切片/搬运号，而用户要的人是
//! **标题里被提到的人**。因此 B 站按“上传者或标题”匹配，并让上传者命中排前；
//! 网易云的 `artists` 是结构化元数据，仍只按艺人字段匹配。

use crate::model::{SourceId, SourceTrack, UpUser};

/// 用户名解析结果（T-0049）：选中的 UP + 精确同名候选数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUp {
    pub user: UpUser,
    pub same_name: usize,
}

/// 优先精确同名，多个同名取粉丝最多；没有精确同名则取粉丝最多的候选。
pub fn resolve_up_user(users: &[UpUser], name: &str) -> Option<ResolvedUp> {
    let wanted = name.trim();
    let exact_count = users.iter().filter(|user| user.name == wanted).count();
    let chosen = users
        .iter()
        .filter(|user| user.name == wanted)
        .max_by_key(|user| user.fans)
        .or_else(|| users.iter().max_by_key(|user| user.fans))?;
    Some(ResolvedUp {
        user: chosen.clone(),
        same_name: exact_count,
    })
}

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

/// 同时给了关键词与 artist 时的匹配（T-0047 精度规则）。
///
/// B 站要求标题同时含关键词——关键词表达“要什么内容”，artist 表达“谁”，
/// 两者都给了就该同时满足。只给 artist 时 `keyword = None`，不做这个约束。
/// 网易云接口本身精确，忽略关键词约束。
pub fn matches_artist_in_query(
    track: &SourceTrack,
    name: &str,
    keyword: Option<&str>,
) -> MatchKind {
    if track.source == SourceId::Bilibili
        && let Some(keyword) = keyword
        && !title_has_all_keywords(&track.title, keyword)
    {
        return MatchKind::None;
    }
    matches_artist(track, name)
}

/// 多关键词标题约束：每个空白分隔的词都要出现在标题里（不要求连续）。
///
/// 空关键词视为不过滤。B 站自己的分词不可控，这是结果侧能做的稳定兜底。
pub fn title_has_all_keywords(title: &str, keyword: &str) -> bool {
    let title = title.to_lowercase();
    keyword
        .split_whitespace()
        .all(|token| title.contains(&token.to_lowercase()))
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

    fn up(mid: u64, name: &str, fans: u64) -> UpUser {
        UpUser {
            mid,
            name: name.to_owned(),
            fans,
        }
    }

    #[test]
    fn resolve_up_prefers_exact_and_highest_fans() {
        let users = vec![
            up(1, "明前奶绿", 100),
            up(2, "明前奶绿", 300),
            up(3, "明明前奶绿", 999),
        ];
        let resolved = resolve_up_user(&users, "明前奶绿").expect("exact");
        assert_eq!(resolved.user.mid, 2);
        assert_eq!(resolved.same_name, 2);
    }

    #[test]
    fn resolve_up_falls_back_to_highest_fans() {
        let users = vec![up(1, "别人", 10), up(2, "另一个", 99)];
        let resolved = resolve_up_user(&users, "明前奶绿").expect("fallback");
        assert_eq!(resolved.user.mid, 2);
        assert_eq!(resolved.same_name, 0);
    }

    #[test]
    fn resolve_up_none_when_empty() {
        assert!(resolve_up_user(&[], "明前奶绿").is_none());
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
    fn title_constraint_requires_every_keyword_token() {
        let track = track(
            SourceId::Bilibili,
            "搬运",
            "明前奶绿 文静 老公天下第一 第一次",
        );
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("文静 第一次")),
            MatchKind::Title
        );
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("文静 不存在")),
            MatchKind::None
        );
    }

    #[test]
    fn title_constraint_does_not_require_contiguous_phrase() {
        assert!(title_has_all_keywords(
            "【明前奶绿】第一次 唱 文静",
            "文静 第一次"
        ));
        assert!(!title_has_all_keywords("【明前奶绿】第一次", "文静 第一次"));
    }

    #[test]
    fn matching_is_case_insensitive_and_trims_name() {
        let track = track(SourceId::Bilibili, "Jay Chou", "晴天");
        assert_eq!(matches_artist(&track, "  jay  "), MatchKind::Author);
    }

    #[test]
    fn both_keyword_and_artist_require_title_keyword_on_bilibili() {
        let track = track(SourceId::Bilibili, "明前奶绿", "别的视频");
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("不为谁而作的歌")),
            MatchKind::None
        );
    }

    #[test]
    fn both_keyword_and_artist_keep_matching_title_on_bilibili() {
        let track = track(
            SourceId::Bilibili,
            "切片搬运号",
            "【明前奶绿】不为谁而作的歌 翻唱",
        );
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("不为谁而作的歌")),
            MatchKind::Title
        );
    }

    #[test]
    fn title_artist_without_keyword_is_filtered_when_both_given() {
        let track = track(SourceId::Bilibili, "切片搬运号", "【明前奶绿】其他歌");
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("不为谁而作的歌")),
            MatchKind::None
        );
    }

    #[test]
    fn artist_only_does_not_require_title_keyword() {
        let track = track(SourceId::Bilibili, "切片搬运号", "【明前奶绿】其他歌");
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", None),
            MatchKind::Title
        );
    }

    #[test]
    fn netease_ignores_keyword_constraint() {
        let track = track(SourceId::Netease, "明前奶绿", "别的歌名");
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("不为谁而作的歌")),
            MatchKind::Author
        );
    }

    #[test]
    fn keyword_constraint_is_case_insensitive() {
        let track = track(SourceId::Bilibili, "切片号", "【明前奶绿】QING TIAN");
        assert_eq!(
            matches_artist_in_query(&track, "明前奶绿", Some("qing tian")),
            MatchKind::Title
        );
    }

    #[test]
    fn empty_name_never_matches() {
        let track = track(SourceId::Bilibili, "明前奶绿", "明前奶绿");
        assert_eq!(matches_artist(&track, "   "), MatchKind::None);
    }
}
