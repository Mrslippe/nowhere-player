//! 有界 `AacFrame` 队列 + 自适应缓冲控制器。
//!
//! 队列是网络线程（producer）和解码线程（consumer）之间唯一的中转：
//!
//! - 网络线程按 FLV tag 到一帧就 push；队列满时等待，不丢数据；
//! - 解码线程按 AAC 帧 pop，落后太多时由控制器从队头丢旧帧；
//! - `buffer_ms = 最新帧 pts - 已播放媒体时间`。
//!
//! 目标默认 900ms（`NP_LIVE_TARGET_MS` 仅供调试覆盖）；缓冲超过 target+100ms
//! 时启动 1.05x WSOLA，超过 target+300ms 到 1.10x，回到 target 恢复 1.00x，
//! 只加速不减速。target+500ms 的硬丢帧仍保留作为兜底。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::flv::AacFrame;

use np_audio::MIN_TEMPO_SPEED;

/// 直播目标缓冲默认值（毫秒），与 `live.buffer_ms` 默认值一致。
pub const DEFAULT_TARGET_BUFFER_MS: u64 = 900;
/// `live.buffer_ms` 下限（毫秒）。
pub const MIN_TARGET_BUFFER_MS: u64 = 500;
/// `live.buffer_ms` 上限（毫秒）。
pub const MAX_TARGET_BUFFER_MS: u64 = 2000;
/// `NP_LIVE_TARGET_MS` 调试覆盖的下限，比设置宽松，方便对比测试。
const ENV_TARGET_MIN_MS: u64 = 300;
/// `NP_LIVE_TARGET_MS` 调试覆盖的上限。
const ENV_TARGET_MAX_MS: u64 = 3000;

/// 进程级目标覆盖；0 表示未设置（合法目标都 ≥ 500）。
static TARGET_OVERRIDE_MS: AtomicU64 = AtomicU64::new(0);

/// 设置进程级直播目标缓冲（毫秒），夹到 `500–2000`；下一次 `/live` 生效。
///
/// 由宿主在开始直播前用 `live.buffer_ms` 的当前值调用；`NP_LIVE_TARGET_MS`
/// 仍然优先，只用于调试。
pub fn set_target_buffer_ms(milliseconds: u64) {
    let clamped = milliseconds.clamp(MIN_TARGET_BUFFER_MS, MAX_TARGET_BUFFER_MS);
    TARGET_OVERRIDE_MS.store(clamped, Ordering::Relaxed);
}

/// 清除进程级覆盖，回到默认 900ms。
pub fn clear_target_buffer_ms() {
    TARGET_OVERRIDE_MS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
static TARGET_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 缓冲目标（毫秒）：调试 env > 进程级设置 > 默认 900。
///
/// `NP_LIVE_TARGET_MS` 仅供低延迟对比测试；设置值由
/// [`set_target_buffer_ms`] 在每次 `/live` 前写入。
pub(crate) fn target_buffer_ms() -> u64 {
    #[cfg(test)]
    let _guard = TARGET_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    target_buffer_ms_unlocked()
}

fn target_buffer_ms_unlocked() -> u64 {
    let env = std::env::var("NP_LIVE_TARGET_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    let override_ms = match TARGET_OVERRIDE_MS.load(Ordering::Relaxed) {
        0 => None,
        value => Some(value),
    };
    resolve_target_buffer_ms(env, override_ms, default_target_buffer_ms())
}

fn default_target_buffer_ms() -> u64 {
    #[cfg(test)]
    {
        TARGET_BUFFER_MS
    }
    #[cfg(not(test))]
    {
        DEFAULT_TARGET_BUFFER_MS
    }
}

/// 纯解析：`NP_LIVE_TARGET_MS`（夹 300–3000）> 进程级设置 > 默认。
fn resolve_target_buffer_ms(env: Option<u64>, override_ms: Option<u64>, default_ms: u64) -> u64 {
    if let Some(value) = env {
        return value.clamp(ENV_TARGET_MIN_MS, ENV_TARGET_MAX_MS);
    }
    override_ms.unwrap_or(default_ms)
}
/// 测试夹具目标（毫秒）；运行时默认 900。
#[cfg(test)]
pub(crate) const TARGET_BUFFER_MS: u64 = 1500;

/// 缓冲上限：超过才丢旧帧（测试夹具）。
#[cfg(test)]
pub(crate) const MAX_BUFFER_MS: u64 = 2000;
/// 缓冲下限：低于它只作为诊断标记，不阻塞 decode 线程（测试夹具）。
#[cfg(test)]
pub(crate) const MIN_BUFFER_MS: u64 = 1200;
/// 控制周期：250ms 检查一次缓冲，不必每帧都判断。
pub(crate) const CONTROL_INTERVAL_MS: u64 = 250;
/// 一次丢帧后的冷却时间，避免连续跳变。
pub(crate) const DROP_COOLDOWN_MS: u64 = 500;
/// 缓冲超过 target + 100ms 时开始轻微加速追帧。
pub(crate) const TEMPO_SOFT_MS: u64 = 100;
/// 缓冲超过 target + 300ms 时加速到上限。
pub(crate) const TEMPO_HARD_MS: u64 = 300;
/// 轻加速：5%。
pub(crate) const TEMPO_SOFT_SPEED: f32 = 1.05;
/// 重加速：10%。
pub(crate) const TEMPO_HARD_SPEED: f32 = 1.10;
/// 帧队列硬上限：约 87s 音频。暂停缓存 cap 30s，恢复后还要容纳
/// 生成中的 live 数据，所以不能只按 30s 配。
pub(crate) const QUEUE_CAPACITY_FRAMES: usize = 4096;

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
    target_ms: u64,
    max_ms: u64,
    min_ms: u64,
    /// 当前 WSOLA 速度；只增不减（回到 target 才回 1.00）。
    speed: f32,
}

impl BufferController {
    pub(crate) fn new() -> Self {
        let target_ms = target_buffer_ms();
        Self {
            last_tick: None,
            last_drop: None,
            starving: false,
            target_ms,
            max_ms: target_ms.saturating_add(500),
            min_ms: target_ms.saturating_sub(300),
            speed: MIN_TEMPO_SPEED,
        }
    }

    /// 当前 WSOLA 速度，直接交给解码线程的变速器。
    pub(crate) fn speed(&self) -> f32 {
        self.speed
    }

    /// 距上次控制周期是否已满 250ms。
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.last_tick.is_none_or(|last| {
            now.saturating_duration_since(last) >= Duration::from_millis(CONTROL_INTERVAL_MS)
        })
    }

    /// 推进一次控制周期。只有 `buffer > target+500ms` 且冷却已过才丢帧；
    /// 其余时间只调整 WSOLA 速度；`buffer < target-300ms` 不再阻塞解码，
    /// 调用方只在队列空时补静音。
    pub(crate) fn tick(&mut self, now: Instant, buffer_ms: u64) -> Control {
        self.last_tick = Some(now);

        let cooldown_ok = self.last_drop.is_none_or(|last| {
            now.saturating_duration_since(last) >= Duration::from_millis(DROP_COOLDOWN_MS)
        });
        if buffer_ms > self.max_ms && cooldown_ok {
            self.last_drop = Some(now);
            // 丢帧后缓冲回到目标附近，追帧速度同步回 1.00，避免继续压缩。
            self.speed = MIN_TEMPO_SPEED;
            return Control::DropTo(self.target_ms);
        }

        self.speed = tempo_speed(self.speed, buffer_ms, self.target_ms);

        if self.starving && buffer_ms >= self.min_ms {
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

/// 由缓冲量决定 WSOLA 速度（只加速，不降速）。
///
/// - `buffer > target + 300ms` → 1.10x；
/// - `buffer > target + 100ms` → 1.05x；
/// - `buffer <= target` → 1.00x；
/// - `(target, target + 100ms]` 保持当前速度，避免在阈值上反复跳变。
fn tempo_speed(current: f32, buffer_ms: u64, target_ms: u64) -> f32 {
    if buffer_ms > target_ms.saturating_add(TEMPO_HARD_MS) {
        TEMPO_HARD_SPEED
    } else if buffer_ms > target_ms.saturating_add(TEMPO_SOFT_MS) {
        TEMPO_SOFT_SPEED
    } else if buffer_ms <= target_ms {
        MIN_TEMPO_SPEED
    } else {
        current.clamp(MIN_TEMPO_SPEED, TEMPO_HARD_SPEED)
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

    #[test]
    fn controller_ramps_tempo_above_the_soft_and_hard_thresholds() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        assert_eq!(controller.speed(), 1.0);

        // (target, target+100] 保持当前速度，不因为噪声跳变。
        assert_eq!(controller.tick(base, TARGET_BUFFER_MS + 50), Control::Idle);
        assert_eq!(controller.speed(), 1.0);

        assert_eq!(
            controller.tick(base + Duration::from_millis(250), TARGET_BUFFER_MS + 150),
            Control::Idle,
            "tempo chases the buffer instead of dropping frames"
        );
        assert_eq!(controller.speed(), 1.05);

        assert_eq!(
            controller.tick(base + Duration::from_millis(500), TARGET_BUFFER_MS + 350),
            Control::Idle
        );
        assert_eq!(controller.speed(), 1.10);
    }

    #[test]
    fn controller_returns_to_one_x_only_at_target() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        let step = Duration::from_millis(250);

        assert_eq!(controller.tick(base, TARGET_BUFFER_MS + 350), Control::Idle);
        assert_eq!(controller.speed(), 1.10);
        assert_eq!(
            controller.tick(base + step, TARGET_BUFFER_MS + 150),
            Control::Idle
        );
        assert_eq!(controller.speed(), 1.05);
        // 回到 target 上方的保持带：不急着回 1.00。
        assert_eq!(
            controller.tick(base + step * 2, TARGET_BUFFER_MS + 50),
            Control::Idle
        );
        assert_eq!(controller.speed(), 1.05);
        assert_eq!(
            controller.tick(base + step * 3, TARGET_BUFFER_MS),
            Control::Idle
        );
        assert_eq!(controller.speed(), 1.0);
    }

    #[test]
    fn controller_never_slows_down() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        for (index, buffer) in [0u64, 200, TARGET_BUFFER_MS - 1, TARGET_BUFFER_MS]
            .into_iter()
            .enumerate()
        {
            let now = base + Duration::from_millis(250 * index as u64);
            let _ = controller.tick(now, buffer);
            assert!(
                controller.speed() >= 1.0,
                "speed must never drop below 1.0, got {} at {buffer}ms",
                controller.speed()
            );
        }
    }

    #[test]
    fn hard_drop_still_wins_and_resets_tempo() {
        let base = Instant::now();
        let mut controller = BufferController::new();
        assert_eq!(controller.tick(base, TARGET_BUFFER_MS + 350), Control::Idle);
        assert_eq!(controller.speed(), 1.10);
        assert_eq!(
            controller.tick(base + Duration::from_millis(250), MAX_BUFFER_MS + 1),
            Control::DropTo(TARGET_BUFFER_MS)
        );
        assert_eq!(controller.speed(), 1.0, "a hard drop returns to 1.00x");
    }

    #[test]
    fn target_priority_is_env_then_setter_then_default() {
        assert_eq!(resolve_target_buffer_ms(None, None, 900), 900);
        assert_eq!(resolve_target_buffer_ms(None, Some(1_000), 900), 1_000);
        assert_eq!(
            resolve_target_buffer_ms(Some(700), Some(1_000), 900),
            700,
            "NP_LIVE_TARGET_MS must win as the debug override"
        );
        assert_eq!(
            resolve_target_buffer_ms(Some(50), None, 900),
            ENV_TARGET_MIN_MS
        );
        assert_eq!(
            resolve_target_buffer_ms(Some(10_000), None, 900),
            ENV_TARGET_MAX_MS
        );
    }

    #[test]
    fn target_setter_takes_effect_and_clear_restores_the_default() {
        let _guard = TARGET_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        clear_target_buffer_ms();
        assert_eq!(target_buffer_ms_unlocked(), TARGET_BUFFER_MS);

        // 与 `BufferController::new()` 走同一个解析函数。
        set_target_buffer_ms(1_000);
        assert_eq!(target_buffer_ms_unlocked(), 1_000);

        set_target_buffer_ms(100);
        assert_eq!(target_buffer_ms_unlocked(), MIN_TARGET_BUFFER_MS);
        set_target_buffer_ms(10_000);
        assert_eq!(target_buffer_ms_unlocked(), MAX_TARGET_BUFFER_MS);

        clear_target_buffer_ms();
        assert_eq!(target_buffer_ms_unlocked(), TARGET_BUFFER_MS);
    }
}
