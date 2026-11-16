//! B 站直播音频最小闭环（B1）。
//!
//! 边界见 ADR-0008：本 crate 只负责直播 API、HTTP-FLV → ADTS、以及把直播流
//! 包装成 [`np_audio::AudioSource`]。播放、队列和界面留在 `np-tui`。

pub mod api;
pub mod error;
pub mod flv;
pub mod prepare;
pub mod source;

pub use error::LiveError;
pub use prepare::{PreparedLive, prepare, prepare_default};
