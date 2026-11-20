use std::path::PathBuf;

use np_app::{PlaybackTarget, SourceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchDisplay {
    Empty { query: String },
    Searching,
    NoResults,
}

/// 全局搜索的平台过滤；Tab 在三种状态间循环，不改变已输入关键词。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SearchPlatform {
    #[default]
    All,
    Netease,
    Bilibili,
}

impl SearchPlatform {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Netease,
            Self::Netease => Self::Bilibili,
            Self::Bilibili => Self::All,
        }
    }

    /// 与命令层 `-source` 的短名一致；嵌在 Key Words 输入框下边框。
    pub(crate) fn id_label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Netease => "netease",
            Self::Bilibili => "bilibili",
        }
    }

    pub(crate) fn source(self) -> Option<SourceId> {
        match self {
            Self::All => None,
            Self::Netease => Some(SourceId::Netease),
            Self::Bilibili => Some(SourceId::Bilibili),
        }
    }

    pub(crate) fn from_source(source: Option<SourceId>) -> Self {
        match source {
            Some(SourceId::Netease) => Self::Netease,
            Some(SourceId::Bilibili) => Self::Bilibili,
            _ => Self::All,
        }
    }
}

/// 播放模式在 TUI 里的短符号：顺序用双右箭头，避免 `·` 被误读成占位符。
pub(crate) fn playback_mode_symbol(id: &str) -> &'static str {
    match id {
        "sequential" => "⇉",
        "list" => "↻",
        "single" => "↺",
        "shuffle" => "⇄",
        _ => "·",
    }
}

/// Player 视觉样式；顺序即 `f` 的循环顺序，id 用于 `ui.visual` 与
/// `NP_TUI_VISUAL`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VisualMode {
    #[default]
    Spectrum,
    Bars,
    Waveform,
    Mirror,
    Vectorscope,
    Rings,
    Starfield,
}

impl VisualMode {
    /// 固定顺序：`f`、设置页 `←→` 与 id 往返共用，避免各写一份循环表。
    pub(crate) const ALL: [Self; 7] = [
        Self::Spectrum,
        Self::Bars,
        Self::Waveform,
        Self::Mirror,
        Self::Vectorscope,
        Self::Rings,
        Self::Starfield,
    ];

    pub(crate) fn next(self) -> Self {
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// 持久化 id；只接受正式取值。
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Spectrum => "spectrum",
            Self::Bars => "bars",
            Self::Waveform => "waveform",
            Self::Mirror => "mirror",
            Self::Vectorscope => "vectorscope",
            Self::Rings => "rings",
            Self::Starfield => "starfield",
        }
    }

    /// 中文短名：状态栏与设置页值列共用。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "频谱",
            Self::Bars => "柱状",
            Self::Waveform => "波形",
            Self::Mirror => "镜像",
            Self::Vectorscope => "矢量",
            Self::Rings => "环形",
            Self::Starfield => "星野",
        }
    }

    /// 只接受正式 id；旧别名 `wave` / `vector` 只在 `NP_TUI_VISUAL` 解析里兼容。
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|mode| mode.id() == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowseKind {
    PlaylistRoot,
    Playlist(String),
    Library,
    Search,
    Queue,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Browse,
    List,
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackRef {
    Local(PathBuf),
    Remote { source: String, id: String },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ViewData {
    pub(crate) title: String,
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) refs: Vec<TrackRef>,
    pub(crate) durations: Vec<u64>,
}

pub(crate) struct RemotePlayback {
    pub(crate) tx: std::sync::mpsc::Sender<String>,
}

impl PlaybackTarget for RemotePlayback {
    fn play_path(&self, path: &str) -> Result<np_core::PlaybackState, np_app::CmdError> {
        let _ = self.tx.send(path.to_owned());
        Ok(np_core::PlaybackState::default())
    }
}
