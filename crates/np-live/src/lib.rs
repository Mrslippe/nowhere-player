//! B 站直播音频最小闭环（B1.2）。
//!
//! 边界见 ADR-0008：本 crate 只负责直播 API、HTTP-FLV → raw AAC 帧、AAC 直解
//! 与可调目标缓冲（默认 900ms、WSOLA 追帧），以及把直播流包装成
//! [`np_audio::AudioSource`]。播放、队列和界面留在 `np-tui`。
//!
//! 与 B1 的区别：不再经过 `AdtsReader + MediaSourceStream(64KB)`，而是网络线程
//! 产出 `AacFrame`、解码线程用 `symphonia` 的 `AacDecoder` 直接解 raw AAC，中间
//! 只有一个目标可配（默认 900ms、硬上限 target+500ms）的有界帧队列，缓冲可观测。
//!
//! 目标缓冲的宿主入口是 [`set_target_buffer_ms`]：np-tui 在每次 `/live` 前用
//! `live.buffer_ms` 的值调用；`NP_LIVE_TARGET_MS` 保留为调试覆盖。

pub mod api;
pub mod auth;
mod control;
pub mod danmaku;
pub mod danmaku_client;
pub mod danmaku_protocol;
pub mod error;
mod flv;
mod metrics;
pub mod prepare;
mod queue;
mod source;

pub use auth::{AuthError, AuthStore, BiliAuth, QR_POLL_INTERVAL, QrLogin, QrStatus};
pub use control::LiveControl;
pub use danmaku::{DANMAKU_MIN_INTERVAL, DanmakuError, send_danmaku};
pub use danmaku_client::{
    CLIENT_PROTOVER, DanmakuHost, DanmakuWsError, DanmuInfo, HEARTBEAT_INTERVAL, IDLE_TIMEOUT,
    danmu_info_url, get_danmu_info, get_danmu_info_with_auth, parse_danmu_info, run_danmaku_ws,
};
pub use danmaku_protocol::DanmakuMessage;
pub use error::LiveError;
pub use metrics::LiveMetrics;
pub use prepare::{PreparedLive, prepare, prepare_auth, prepare_default, prepare_default_auth};
pub use queue::{
    DEFAULT_TARGET_BUFFER_MS, MAX_TARGET_BUFFER_MS, MIN_TARGET_BUFFER_MS, clear_target_buffer_ms,
    set_target_buffer_ms,
};
