//! 直播链路的可观测计数。
//!
//! TUI 只需要 `buffer_ms()`（LIVE 状态栏）；drop / underrun / decode error 是
//! B1.2 验收和排障要记录的数字。句柄可以脱离 source 单独持有，`Player` 内部
//! 的解码线程与 UI 线程通过原子变量通信，不加锁。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct Counters {
    buffer_ms: AtomicU64,
    min_buffer_ms: AtomicU64,
    max_buffer_ms: AtomicU64,
    drop_events: AtomicU64,
    dropped_frames: AtomicU64,
    underruns: AtomicU64,
    queue_empty_events: AtomicU64,
    fill_silence_ms: AtomicU64,
    max_read_stall_ms: AtomicU64,
    decode_errors: AtomicU64,
    frames_received: AtomicU64,
    video_tags: AtomicU64,
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            buffer_ms: AtomicU64::new(0),
            min_buffer_ms: AtomicU64::new(u64::MAX),
            max_buffer_ms: AtomicU64::new(0),
            drop_events: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            queue_empty_events: AtomicU64::new(0),
            fill_silence_ms: AtomicU64::new(0),
            max_read_stall_ms: AtomicU64::new(0),
            decode_errors: AtomicU64::new(0),
            frames_received: AtomicU64::new(0),
            video_tags: AtomicU64::new(0),
        }
    }
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

    /// `queue.pop()` 返回空、但流尚未结束的次数。
    pub fn queue_empty_events(&self) -> u64 {
        self.counters.queue_empty_events.load(Ordering::Relaxed)
    }

    /// 累计补静音的毫秒数。
    pub fn fill_silence_ms(&self) -> u64 {
        self.counters.fill_silence_ms.load(Ordering::Relaxed)
    }

    /// 单次 `FlvFrameSource::next_frame()` 的最长墙钟耗时（含阻塞 HTTP read）。
    pub fn max_read_stall_ms(&self) -> u64 {
        self.counters.max_read_stall_ms.load(Ordering::Relaxed)
    }

    /// 见过的队列 buffer 最小值（毫秒）；还没采样时为 0。
    pub fn min_buffer_ms(&self) -> u64 {
        let value = self.counters.min_buffer_ms.load(Ordering::Relaxed);
        if value == u64::MAX { 0 } else { value }
    }

    /// 见过的队列 buffer 最大值（毫秒）。
    pub fn max_buffer_ms(&self) -> u64 {
        self.counters.max_buffer_ms.load(Ordering::Relaxed)
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
        self.counters
            .min_buffer_ms
            .fetch_min(milliseconds, Ordering::Relaxed);
        self.counters
            .max_buffer_ms
            .fetch_max(milliseconds, Ordering::Relaxed);
    }

    pub(crate) fn note_queue_empty(&self) {
        self.counters
            .queue_empty_events
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_fill_silence(&self, milliseconds: u64) {
        self.counters
            .fill_silence_ms
            .fetch_add(milliseconds, Ordering::Relaxed);
    }

    pub(crate) fn note_read_stall(&self, milliseconds: u64) {
        self.counters
            .max_read_stall_ms
            .fetch_max(milliseconds, Ordering::Relaxed);
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
        clone.set_buffer(1_200);
        clone.set_buffer(700);
        clone.note_drop(3);
        clone.note_underrun();
        clone.note_queue_empty();
        clone.note_fill_silence(21);
        clone.note_read_stall(1_050);
        clone.note_decode_error();
        clone.note_frame();
        clone.note_video_tag();
        assert_eq!(metrics.buffer_ms(), 700);
        assert_eq!(metrics.min_buffer_ms(), 700);
        assert_eq!(metrics.max_buffer_ms(), 1_200);
        assert_eq!(metrics.dropped_frames(), 3);
        assert_eq!(metrics.drop_events(), 1);
        assert_eq!(metrics.underruns(), 1);
        assert_eq!(metrics.queue_empty_events(), 1);
        assert_eq!(metrics.fill_silence_ms(), 21);
        assert_eq!(metrics.max_read_stall_ms(), 1_050);
        assert_eq!(metrics.decode_errors(), 1);
        assert_eq!(metrics.frames_received(), 1);
        assert_eq!(metrics.video_tags(), 1);
    }
}
