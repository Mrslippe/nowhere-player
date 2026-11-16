//! 直播链路的可观测计数。
//!
//! TUI 只需要 `buffer_ms()`（LIVE 状态栏）；drop / underrun / decode error 是
//! B1.2 验收和排障要记录的数字。句柄可以脱离 source 单独持有，`Player` 内部
//! 的解码线程与 UI 线程通过原子变量通信，不加锁。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
struct Counters {
    buffer_ms: AtomicU64,
    drop_events: AtomicU64,
    dropped_frames: AtomicU64,
    underruns: AtomicU64,
    decode_errors: AtomicU64,
    frames_received: AtomicU64,
    video_tags: AtomicU64,
}

/// 一路直播的只读计数器句柄（Clone 后共享同一份数据）。
#[derive(Debug, Clone, Default)]
pub struct LiveMetrics {
    counters: Arc<Counters>,
}

impl LiveMetrics {
    /// 当前队列里还没播的音频长度（毫秒）。
    pub fn buffer_ms(&self) -> u64 {
        self.counters.buffer_ms.load(Ordering::Relaxed)
    }

    /// 超过上限后触发丢旧帧的次数。
    pub fn drop_events(&self) -> u64 {
        self.counters.drop_events.load(Ordering::Relaxed)
    }

    /// 累计丢掉的 AAC 帧数。
    pub fn dropped_frames(&self) -> u64 {
        self.counters.dropped_frames.load(Ordering::Relaxed)
    }

    /// 队列饥饿（补静音）的段数。
    pub fn underruns(&self) -> u64 {
        self.counters.underruns.load(Ordering::Relaxed)
    }

    /// AAC 解码失败次数。
    pub fn decode_errors(&self) -> u64 {
        self.counters.decode_errors.load(Ordering::Relaxed)
    }

    /// 从网络收到的 raw AAC 帧数。
    pub fn frames_received(&self) -> u64 {
        self.counters.frames_received.load(Ordering::Relaxed)
    }

    /// `only_audio=1` 被忽略时收到的视频 tag 数（只计数丢弃）。
    pub fn video_tags(&self) -> u64 {
        self.counters.video_tags.load(Ordering::Relaxed)
    }

    pub(crate) fn set_buffer(&self, milliseconds: u64) {
        self.counters
            .buffer_ms
            .store(milliseconds, Ordering::Relaxed);
    }

    pub(crate) fn note_drop(&self, frames: u64) {
        self.counters.drop_events.fetch_add(1, Ordering::Relaxed);
        self.counters
            .dropped_frames
            .fetch_add(frames, Ordering::Relaxed);
    }

    pub(crate) fn note_underrun(&self) {
        self.counters.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_decode_error(&self) {
        self.counters.decode_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_frame(&self) {
        self.counters
            .frames_received
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_video_tag(&self) {
        self.counters.video_tags.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_shares_the_same_counters() {
        let metrics = LiveMetrics::default();
        let clone = metrics.clone();
        clone.set_buffer(900);
        clone.note_drop(3);
        clone.note_underrun();
        clone.note_decode_error();
        clone.note_frame();
        clone.note_video_tag();
        assert_eq!(metrics.buffer_ms(), 900);
        assert_eq!(metrics.dropped_frames(), 3);
        assert_eq!(metrics.drop_events(), 1);
        assert_eq!(metrics.underruns(), 1);
        assert_eq!(metrics.decode_errors(), 1);
        assert_eq!(metrics.frames_received(), 1);
        assert_eq!(metrics.video_tags(), 1);
    }
}
