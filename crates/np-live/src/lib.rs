//! B 站直播音频最小闭环（B1.2）。
//!
//! 边界见 ADR-0008：本 crate 只负责直播 API、HTTP-FLV → raw AAC 帧、AAC 直解
//! 与 900ms 缓冲队列，以及把直播流包装成 [`np_audio::AudioSource`]。播放、队列
//! 和界面留在 `np-tui`。
//!
//! 与 B1 的区别：不再经过 `AdtsReader + MediaSourceStream(64KB)`，而是网络线程
//! 产出 `AacFrame`、解码线程用 `symphonia` 的 `AacDecoder` 直接解 raw AAC，中间
//! 只有一个目标 900ms（上限 1400ms、下限 500ms）的有界帧队列，缓冲可观测。

pub mod api;
pub mod error;
mod flv;
mod metrics;
pub mod prepare;
mod queue;
mod source;

pub use error::LiveError;
pub use metrics::LiveMetrics;
pub use prepare::{PreparedLive, prepare, prepare_default};
