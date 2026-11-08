//! 领域模型、命令定义、事件与错误码

pub mod player;
pub mod terminal;

pub use player::{
    AudioFailure, POSITION_TICK_MS, PlaybackMachine, PlaybackMode, PlaybackState, PlaybackStatus,
    PlayerCommand, PlayerError, PlayerErrorCode, PlayerEvent, PositionTick, RepeatMode,
};
pub use terminal::{Block, BlockStatus, MediaKind, Output, SCHEMA_VERSION, Span, SpanColor, Tone};
