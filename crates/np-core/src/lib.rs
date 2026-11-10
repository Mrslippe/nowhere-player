//! 领域模型、命令定义、事件与错误码

pub mod command;
pub mod player;
pub mod queue;
pub mod settings;
pub mod terminal;
pub mod theme;

pub use command::{
    ArgKind, ArgSpec, CommandErrorCode, CommandSpec, Completion, CompletionItem, CompletionKind,
    FlagSpec, ParseError, Parsed, Registry, SubcommandSpec, complete, parse,
};
pub use player::{
    AudioFailure, POSITION_TICK_MS, PlaybackMachine, PlaybackMode, PlaybackState, PlaybackStatus,
    PlayerCommand, PlayerError, PlayerErrorCode, PlayerEvent, PositionTick, RepeatMode,
};
pub use queue::{Queue, QueueStep, QueueView};
pub use settings::{
    SETTINGS_BACKUP_FILE, SETTINGS_FILE, SettingValue, Settings, SettingsError, SettingsWarning,
    settings_path, validate_setting,
};
pub use terminal::{
    Block, BlockAction, BlockStatus, MediaKind, Output, SCHEMA_VERSION, Span, SpanColor, Tone,
};
pub use theme::{
    BUILTIN_THEMES, BuiltinTheme, MAX_THEME_ID_LEN, MAX_THEME_NAME_CHARS, RawTheme, Skin,
    THEME_CONTRACT, THEME_TOKEN_NAMES, ThemeApplyReply, ThemeError, ThemeListEntry, ThemeSource,
    ThemeSpec, ThemeTokens, is_valid_theme_color, validate_theme_id,
};
