//! 领域模型、命令定义、事件与错误码

pub mod command;
pub mod player;
pub mod queue;
pub mod terminal;

pub use command::{CommandErrorCode, ParseError};
pub use player::{
    AudioFailure, POSITION_TICK_MS, PlaybackMachine, PlaybackMode, PlaybackState, PlaybackStatus,
    PlayerCommand, PlayerError, PlayerErrorCode, PlayerEvent, PositionTick, RepeatMode,
};
pub use queue::{Queue, QueueStep, QueueView};
pub use terminal::{Block, BlockStatus, MediaKind, Output, SCHEMA_VERSION, Span, SpanColor, Tone};
