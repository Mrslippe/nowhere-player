//! 有界 `AacFrame` 队列 + 1500ms 缓冲控制器。
//!
//! 队列是网络线程（producer）和解码线程（consumer）之间唯一的中转：
//!
//! - 网络线程按 FLV tag 到一帧就 push；队列满时等待，不丢数据；
//! - 解码线程按 AAC 帧 pop，落后太多时由控制器从队头丢旧帧；
//! - `buffer_ms = 最新帧 pts - 已播放媒体时间`。
//!
//! 目标 1500ms / 上限 2000ms / 下限 1200ms 是 B1.2g 稳定降延迟值：
//! 低 buffer 只用于诊断，不再阻塞 decode 线程；队列空才补静音。

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::flv::AacFrame;

/// 缓冲目标（毫秒）：超过上限后追到这个值。
pub(crate) const TARGET_BUFFER_MS: u64 = 1500;
/// 缓冲上限：超过才丢旧帧。
pub(crate) const MAX_BUFFER_MS: u64 = 2000;
/// 缓冲下限：低于它只作为诊断标记，不阻塞 decode 线程。
pub(crate) const MIN_BUFFER_MS: u64 = 1200;
/// 控制周期：250ms 检查一次缓冲，不必每帧都判断。
pub(crate) const CONTROL_INTERVAL_MS: u64 = 250;
/// 一次丢帧后的冷却时间，避免连续跳变。
pub(crate) const DROP_COOLDOWN_MS: u64 = 500;
/// 帧队列硬上限：约 11s 音频，只作为网络突发时的安全带。
pub(crate) const QUEUE_CAPACITY_FRAMES: usize = 512;

#[derive(Default)]
struct QueueState {
    frames: VecDeque<AacFrame>,
    newest_pts: Option<u64>,
    done: bool,
    stop: bool,
    error: Option<String>,
    overflow_waits: u64,
}

struct Shared {
    state: Mutex<QueueState>,
    cv: Condvar,
}

/// 生产者 / 消费者共享的帧队列。
#[derive(Clone)]
pub(crate) struct FrameQueue {
    shared: Arc<Shared>,
    capacity: usize,
}

impl FrameQueue {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(QueueState::default()),
                cv: Condvar::new(),
            }),
            capacity: capacity.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wake(&self) {
        self.shared.cv.notify_all();
    }

    /// 入队；队列满时等待消费者腾位置。`false` 表示整个 source 已被要求停止。
    pub(crate) fn push(&self, frame: AacFrame) -> bool {
        let mut state = self.lock();
        while state.frames.len() >= self.capacity && !state.stop {
            state.overflow_waits = state.overflow_waits.saturating_add(1);
            let (guard, _) = self
                .shared
                .cv
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = guard;
        }
        if state.stop {
            return false;
        }
        state.newest_pts = Some(frame.pts_ms);
        state.frames.push_back(frame);
        drop(state);
        self.wake();
        true
    }

    pub(crate) fn pop(&self) -> Option<AacFrame> {
        let frame = self.lock().frames.pop_front();
        if frame.is_some() {
            self.wake();
        }
        frame
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().frames.len()
    }

    pub(crate) fn newest_pts(&self) -> Option<u64> {
        self.lock().newest_pts
    }

    pub(crate) fn front_pts(&self) -> Option<u64> {
        self.lock().frames.front().map(|frame| frame.pts_ms)
    }

    /// 队列里还没解码的音频长度（毫秒）= 最新帧 pts - 队首帧 pts。
    ///
    /// 只用流内 pts，不依赖“已解码样本数”，长时间播放不会因为每帧
    /// 21.33ms 与整数 pts 的舍入差而漂移。
    pub(crate) fn buffer_ms(&self) -> u64 {
        span_of(&self.lock()).unwrap_or(0)
    }

    /// 等待缓冲跨度达到 `min_span_ms`。返回 `true` 表示达到；
    /// `false` 表示流已结束或超时（调用方用 `len()` 判断能不能播）。
    pub(crate) fn wait_span(&self, min_span_ms: u64, deadline: Instant) -> bool {
        let mut state = self.lock();
        loop {
            if span_of(&state).is_some_and(|span| span >= min_span_ms) {
                return true;
            }
            if state.done {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(20));
            let (guard, _) = self
                .shared
                .cv
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = guard;
        }
    }

    /// 从队头丢掉 `pts_ms < threshold_ms` 的旧帧，至少保留一帧。
    pub(crate) fn drop_below(&self, threshold_ms: u64) -> usize {
        let mut state = self.lock();
        let mut dropped = 0;
        loop {
            if state.frames.len() <= 1 {
                break;
            }
            let keep = match state.frames.front() {
                Some(front) => front.pts_ms >= threshold_ms,
                None => true,
            };
            if keep {
                break;
            }
            state.frames.pop_front();
            dropped += 1;
        }
        if dropped > 0 {
            drop(state);
            self.wake();
        }
        dropped
    }

    pub(crate) fn is_done(&self) -> bool {
        self.lock().done
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    /// 网络线程干净结束。
    pub(crate) fn finish(&self) {
        let mut state = self.lock();
        state.done = true;
        drop(state);
        self.wake();
    }

    /// 网络线程出错结束；错误文本留给解码线程转成 `AudioError`。
    pub(crate) fn fail(&self, message: String) {
        let mut state = self.lock();
        state.done = true;
        state.error = Some(message);
        drop(state);
        self.wake();
    }

    /// 通知网络线程退出（`Drop` / 启动失败），不 join。
    pub(crate) fn stop(&self) {
        let mut state = self.lock();
        state.stop = true;
        drop(state);
        self.wake();
    }
}

fn span_of(state: &QueueState) -> Option<u64> {
    let front = state.frames.front()?.pts_ms;
    state.newest_pts.map(|newest| newest.saturating_sub(front))
}

/// 一次控制周期的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    /// 不需要动作。
    Idle,
    /// 追到目标缓冲（调用方按最新 pts 计算阈值）。
    DropTo(u64),
    /// 缓冲回到下限以上，结束一次饥饿。
    Recovered,
}

/// 250ms 周期的缓冲控制器。
pub(crate) struct BufferController {
    last_tick: Option<Instant>,
    last_drop: Option<Instant>,
    starving: bool,
}

impl BufferController {
    pub(crate) fn new() -> Self {
        Self {
            last_tick: None,
            last_drop: None,
            starving: false,
        }
    }

    /// 距上次控制周期是否已满 250ms。
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.last_tick.is_none_or(|last| {
            now.saturating_duration_since(last) >= Duration::from_millis(CONTROL_INTERVAL_MS)
        })
    }

    /// 推进一次控制周期。只有 `buffer > 2000ms` 且冷却已过才丢帧；
    /// `buffer < 1200ms` 不再阻塞解码；调用方只在队列空时补静音。
    pub(crate) fn tick(&mut self, now: Instant, buffer_ms: u64) -> Control {
        self.last_tick = Some(now);

        let cooldown_ok = self.last_drop.is_none_or(|last| {
            now.saturating_duration_since(last) >= Duration::from_millis(DROP_COOLDOWN_MS)
        });
        if buffer_ms > MAX_BUFFER_MS && cooldown_ok {
            self.last_drop = Some(now);
            return Control::DropTo(TARGET_BUFFER_MS);
        }

        if self.starving && buffer_ms >= MIN_BUFFER_MS {
            self.starving = false;
            return Control::Recovered;
        }
        Control::Idle
    }

    /// 记录一次饥饿；同一段饥饿里重复调用返回 `false`（不重复计数）。
    pub(crate) fn note_starved(&mut self) -> bool {
        if self.starving {
            false
        } else {
            self.starving = true;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts_ms: u64) -> AacFrame {
        AacFrame {
            pts_ms,
            raw: vec![0u8; 8],
        }
    }

    fn fill(queue: &FrameQueue, points: &[u64]) {
        for pts in points {
            assert!(queue.push(frame(*pts)));
        }
    }

    #[test]
    fn queue_reports_buffer_as_a_pts_span() {
        let queue = FrameQueue::new(8);
        assert_eq!(queue.buffer_ms(), 0, "empty queue has no buffer");
        fill(&queue, &[0, 21, 42, 63]);
        assert_eq!(queue.len(), 4);
        assert_eq!(queue.buffer_ms(), 63);

        assert_eq!(queue.pop().map(|frame| frame.pts_ms), Some(0));
        assert_eq!(queue.buffer_ms(), 42, "buffer shrinks from the front");
        queue.drop_below(63);
        assert_eq!(queue.buffer_ms(), 0, "only the newest frame is left");
    }

    #[test]
    fn queue_blocks_at_capacity_instead_of_dropping() {
        let queue = FrameQueue::new(2);
        fill(&queue, &[0, 10]);

        let producer = queue.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let pushed = producer.push(frame(20));
            let _ = tx.send(pushed);
        });
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "push must wait while the queue is full"
        );
        assert_eq!(queue.pop().map(|frame| frame.pts_ms), Some(0));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(true));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn stop_releases_a_blocked_producer() {
        let queue = FrameQueue::new(1);
        fill(&queue, &[0]);

        let producer = queue.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(producer.push(frame(10)));
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        queue.stop();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(false));
    }

    #[test]
    fn drop_below_keeps_the_newest_frame() {
        let queue = FrameQueue::new(16);
        fill(&queue, &[0, 21, 42, 900, 921, 942]);
        let dropped = queue.drop_below(900);
        assert_eq!(dropped, 3);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.front_pts(), Some(900));
        assert_eq!(queue.newest_pts(), Some(942));

        // 阈值超过队尾时至少保留一帧。
        assert_eq!(queue.drop_below(10_000), 2);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn controller_drops_only_above_the_maximum() {
        let base = Instant::now();
        let mut controller = BufferController::new();

        assert!(controller.due(base));
        assert_eq!(controller.tick(base, TARGET_BUFFER_MS), Control::Idle);
        assert!(!controller.due(base + Duration::from_millis(100)));
        assert_eq!(
            controller.tick(base + Duration::from_millis(250), MAX_BUFFER_MS),
            Control::Idle,
            "2000ms is still acceptable"
        );
        assert_eq!(
            controller.tick(base + Duration::from_millis(500), MAX_BUFFER_MS + 1),
            Control::DropTo(TARGET_BUFFER_MS)
        );
        assert_eq!(
            controller.tick(base + Duration::from_millis(750), MAX_BUFFER_MS + 1),
            Control::Idle,
            "cooldown blocks a second drop within 500ms"
        );
        assert_eq!(
            controller.tick(base + Duration::from_millis(1_000), MAX_BUFFER_MS + 1),
            Control::DropTo(TARGET_BUFFER_MS)
        );
    }

    #[test]
    fn controller_never_drops_below_the_maximum() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        for (index, buffer) in [0u64, 300, MIN_BUFFER_MS, TARGET_BUFFER_MS, MAX_BUFFER_MS]
            .into_iter()
            .enumerate()
        {
            let now = base + Duration::from_millis(250 * index as u64);
            assert_eq!(
                controller.tick(now, buffer),
                Control::Idle,
                "buffer {buffer}"
            );
        }
    }

    #[test]
    fn controller_marks_one_starvation_per_episode() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        assert_eq!(controller.tick(base, 0), Control::Idle);
        assert!(controller.note_starved());
        assert!(!controller.note_starved(), "same episode counts once");
        assert_eq!(
            controller.tick(base + Duration::from_millis(250), MIN_BUFFER_MS),
            Control::Recovered
        );
        assert!(controller.note_starved(), "a new episode counts again");
    }
}
