use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingSource {
    Search,
    Queue,
    Library,
    Playlist(String),
}

impl ListingSource {
    /// 终端文案；越界错误也用它。
    pub fn label(&self) -> String {
        match self {
            Self::Search => "搜索结果".to_owned(),
            Self::Queue => "播放队列".to_owned(),
            Self::Library => "媒体库".to_owned(),
            Self::Playlist(name) => format!("歌单《{name}》"),
        }
    }
}

/// 会话级序号上下文：来源 + 当时渲染的行数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveListing {
    pub source: ListingSource,
    pub len: usize,
}

/// 搜索结果硬上限（T-0023 §6.4）；`--limit` 也不能超过它。
pub const MAX_SEARCH_RESULTS: usize = 100;

/// 默认搜索结果总数（T-0046 C）。
pub const DEFAULT_SEARCH_RESULTS: usize = 20;

/// 默认每个在线源取多少条；两个源正好凑成默认总数 20。
pub const ONLINE_SEARCH_PER_SOURCE: usize = 10;

/// `--up` 无关键词时的最近投稿页大小（T-0049）。
pub const UP_ARCHIVE_PAGE_SIZE: usize = 30;

/// 一次 `search` 的解析结果（T-0046 D）。
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// 实际发给源的词；只给 `--artist` 时等于艺人名。
    pub keyword: String,
    pub artist: Option<String>,
    /// `None` = all（含本地 + 全部在线源）。
    pub source: Option<SourceId>,
    pub limit: usize,
    pub local: bool,
    pub explicit_limit: bool,
    /// 用户是否真的给了关键词（只给 artist 时 keyword 会填 artist，但不算关键词）。
    pub has_keyword: bool,
    /// `--up <用户名>`；与 `mid` 互斥。
    pub up: Option<String>,
    /// `--mid <id>`；跳过用户名解析。
    pub mid: Option<u64>,
    /// `--no-filter`：关闭 artist 过滤与标题关键词约束。
    pub no_filter: bool,
}

impl SearchOptions {
    pub fn parse(parsed: &Parsed) -> Result<Self, CmdError> {
        let local = parsed.flags.contains_key("local");
        let no_filter = parsed.flags.contains_key("no-filter");
        let up = match parsed.flags.get("up") {
            Some(raw) if raw.trim().is_empty() => {
                return Err(CmdError::new("E_CMD_ARG", "--up 不能为空"));
            }
            Some(raw) => Some(raw.trim().to_owned()),
            None => None,
        };
        let mid = match parsed.flags.get("mid") {
            Some(raw) => {
                let value = raw
                    .parse::<u64>()
                    .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的 --mid：{raw}")))?;
                if value == 0 {
                    return Err(CmdError::new("E_CMD_ARG", "--mid 必须大于 0"));
                }
                Some(value)
            }
            None => None,
        };
        if up.is_some() && mid.is_some() {
            return Err(CmdError::new("E_CMD_ARG", "search --up 与 --mid 互斥")
                .with_hint("--up 按用户名解析；--mid 直接指定"));
        }
        let source_raw = parsed.flags.get("source");
        if local && source_raw.is_some() {
            return Err(
                CmdError::new("E_CMD_ARG", "search --source 与 --local 互斥")
                    .with_hint("--local 只搜本地库；--source 指定在线源"),
            );
        }
        let source = match source_raw.map(String::as_str) {
            None | Some("all") => None,
            Some("bilibili") => Some(SourceId::Bilibili),
            Some("netease") => Some(SourceId::Netease),
            Some(other) => {
                return Err(CmdError::new("E_CMD_ARG", format!("未知音源：{other}"))
                    .with_hint("可用：all / bilibili / netease"));
            }
        };
        if up.is_some() || mid.is_some() {
            if local {
                return Err(
                    CmdError::new("E_CMD_ARG", "search --up/--mid 与 --local 互斥")
                        .with_hint("UP 检索只支持 B 站在线"),
                );
            }
            if parsed.flags.contains_key("artist") {
                return Err(
                    CmdError::new("E_CMD_ARG", "search --up/--mid 与 --artist 互斥")
                        .with_hint("--up 已在指定人；关键词语义由空间接口处理"),
                );
            }
            if source == Some(SourceId::Netease) {
                return Err(CmdError::new("E_CMD_ARG", "search --up/--mid 只支持 B 站")
                    .with_hint("--source 改为 all 或 bilibili"));
            }
        }

        let explicit_limit = parsed.flags.contains_key("limit");
        let limit = if let Some(raw) = parsed.flags.get("limit") {
            let value = raw.parse::<usize>().map_err(|_| {
                CmdError::new("E_CMD_ARG", format!("无效的 --limit：{raw}"))
                    .with_hint("--limit 取值范围 1-100")
            })?;
            if !(1..=MAX_SEARCH_RESULTS).contains(&value) {
                return Err(
                    CmdError::new("E_CMD_ARG", format!("--limit 超出范围：{value}"))
                        .with_hint("--limit 取值范围 1-100"),
                );
            }
            value
        } else {
            DEFAULT_SEARCH_RESULTS
        };

        let artist = match parsed.flags.get("artist") {
            Some(raw) if raw.trim().is_empty() => {
                return Err(CmdError::new("E_CMD_ARG", "--artist 不能为空"));
            }
            Some(raw) => Some(raw.trim().to_owned()),
            None => None,
        };
        let raw_keyword = parsed
            .positional
            .first()
            .map(|keyword| keyword.trim())
            .unwrap_or_default();
        let has_keyword = !raw_keyword.is_empty();
        let keyword = if raw_keyword.is_empty() {
            artist.clone().unwrap_or_default()
        } else {
            raw_keyword.to_owned()
        };
        if keyword.is_empty() && up.is_none() && mid.is_none() {
            return Err(CmdError::new("E_CMD_ARG", "搜索关键词不能为空")
                .with_hint("search <关键词>、search --artist <名> 或 search --up <用户名>"));
        }

        Ok(Self {
            keyword,
            artist,
            source,
            limit,
            local,
            explicit_limit,
            has_keyword,
            up,
            mid,
            no_filter,
        })
    }

    #[cfg(test)]
    pub(crate) fn legacy(keyword: &str, limit: usize) -> Self {
        Self {
            keyword: keyword.to_owned(),
            artist: None,
            source: None,
            limit,
            local: false,
            explicit_limit: true,
            has_keyword: !keyword.trim().is_empty(),
            up: None,
            mid: None,
            no_filter: false,
        }
    }

    pub(crate) fn per_source(&self) -> usize {
        if self.explicit_limit {
            self.limit
        } else {
            ONLINE_SEARCH_PER_SOURCE
        }
    }

    /// 默认配额下本地最多占 10 条，剩下留给在线；显式 `--limit` 不额外压本地。
    pub(crate) fn local_cap(&self) -> usize {
        if self.explicit_limit {
            self.limit
        } else {
            ONLINE_SEARCH_PER_SOURCE
        }
    }

    /// 带 `--artist` 时扩大候选池；无 artist 维持现有配额。
    pub(crate) fn candidate_pool(&self) -> usize {
        if self.artist.is_some() {
            self.limit.saturating_mul(3).clamp(1, 50)
        } else {
            self.per_source()
        }
    }

    /// 带 artist 时本地也扩大抓取，过滤后再截断。
    pub(crate) fn local_fetch_limit(&self) -> usize {
        if self.artist.is_some() {
            self.limit.saturating_mul(3).clamp(1, 50)
        } else {
            self.limit.saturating_add(1)
        }
    }

    /// 发给在线源的 query；构造规则集中在 `np-sources`。
    pub(crate) fn source_query(&self) -> String {
        build_query(&self.keyword, self.artist.as_deref())
    }

    /// 同时给关键词与 artist 时，B 站标题必须含关键词；只给 artist 时返回 None。
    pub(crate) fn keyword_filter(&self) -> Option<&str> {
        if self.artist.is_some() && self.has_keyword {
            Some(self.keyword.as_str())
        } else {
            None
        }
    }

    pub(crate) fn footer(&self, count: usize, with_source_column: bool) -> String {
        let hint = if self.local {
            if self.explicit_limit {
                format!("--limit {}", self.limit)
            } else {
                "--limit 可调".to_owned()
            }
        } else if self.explicit_limit {
            format!("--limit {}", self.limit)
        } else {
            format!("每源 {}，--limit 可调", self.per_source())
        };
        let source = if with_source_column {
            "（来源见「来源」列）"
        } else {
            ""
        };
        format!("共 {count} 条（{hint}） · 输入 play <序号> 播放{source}")
    }
}

/// 会话上下文里的一条搜索结果：本地曲目或在线曲目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchHit {
    /// 本地库曲目。
    Local(Track),
    /// 在线源曲目。
    Online(SourceTrack),
}

impl SearchHit {
    pub(crate) fn title(&self) -> &str {
        match self {
            Self::Local(track) => &track.title,
            Self::Online(track) => &track.title,
        }
    }

    pub(crate) fn artist(&self) -> &str {
        match self {
            Self::Local(track) => &track.artist,
            Self::Online(track) => &track.artist,
        }
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        match self {
            Self::Local(track) => u64::try_from(track.duration_ms).unwrap_or(0),
            Self::Online(track) => track.duration_ms,
        }
    }

    pub(crate) fn source_label(&self) -> &'static str {
        match self {
            Self::Local(_) => "本地",
            Self::Online(track) => source_label(track.source),
        }
    }

    pub(crate) fn artist_match(&self, name: &str, keyword: Option<&str>) -> MatchKind {
        match self {
            Self::Local(track) => local_artist_match(track, name),
            Self::Online(track) => match_source_artist_in_query(track, name, keyword),
        }
    }
}

/// 本地库的 artist 是结构化字段，只比它，不比标题。
pub(crate) fn local_artist_match(track: &Track, name: &str) -> MatchKind {
    let wanted = name.trim().to_lowercase();
    if wanted.is_empty() {
        MatchKind::None
    } else if track.artist.to_lowercase().contains(&wanted) {
        MatchKind::Author
    } else {
        MatchKind::None
    }
}
