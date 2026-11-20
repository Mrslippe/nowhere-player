//! 暂停/恢复控制：连接保持、暂停缓存上限、恢复时先播缓存。
//!
//! 暂停时不 drop `Player`，只让 decode 线程停住；网络线程继续读 FLV，
//! 但暂停缓存超过 [`PAUSE_CAP_MS`] 后丢弃**新**帧，保留暂停起点。
//! 恢复时 source 先播缓存到 `replay_until`，之后才恢复 1500ms 控制器。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 暂停缓存上限：30s 音频约 0.5MB。
pub const PAUSE_CAP_MS: u64 = 30_000;

/// AAC-LC 每帧 1024 samples，48k 下 30s 约 1406 帧。
/// 同时按帧数封顶，避免 FLV timestamp 跳变让毫秒 cap 失效。
pub(crate) const PAUSE_CAP_FRAMES: usize = 1_406;

#[derive(Debug, Default)]
struct State {
    paused: AtomicBool,
    replay: AtomicBool,
    refresh: AtomicBool,
    paused_dropped_frames: AtomicU64,
}

/// 共享给生产者、解码线程和 TUI 的控制句柄。
#[derive(Debug, Clone, Default)]
pub struct LiveControl {
    state: Arc<State>,
}

impl LiveControl {
    /// 进入暂停：生产者继续读，但超过 cap 后丢新帧。
    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::Release);
    }

    /// 恢复：先播缓存；source 会在下一次 read 记录 replay 边界。
    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::Release);
        self.state.replay.store(true, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Acquire)
    }

    /// 解码线程取一次“从这里开始 replay”。
    pub(crate) fn take_replay(&self) -> bool {
        self.state.replay.swap(false, Ordering::AcqRel)
    }

    /// 本地刷新：丢弃旧队列，从最新缓冲前约 300ms 重新起播。
    pub fn request_refresh(&self) {
        self.state.refresh.store(true, Ordering::Release);
    }

    pub(crate) fn take_refresh(&self) -> bool {
        self.state.refresh.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn note_paused_drop(&self) {
        self.state
            .paused_dropped_frames
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 暂停期间因 cap 丢弃的新帧数。
    pub fn paused_dropped_frames(&self) -> u64 {
        self.state.paused_dropped_frames.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_resume_sets_replay_once() {
        let control = LiveControl::default();
        assert!(!control.is_paused());
        control.pause();
        assert!(control.is_paused());
        control.resume();
        assert!(!control.is_paused());
        assert!(control.take_replay());
        assert!(!control.take_replay());
        control.request_refresh();
        assert!(control.take_refresh());
        assert!(!control.take_refresh());
        control.note_paused_drop();
        assert_eq!(control.paused_dropped_frames(), 1);
    }
}
