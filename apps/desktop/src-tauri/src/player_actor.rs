//! 播放器 actor：单线程独占 `np_audio::Player`，串行处理命令并广播事件。
//!
//! `np_audio::Player` 是 `!Send`（`Engine` 持有 `Box<dyn AudioSink>`，trait 没有
//! `Send` 约束），因此 actor 用 `std::thread` + `std::sync::mpsc::recv_timeout`
//! 在同一个循环里处理命令与 4 Hz tick（ARCH-006 §4 / T-0013 §7）。
//!
//! `Player` 的创建与 drop 都发生在 actor 线程；**切歌也必须在这里完成**
//! （T-0015）：先把旧 `Player` `take()` 出来在本线程 drop，再 `Player::open`
//! 新曲目，不能把 `Player` 跨线程移动。
//!
//! 事件出口抽象成 [`EventSink`]：生产环境是 Tauri 的 `AppHandle`，单测用
//! 内存记录器，这样 actor 的队列/切歌逻辑不依赖真实窗口。

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use np_audio::{AudioError, CpalDevices, DeviceSignature, Player, clamp_volume};
use np_core::{
    AudioFailure, POSITION_TICK_MS, PlaybackMachine, PlaybackMode, PlaybackState, PlaybackStatus,
    PlayerCommand, PlayerError, PlayerEvent, Queue, QueueStep, QueueView, player::AdvanceReason,
};
use tauri::{AppHandle, Emitter};

/// 状态快照事件。
///
/// 主线定名 `player:state`（T-0013）：Tauri 2.12 的 `EventName` 只允许
/// 字母数字与 `- / : _`，点号非法；开发方案 §4.3 原写法 `evt.player` 需同步。
pub(crate) const EVT_PLAYER: &str = "player:state";
/// 4 Hz 位置事件（主线定名 `player:position`）。
pub(crate) const EVT_POSITION: &str = "player:position";

/// 命令通道容量：满了立即回 `E_BUSY`，不阻塞调用方。
const COMMAND_CAPACITY: usize = 32;
/// 单条命令等待 actor 应答的上限；超时按 `E_BUSY` 返回。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// actor 的 tick 周期（4 Hz）。
const TICK_INTERVAL: Duration = Duration::from_millis(POSITION_TICK_MS);
/// 切歌时打开失败的连续跳过上限（T-0015 §6）。
const MAX_OPEN_ATTEMPTS: usize = 3;
/// 设备探测周期（tick 数）：4 × 250 ms ≈ 1 秒（T-0016 §2.1）。
const DEVICE_PROBE_TICKS: u32 = 4;

/// actor 的事件出口。生产实现是 Tauri 的 [`AppHandle`]。
pub(crate) trait EventSink: Send + 'static {
    /// 广播一个事件；失败只打日志，不影响播放。
    fn emit(&self, event: PlayerEvent);
}

impl EventSink for AppHandle {
    fn emit(&self, event: PlayerEvent) {
        let result = match event {
            PlayerEvent::StateChanged(state) => Emitter::emit(self, EVT_PLAYER, state),
            PlayerEvent::PositionTick(tick) => Emitter::emit(self, EVT_POSITION, tick),
        };
        if let Err(error) = result {
            eprintln!("[player] emit failed: {error}");
        }
    }
}

/// actor 的应答：播放命令回快照，队列命令回队列视图。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ActorReply {
    /// 播放状态快照。
    State(PlaybackState),
    /// 队列视图。
    Queue(QueueView),
}

/// 默认输出设备的只读探测（T-0016 §2.1）。
///
/// 生产实现是 [`CpalDevices`]，单测注入固定签名、不依赖真实硬件；探测只在
/// actor 的 tick 循环里跑，不新开线程（T-0016 §6）。
pub(crate) trait DeviceProbe {
    /// 当前默认输出设备签名（名称 + 通道数）；没有可用设备时为 `None`。
    fn default_output(&self) -> Option<DeviceSignature>;
}

impl DeviceProbe for CpalDevices {
    fn default_output(&self) -> Option<DeviceSignature> {
        self.default_output_signature()
    }
}

/// 桌面层持有的 actor 句柄（Tauri managed state）。
pub(crate) struct PlayerHandle {
    tx: SyncSender<ActorMessage>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl PlayerHandle {
    /// 启动 actor 线程并返回句柄；线程内惰性创建 `Player`。
    pub(crate) fn spawn(app: AppHandle) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        // `PlayerActor`（含 `Option<Player>`）在 actor 线程内构造：`Player`
        // 是 `!Send`，连“先构造再移动”都不允许。
        let join = thread::Builder::new()
            .name("np-player-actor".into())
            .spawn(move || PlayerActor::new(app).run(rx))?;
        Ok(Self {
            tx,
            join: Mutex::new(Some(join)),
        })
    }

    /// 当前完整快照。
    pub(crate) fn snapshot(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Snapshot)
    }

    /// 打开 `path` 并播放（同时入队）；`path = None` 时继续当前曲目。
    pub(crate) fn play(&self, path: Option<String>) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Play { path })
    }

    /// 暂停。
    pub(crate) fn pause(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Pause)
    }

    /// 下一首。
    pub(crate) fn next(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Next)
    }

    /// 上一首。
    pub(crate) fn prev(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Prev)
    }

    /// 跳转。
    pub(crate) fn seek(&self, position_ms: u64) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Seek { position_ms })
    }

    /// 设置音量（0.0–1.0）。
    pub(crate) fn volume(&self, volume: f32) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::SetVolume { volume })
    }

    /// 设置 `repeat` / `shuffle`。
    pub(crate) fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::SetMode { mode })
    }

    /// 队列快照。
    pub(crate) fn queue_list(&self) -> Result<QueueView, PlayerError> {
        self.request_queue(PlayerCommand::QueueList)
    }

    /// 追加曲目。
    pub(crate) fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, PlayerError> {
        self.request_queue(PlayerCommand::QueueAdd { paths })
    }

    /// 移除队列下标（按播放顺序）。
    pub(crate) fn queue_remove(&self, index: usize) -> Result<QueueView, PlayerError> {
        self.request_queue(PlayerCommand::QueueRemove { index })
    }

    /// 清空队列。
    pub(crate) fn queue_clear(&self) -> Result<QueueView, PlayerError> {
        self.request_queue(PlayerCommand::QueueClear)
    }

    /// 跳到队列下标并播放。
    pub(crate) fn queue_jump(&self, index: usize) -> Result<QueueView, PlayerError> {
        self.request_queue(PlayerCommand::QueueJump { index })
    }

    /// 停止 actor 并等待 `Player` 在 actor 线程内释放设备。幂等。
    pub(crate) fn shutdown(&self) {
        let join = self.join.lock().ok().and_then(|mut slot| slot.take());
        let Some(join) = join else {
            return;
        };
        // 通道满时阻塞到 actor 取走消息；actor 线程已退出时 send 直接报错。
        if self.tx.send(ActorMessage::Shutdown).is_ok() {
            let _ = join.join();
        }
    }

    /// 发送播放命令并等待快照应答。
    fn request(&self, command: PlayerCommand) -> Result<PlaybackState, PlayerError> {
        match self.request_reply(command)? {
            ActorReply::State(state) => Ok(state),
            ActorReply::Queue(_) => Err(PlayerError::busy("播放器应答类型不匹配")),
        }
    }

    /// 发送队列命令并等待队列视图应答。
    fn request_queue(&self, command: PlayerCommand) -> Result<QueueView, PlayerError> {
        match self.request_reply(command)? {
            ActorReply::Queue(view) => Ok(view),
            ActorReply::State(_) => Err(PlayerError::busy("队列应答类型不匹配")),
        }
    }

    /// 发送一条命令并等待一次性应答。
    fn request_reply(&self, command: PlayerCommand) -> Result<ActorReply, PlayerError> {
        let (reply, response) = mpsc::sync_channel(1);
        let message = ActorMessage::Command(PendingCommand { command, reply });

        match self.tx.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err(PlayerError::busy("播放器命令队列已满，请稍后重试"));
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(PlayerError::busy("播放器线程已退出"));
            }
        }

        match response.recv_timeout(COMMAND_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(PlayerError::busy("播放器响应超时")),
            Err(RecvTimeoutError::Disconnected) => Err(PlayerError::busy("播放器线程已退出")),
        }
    }
}

impl Drop for PlayerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 一条带一次性应答通道的命令。
struct PendingCommand {
    command: PlayerCommand,
    reply: SyncSender<Result<ActorReply, PlayerError>>,
}

/// actor 控制通道的消息。
enum ActorMessage {
    Command(PendingCommand),
    Shutdown,
}

/// 独占 `Player` 的单线程状态机。
struct PlayerActor<S: EventSink> {
    sink: S,
    machine: PlaybackMachine,
    queue: Queue,
    player: Option<Player>,
    /// 默认输出设备探测器（生产 cpal，单测注入）。
    devices: Box<dyn DeviceProbe>,
    /// 上一次观察到的默认输出设备；`None` 表示还没建立基线（T-0016 §2.1）。
    observed_device: Option<DeviceSignature>,
    /// tick 计数，用于把设备探测降到约 1 Hz。
    tick_count: u32,
}

impl<S: EventSink> PlayerActor<S> {
    fn new(sink: S) -> Self {
        Self::with_devices(sink, Box::new(CpalDevices::new()))
    }

    /// 用可注入的设备探测器构造（单测不碰真实硬件，T-0016 §8）。
    fn with_devices(sink: S, devices: Box<dyn DeviceProbe>) -> Self {
        Self {
            sink,
            machine: PlaybackMachine::new(),
            queue: Queue::new(),
            player: None,
            devices,
            observed_device: None,
            tick_count: 0,
        }
    }

    /// 主循环：命令优先，按固定 deadline 每 250 ms 跑一次 tick。
    ///
    /// 直接用 `recv_timeout(TICK_INTERVAL)` 会累积 Windows 定时器粒度
    /// （实测约 265 ms），所以用绝对 deadline 计算下一次等待时长。
    fn run(mut self, rx: Receiver<ActorMessage>) {
        let mut next_tick = Instant::now() + TICK_INTERVAL;
        loop {
            let mut batch = Vec::new();
            let mut shutdown = false;

            let wait = next_tick.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(ActorMessage::Command(pending)) => batch.push(pending),
                Ok(ActorMessage::Shutdown) => shutdown = true,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => shutdown = true,
            }

            if !shutdown {
                loop {
                    match rx.try_recv() {
                        Ok(ActorMessage::Command(pending)) => batch.push(pending),
                        Ok(ActorMessage::Shutdown) => {
                            shutdown = true;
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            shutdown = true;
                            break;
                        }
                    }
                }
            }

            if !batch.is_empty() {
                self.handle_batch(batch);
            }
            if shutdown {
                break;
            }

            if Instant::now() >= next_tick {
                self.tick();
                next_tick += TICK_INTERVAL;
                let now = Instant::now();
                if next_tick <= now {
                    // 落后超过一个周期（例如命令阻塞）时不补发，避免突发。
                    next_tick = now + TICK_INTERVAL;
                }
            }
        }

        // 必须在本线程释放 cpal 流与解码线程（T-0013 §7）。
        self.player = None;
        println!("[player] actor stopped, device released");
    }

    /// 批量执行命令；`seek` / `volume` 只执行最后一次（latest-wins）。
    ///
    /// 切歌命令不与 seek 合并：批内按到达顺序串行执行，`next` 之后的 seek
    /// 自然作用在新曲目上；`next` 之前的 seek 作用在旧曲目上。
    fn handle_batch(&mut self, batch: Vec<PendingCommand>) {
        let last_seek = batch
            .iter()
            .rposition(|pending| matches!(pending.command, PlayerCommand::Seek { .. }));
        let last_volume = batch
            .iter()
            .rposition(|pending| matches!(pending.command, PlayerCommand::SetVolume { .. }));

        let mut seek_replies = Vec::new();
        let mut volume_replies = Vec::new();

        for (index, pending) in batch.into_iter().enumerate() {
            let is_seek = matches!(pending.command, PlayerCommand::Seek { .. });
            let is_volume = matches!(pending.command, PlayerCommand::SetVolume { .. });

            if is_seek && Some(index) != last_seek {
                seek_replies.push(pending.reply);
                continue;
            }
            if is_volume && Some(index) != last_volume {
                volume_replies.push(pending.reply);
                continue;
            }

            let result = self.execute(pending.command);

            if is_seek {
                for reply in seek_replies.drain(..) {
                    let _ = reply.send(result.clone());
                }
            }
            if is_volume {
                for reply in volume_replies.drain(..) {
                    let _ = reply.send(result.clone());
                }
            }
            let _ = pending.reply.send(result);
        }
    }

    fn execute(&mut self, command: PlayerCommand) -> Result<ActorReply, PlayerError> {
        match command {
            PlayerCommand::Snapshot => self.snapshot().map(ActorReply::State),
            PlayerCommand::Play { path } => self.play(path).map(ActorReply::State),
            PlayerCommand::Pause => self.pause().map(ActorReply::State),
            PlayerCommand::Next => self
                .next_track(AdvanceReason::Manual)
                .map(ActorReply::State),
            PlayerCommand::Prev => self.prev_track().map(ActorReply::State),
            PlayerCommand::Seek { position_ms } => self.seek(position_ms).map(ActorReply::State),
            PlayerCommand::SetVolume { volume } => self.set_volume(volume).map(ActorReply::State),
            PlayerCommand::SetMode { mode } => self.set_mode(mode).map(ActorReply::State),
            PlayerCommand::QueueList => Ok(ActorReply::Queue(self.queue.view())),
            PlayerCommand::QueueAdd { paths } => self.queue_add(paths).map(ActorReply::Queue),
            PlayerCommand::QueueRemove { index } => self.queue_remove(index).map(ActorReply::Queue),
            PlayerCommand::QueueClear => self.queue_clear().map(ActorReply::Queue),
            PlayerCommand::QueueJump { index } => self.queue_jump(index).map(ActorReply::Queue),
            PlayerCommand::Shutdown => {
                Err(PlayerError::unsupported("Shutdown 由 actor 控制通道处理"))
            }
        }
    }

    fn snapshot(&mut self) -> Result<PlaybackState, PlayerError> {
        // `Ended` 时位置停在末尾（由状态机钉住），不要被已停下的 player 拉回 0。
        if self.machine.snapshot().status != PlaybackStatus::Ended {
            let position = self.position_ms();
            self.machine.sync_position(position);
        }
        self.sync_queue_state();
        Ok(self.machine.snapshot())
    }

    fn play(&mut self, path: Option<String>) -> Result<PlaybackState, PlayerError> {
        match path {
            Some(path) => {
                let index = match self.queue.find(&path) {
                    Some(index) => index,
                    None => self.queue.push(path),
                };
                // 刚查找/插入的下标必然有效；失败说明队列不变式被破坏。
                let Some(index) = self.queue.jump(index) else {
                    return Err(PlayerError::busy("队列下标失效"));
                };
                self.sync_queue_state();
                self.switch_to(index, true)
            }
            None => self.resume(),
        }
    }

    /// 继续当前曲目；播完后重新 `play` 先回到 0。
    ///
    /// 队列非空但还没有打开过 player（例如 `queue.add` 之后直接 `play`）时，
    /// 打开当前曲目再播。
    fn resume(&mut self) -> Result<PlaybackState, PlayerError> {
        if self.player.is_none() {
            let Some(index) = self.queue.position() else {
                return Err(PlayerError::unsupported("没有可播放的曲目，请先提供 path"));
            };
            self.sync_queue_state();
            return self.switch_to(index, true);
        }
        let restart = self.machine.snapshot().status == PlaybackStatus::Ended;

        let (seek_result, position, play_result) = match self.player.as_ref() {
            Some(player) => {
                let seek_result = if restart {
                    player.seek(0).map_err(map_audio_error)
                } else {
                    Ok(())
                };
                let position = player.position_ms();
                let play_result = if seek_result.is_ok() {
                    player.play().map_err(map_audio_error)
                } else {
                    Ok(())
                };
                (seek_result, position, play_result)
            }
            None => {
                return Err(PlayerError::unsupported("没有可播放的曲目，请先提供 path"));
            }
        };

        if let Err(error) = seek_result {
            self.fail(error.clone(), position);
            return Err(error);
        }
        if let Err(error) = play_result {
            self.fail(error.clone(), position);
            return Err(error);
        }

        let event = self.machine.play(position);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    fn pause(&mut self) -> Result<PlaybackState, PlayerError> {
        let (position, result) = match self.player.as_ref() {
            Some(player) => (
                player.position_ms(),
                player.pause().map_err(map_audio_error),
            ),
            None => return Err(PlayerError::unsupported("没有可暂停的曲目")),
        };

        if let Err(error) = result {
            self.fail(error.clone(), position);
            return Err(error);
        }

        let event = self.machine.pause(position);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    fn seek(&mut self, position_ms: u64) -> Result<PlaybackState, PlayerError> {
        let (result, actual) = match self.player.as_ref() {
            Some(player) => (
                player.seek(position_ms).map_err(map_audio_error),
                player.position_ms(),
            ),
            None => return Err(PlayerError::unsupported("没有可 seek 的曲目")),
        };

        if let Err(error) = result {
            self.fail(error.clone(), actual);
            return Err(error);
        }

        let event = self.machine.seek(actual);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    fn set_volume(&mut self, volume: f32) -> Result<PlaybackState, PlayerError> {
        let volume = clamp_volume(volume);
        if let Some(player) = self.player.as_ref() {
            player.set_volume(volume);
        }
        let event = self.machine.set_volume(volume);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    /// 下一首。`reason` 决定 `repeat = one` 是否生效（T-0016 §2.3）；
    /// `repeat = off` 且已在最后一首时停在该曲目并置 `Ended`。
    fn next_track(&mut self, reason: AdvanceReason) -> Result<PlaybackState, PlayerError> {
        if self.queue.is_empty() {
            return Err(PlayerError::unsupported("队列为空，无法切歌"));
        }
        match self.queue.next(reason) {
            QueueStep::Open(index) => self.switch_to(index, true),
            QueueStep::Exhausted => self.finish_queue(),
        }
    }

    /// 上一首。显式切歌忽略 `repeat = one`；`repeat = off` 时在首曲重播首曲，不越界。
    fn prev_track(&mut self) -> Result<PlaybackState, PlayerError> {
        if self.queue.is_empty() {
            return Err(PlayerError::unsupported("队列为空，无法切歌"));
        }
        match self.queue.prev(AdvanceReason::Manual) {
            QueueStep::Open(index) => self.switch_to(index, true),
            QueueStep::Exhausted => Ok(self.machine.snapshot()),
        }
    }

    /// 队列播完：暂停输出，停在该曲目并把状态置 `Ended`。
    fn finish_queue(&mut self) -> Result<PlaybackState, PlayerError> {
        let position = self.position_ms();
        let result = match self.player.as_ref() {
            Some(player) => player.pause().map_err(map_audio_error),
            None => Ok(()),
        };
        if let Err(error) = result {
            self.fail(error.clone(), position);
            return Err(error);
        }
        self.sync_queue_state();
        let event = self.machine.end(position);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    /// 切歌到 `index`：打开失败自动跳下一首，最多连续尝试 [`MAX_OPEN_ATTEMPTS`] 首。
    ///
    /// 全部失败时释放旧 `Player`、置 `status = error` 并返回最后一次错误。
    fn switch_to(&mut self, index: usize, autoplay: bool) -> Result<PlaybackState, PlayerError> {
        let mut candidate = index;
        let mut attempt = 1;
        let error = loop {
            match self.open_candidate(candidate, autoplay) {
                Ok(()) => return Ok(self.machine.snapshot()),
                Err(error) => {
                    if attempt >= MAX_OPEN_ATTEMPTS {
                        break error;
                    }
                    match self.queue.next_for_skip() {
                        Some(next) => candidate = next,
                        None => break error,
                    }
                    attempt += 1;
                }
            }
        };

        self.player = None;
        self.sync_queue_state();
        self.fail(error.clone(), 0);
        Err(error)
    }

    /// 打开第 `index` 首并（可选）开始播放。
    ///
    /// 时序（T-0015 §11）：先 `take()` / drop 旧 `Player`，再 `Player::open`，
    /// 最后 `play()`——两个 cpal 输出流不能同时在世。
    fn open_candidate(&mut self, index: usize, autoplay: bool) -> Result<(), PlayerError> {
        let path = match self.queue.track_at(index) {
            Some(path) => path.to_owned(),
            None => return Err(PlayerError::decode("队列下标越界")),
        };
        let volume = self.machine.snapshot().volume;

        self.player = None;
        let player = Player::open(&path).map_err(map_audio_error)?;
        player.set_volume(volume);
        let duration_ms = player.duration_ms();
        self.player = Some(player);
        self.observed_device = self.devices.default_output();

        self.sync_queue_state();
        let opened = self.machine.open(path, duration_ms);
        self.emit(opened);

        if !autoplay {
            return Ok(());
        }

        let play_result = match self.player.as_ref() {
            Some(player) => player.play().map_err(map_audio_error),
            None => Err(PlayerError::busy("播放器状态丢失")),
        };
        if let Err(error) = play_result {
            // 打开成功但起播失败：释放它，交给 skip 循环尝试下一首。
            self.player = None;
            return Err(error);
        }
        let started = self.machine.play(0);
        self.emit(started);
        Ok(())
    }

    fn set_mode(&mut self, mode: PlaybackMode) -> Result<PlaybackState, PlayerError> {
        self.queue.set_mode(mode);
        self.sync_queue_state();
        self.emit_state();
        Ok(self.machine.snapshot())
    }

    fn queue_add(&mut self, paths: Vec<String>) -> Result<QueueView, PlayerError> {
        if paths.is_empty() {
            return Err(PlayerError::decode("paths 不能为空"));
        }
        for path in paths {
            self.queue.push(path);
        }
        self.after_queue_change();
        Ok(self.queue.view())
    }

    fn queue_remove(&mut self, index: usize) -> Result<QueueView, PlayerError> {
        let was_current = self.queue.position() == Some(index);
        if self.queue.remove(index).is_none() {
            return Err(PlayerError::decode("队列下标越界"));
        }

        if was_current {
            // 语义（主线确认，T-0015）：停止当前播放、**不自动播下一首**；
            // 指针已由 `Queue::remove` 停在原位置的下一首（删末尾则退到新的末尾）。
            self.player = None;
            self.machine.reset();
        }
        self.after_queue_change();
        Ok(self.queue.view())
    }

    fn queue_clear(&mut self) -> Result<QueueView, PlayerError> {
        self.queue.clear();
        self.player = None;
        self.sync_queue_state();
        let event = self.machine.reset();
        self.emit(event);
        Ok(self.queue.view())
    }

    fn queue_jump(&mut self, index: usize) -> Result<QueueView, PlayerError> {
        let Some(index) = self.queue.jump(index) else {
            return Err(PlayerError::decode("队列下标越界"));
        };
        self.sync_queue_state();
        self.switch_to(index, true)?;
        Ok(self.queue.view())
    }

    /// 队列变更后同步快照并立即推 `player:state`（T-0015 §6）。
    fn after_queue_change(&mut self) {
        self.sync_queue_state();
        self.emit_state();
    }

    fn sync_queue_state(&mut self) {
        let length = self.queue.len();
        let index = self.queue.position();
        let mode = self.queue.mode();
        self.machine.sync_queue(length, index, mode);
    }

    /// 推送一次当前快照（不改状态）。
    fn emit_state(&mut self) {
        let state = self.machine.snapshot();
        self.emit(PlayerEvent::StateChanged(state));
    }

    /// 4 Hz tick：设备探测、错误恢复、位置事件与自然播完切歌。
    ///
    /// 设备探测每 4 次 tick（约 1 秒）比较一次默认输出设备签名（T-0016 §2.1）；
    /// 自然播完时先交给队列：`repeat = one` 重播、`repeat = all` 回绕、
    /// 否则进下一首；没有下一首才上报 `Ended`（T-0015 §6）。
    fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.tick_count.is_multiple_of(DEVICE_PROBE_TICKS) {
            self.check_device_change();
        }

        let Some(player) = self.player.as_ref() else {
            return;
        };
        let device_error = player.take_device_error();
        let message = player.last_error();
        let position = player.position_ms();
        let ended = player.is_ended();

        if device_error {
            self.recover_device_lost(position);
            return;
        }
        if let Some(message) = message {
            self.fail(PlayerError::decode(message), position);
            return;
        }

        if ended && self.machine.is_playing() {
            // 自然播完：`repeat = one` 在这里生效（T-0016 §2.3）。
            match self.queue.next(AdvanceReason::Natural) {
                QueueStep::Open(index) => {
                    // 切歌自带状态推送；失败已按 error 状态上报。
                    let _ = self.switch_to(index, true);
                    return;
                }
                QueueStep::Exhausted => {}
            }
        }

        if let Some(event) = self.machine.tick(position, ended) {
            self.emit(event);
        }
    }

    /// 默认输出设备是否变化：名称 + 通道数复合判断（T-0016 §2.1/§11）。
    ///
    /// 变化时按原位置重开当前曲目；重开失败置 `E_DEVICE_LOST`，队列保留。
    fn check_device_change(&mut self) {
        if self.machine.snapshot().status == PlaybackStatus::Idle {
            return;
        }
        let Some(current) = self.devices.default_output() else {
            // 拿不到默认设备签名：不在这里判死，交给设备错误标志与重开失败。
            return;
        };
        let Some(previous) = self.observed_device.replace(current.clone()) else {
            // 首次观察：只建立基线。
            return;
        };
        if previous == current {
            return;
        }
        let position = self
            .player
            .as_ref()
            .map_or_else(|| self.machine.snapshot().position_ms, Player::position_ms);
        println!(
            "[player] default output device changed: {} ({} ch) -> {} ({} ch), reopening at {position} ms",
            previous.name, previous.channels, current.name, current.channels
        );
        self.recover_device_lost(position);
    }

    /// 设备侧失败：先释放旧输出流，再用当前默认设备重开当前曲目并 seek 回原位。
    ///
    /// 成功保持原来的播放/暂停状态；失败置 `E_DEVICE_LOST`，队列保留，
    /// 用户 `play` 可重试（T-0016 §2.1）。
    fn recover_device_lost(&mut self, position_ms: u64) {
        let was_playing = self.machine.is_playing();
        let Some(index) = self.queue.position() else {
            self.player = None;
            return;
        };
        match self.reopen_at(index, position_ms, was_playing) {
            Ok(()) => println!(
                "[player] output device recovered, position {} ms",
                self.machine.snapshot().position_ms
            ),
            Err(error) => {
                self.player = None;
                self.fail(
                    PlayerError::device_lost(format!("output device lost: {}", error.message)),
                    position_ms,
                );
            }
        }
    }

    /// 释放旧 `Player`，用当前默认设备重开 `index`，seek 回 `position_ms`。
    ///
    /// 设备变化与解码恢复共用（T-0016 §2.1/§2.2）：始终先 `self.player = None`
    /// 再 `Player::open`，两个 cpal 输出流不能同时在世（T-0015 §11）。
    /// `seek` 失败不阻断恢复，继续播放（卡 §6）；位置钳制到 `duration_ms`（卡 §11）。
    fn reopen_at(
        &mut self,
        index: usize,
        position_ms: u64,
        resume_playing: bool,
    ) -> Result<(), PlayerError> {
        let path = match self.queue.track_at(index) {
            Some(path) => path.to_owned(),
            None => return Err(PlayerError::decode("队列下标越界")),
        };
        let volume = self.machine.snapshot().volume;

        self.player = None;
        let player = Player::open(&path).map_err(map_audio_error)?;
        player.set_volume(volume);
        let duration_ms = player.duration_ms();
        let target = duration_ms.map_or(position_ms, |duration| position_ms.min(duration));
        if let Err(error) = player.seek(target) {
            eprintln!("[player] recovery seek to {target} ms failed: {error}");
        }
        let actual = player.position_ms().min(duration_ms.unwrap_or(u64::MAX));
        self.player = Some(player);

        self.observed_device = self.devices.default_output();
        self.sync_queue_state();

        if resume_playing {
            let result = self
                .player
                .as_ref()
                .map_or(Ok(()), |player| player.play().map_err(map_audio_error));
            if let Err(error) = result {
                self.player = None;
                return Err(error);
            }
            let event = self.machine.play(actual);
            self.emit(event);
        } else {
            let event = self.machine.pause(actual);
            self.emit(event);
        }
        Ok(())
    }

    fn fail(&mut self, error: PlayerError, position_ms: u64) {
        if let Some(event) = self.machine.fail(error, position_ms) {
            self.emit(event);
        }
    }

    fn position_ms(&self) -> u64 {
        self.player
            .as_ref()
            .map_or(0, |player| player.position_ms())
    }

    fn emit(&self, event: PlayerEvent) {
        self.sink.emit(event);
    }
}

/// 按 T-0013 §7 的错误码表把 `np-audio` 的失败归类并映射成 IPC 错误。
fn map_audio_error(error: AudioError) -> PlayerError {
    let message = error.to_string();
    let failure = match error {
        AudioError::UnsupportedFormat(_)
        | AudioError::NoSupportedFormat
        | AudioError::UnsupportedSampleFormat(_) => AudioFailure::Unsupported,
        AudioError::DeviceUnavailable(_) | AudioError::DeviceOpen(_) | AudioError::DeviceLost => {
            AudioFailure::DeviceLost
        }
        AudioError::Decode(_)
        | AudioError::Seek(_)
        | AudioError::Resample(_)
        | AudioError::InvalidArgument(_)
        | AudioError::Io(_) => AudioFailure::Decode,
    };
    PlayerError::from_audio(failure, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use np_audio::SampleFormat;
    use np_core::{PlayerErrorCode, RepeatMode};

    /// 收集事件的内存 sink，代替 Tauri 的 `AppHandle`。
    #[derive(Clone, Default)]
    struct RecordingSink {
        events: Arc<Mutex<Vec<PlayerEvent>>>,
    }

    impl EventSink for RecordingSink {
        fn emit(&self, event: PlayerEvent) {
            self.events.lock().expect("event log poisoned").push(event);
        }
    }

    impl RecordingSink {
        fn last_state(&self) -> Option<PlaybackState> {
            self.events
                .lock()
                .expect("event log poisoned")
                .iter()
                .rev()
                .find_map(|event| match event {
                    PlayerEvent::StateChanged(state) => Some(state.clone()),
                    PlayerEvent::PositionTick(_) => None,
                })
        }
    }

    /// 固定签名的设备探测器：单测不依赖真实硬件（T-0016 §8）。
    #[derive(Clone, Default)]
    struct FixedProbe {
        signature: Arc<Mutex<Option<DeviceSignature>>>,
    }

    impl FixedProbe {
        fn new(signature: DeviceSignature) -> Self {
            Self {
                signature: Arc::new(Mutex::new(Some(signature))),
            }
        }

        fn set(&self, signature: Option<DeviceSignature>) {
            *self.signature.lock().expect("probe poisoned") = signature;
        }
    }

    impl DeviceProbe for FixedProbe {
        fn default_output(&self) -> Option<DeviceSignature> {
            self.signature.lock().expect("probe poisoned").clone()
        }
    }

    fn device(name: &str, channels: u16) -> DeviceSignature {
        DeviceSignature {
            name: name.into(),
            channels,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("np-t0015-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn event_names_are_legal_tauri_names() {
        assert_eq!(EVT_PLAYER, "player:state");
        assert_eq!(EVT_POSITION, "player:position");
        for name in [EVT_PLAYER, EVT_POSITION] {
            assert!(
                name.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '/' | ':' | '_')),
                "{name} is rejected by tauri::EventName"
            );
            assert!(!name.contains('.'), "dots are illegal in Tauri event names");
        }
    }

    #[test]
    fn audio_errors_map_to_the_expected_codes() {
        let cases = [
            (
                AudioError::Decode("bad frame".into()),
                PlayerErrorCode::Decode,
            ),
            (
                AudioError::Io(std::io::Error::other("missing file")),
                PlayerErrorCode::Decode,
            ),
            (AudioError::Resample("x".into()), PlayerErrorCode::Decode),
            (AudioError::Seek("x".into()), PlayerErrorCode::Decode),
            (AudioError::InvalidArgument("x"), PlayerErrorCode::Decode),
            (
                AudioError::DeviceUnavailable("x".into()),
                PlayerErrorCode::DeviceLost,
            ),
            (
                AudioError::DeviceOpen("x".into()),
                PlayerErrorCode::DeviceLost,
            ),
            (AudioError::DeviceLost, PlayerErrorCode::DeviceLost),
            (
                AudioError::UnsupportedFormat("x".into()),
                PlayerErrorCode::Unsupported,
            ),
            (AudioError::NoSupportedFormat, PlayerErrorCode::Unsupported),
            (
                AudioError::UnsupportedSampleFormat(SampleFormat::F32),
                PlayerErrorCode::Unsupported,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(map_audio_error(error).code, expected);
        }
    }

    #[test]
    fn queue_commands_emit_snapshots_with_queue_fields() {
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::new(sink.clone());

        assert!(actor.queue.view().tracks.is_empty());

        let view = actor.queue_add(vec!["a".into(), "b".into()]).unwrap();
        assert_eq!(view.tracks, vec!["a", "b"]);
        assert_eq!(view.index, Some(0));
        let state = sink.last_state().expect("queue add must emit a snapshot");
        assert_eq!(state.queue_length, Some(2));
        assert_eq!(state.queue_index, Some(0));
        assert_eq!(state.status, PlaybackStatus::Idle);

        let view = actor.queue_add(vec!["c".into()]).unwrap();
        assert_eq!(view.tracks, vec!["a", "b", "c"]);
        assert_eq!(sink.last_state().unwrap().queue_length, Some(3));

        // 删除当前项之外的曲目：当前曲目不变，指针不动。
        let view = actor.queue_remove(1).unwrap();
        assert_eq!(view.tracks, vec!["a", "c"]);
        assert_eq!(view.index, Some(0));
        let state = sink.last_state().unwrap();
        assert_eq!(state.queue_length, Some(2));
        assert_eq!(state.queue_index, Some(0));

        actor
            .set_mode(PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: true,
            })
            .unwrap();
        let state = sink.last_state().unwrap();
        assert_eq!(state.mode.repeat, RepeatMode::All);
        assert!(state.mode.shuffle);

        let view = actor.queue_clear().unwrap();
        assert!(view.tracks.is_empty());
        let state = sink.last_state().unwrap();
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert_eq!(state.queue_length, Some(0));
        assert_eq!(state.queue_index, None);
    }

    #[test]
    fn next_and_prev_on_an_empty_queue_return_unsupported() {
        let mut actor = PlayerActor::new(RecordingSink::default());
        let next = actor.next_track(AdvanceReason::Natural);
        let prev = actor.prev_track();
        for result in [next, prev] {
            let error = result.expect_err("empty queue must not panic or succeed");
            assert_eq!(error.code, PlayerErrorCode::Unsupported);
            assert_eq!(actor.machine.snapshot().status, PlaybackStatus::Idle);
        }
    }

    /// 设备变化触发重开（失败路径）：名称 + 通道数复合判断，失败落 `E_DEVICE_LOST`。
    #[test]
    fn device_change_reopens_and_reports_device_lost_on_failure() {
        let dir = temp_dir("device-change");
        let missing = dir.join("missing.wav").to_string_lossy().into_owned();

        let probe = FixedProbe::new(device("Speakers", 2));
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::with_devices(sink.clone(), Box::new(probe.clone()));

        actor.queue_add(vec![missing.clone()]).unwrap();
        actor.machine.open(&missing, Some(1_000));
        actor.machine.play(0);
        actor.observed_device = Some(device("Speakers", 2));

        // 同一设备：不触发恢复，状态保持 playing。
        actor.check_device_change();
        assert_eq!(actor.machine.snapshot().status, PlaybackStatus::Playing);

        // 名称相同、通道数不同 → 判为变化并尝试重开；文件缺失 → 重开失败。
        probe.set(Some(device("Speakers", 6)));
        actor.check_device_change();

        let state = actor.machine.snapshot();
        assert_eq!(state.status, PlaybackStatus::Error);
        let error = state.error.expect("device loss must carry an error");
        assert_eq!(error.code, PlayerErrorCode::DeviceLost);
        assert!(
            actor.player.is_none(),
            "failed reopen must release the player"
        );
        assert_eq!(actor.queue.position(), Some(0), "queue is kept for a retry");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 设备探测复用 tick 循环，每 4 次（约 1 秒）才比较一次。
    #[test]
    fn device_probe_runs_every_fourth_tick() {
        let probe = FixedProbe::new(device("Speakers", 2));
        let mut actor =
            PlayerActor::with_devices(RecordingSink::default(), Box::new(probe.clone()));
        actor.queue_add(vec!["a".into()]).unwrap();
        actor.machine.open("a", Some(1_000));
        actor.machine.play(0);
        actor.observed_device = Some(device("Speakers", 2));

        // 第 1–3 次 tick 还没到探测周期，设备变化不应触发。
        probe.set(Some(device("Headphones", 2)));
        actor.tick();
        actor.tick();
        actor.tick();
        assert_eq!(actor.machine.snapshot().status, PlaybackStatus::Playing);

        // 第 4 次 tick 探测到变化 → 尝试重开（"a" 不是真实文件 → 失败）。
        actor.tick();
        assert_eq!(actor.machine.snapshot().status, PlaybackStatus::Error);
    }

    /// 设备变化恢复的成功路径：真实设备 + 临时 wav，位置不回零。
    /// 无音频设备的环境（CI）先探测再跳过（T-0016 §7 真机项）。
    #[test]
    fn device_change_reopens_at_the_previous_position() {
        let dir = temp_dir("device-success");
        let track = dir.join("track.wav");
        write_wav(&track, 2_000);
        let track = track.to_string_lossy().into_owned();

        match Player::open(&track) {
            Ok(probe) => drop(probe),
            Err(error) => {
                eprintln!("[test] no audio device ({error}); skipping device recovery test");
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        }

        let probe = FixedProbe::new(device("current", 2));
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::with_devices(sink.clone(), Box::new(probe.clone()));
        actor.queue_add(vec![track.clone()]).unwrap();
        actor.play(Some(track)).expect("track must open");
        actor.seek(700).expect("seek must succeed");
        actor.observed_device = Some(device("stale", 1));

        actor.check_device_change();

        let state = actor.machine.snapshot();
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert!(
            state.position_ms >= 650,
            "position must not restart at 0 (got {})",
            state.position_ms
        );
        assert!(actor.player.is_some(), "recovery keeps a live player");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_failures_skip_at_most_three_tracks_then_report_error() {
        let dir = temp_dir("skip");
        let missing: Vec<String> = (0..5)
            .map(|index| {
                dir.join(format!("missing-{index}.wav"))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        let sink = RecordingSink::default();
        let mut actor = PlayerActor::new(sink.clone());
        actor.queue_add(missing.clone()).unwrap();

        let error = actor
            .play(Some(missing[0].clone()))
            .expect_err("missing file must fail");
        assert_eq!(error.code, PlayerErrorCode::Decode);
        assert_eq!(actor.machine.snapshot().status, PlaybackStatus::Error);
        assert_eq!(
            actor.queue.position(),
            Some(2),
            "exactly three tracks are attempted before giving up"
        );
        let state = sink.last_state().unwrap();
        assert_eq!(state.status, PlaybackStatus::Error);
        assert_eq!(state.queue_length, Some(5));
        assert!(state.error.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_the_current_track_stops_without_autoplay() {
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::new(sink.clone());
        actor
            .queue_add(vec!["a".into(), "b".into(), "c".into()])
            .unwrap();

        // 当前是 a；删除它 → 指针停在 b，但不打开、不播放。
        let view = actor.queue_remove(0).unwrap();
        assert_eq!(view.tracks, vec!["b", "c"]);
        assert_eq!(view.index, Some(0));
        assert!(
            actor.player.is_none(),
            "removing the current track must release the player"
        );
        let state = sink.last_state().unwrap();
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert!(state.track_id.is_none());
        assert_eq!(state.queue_length, Some(2));
        assert_eq!(state.queue_index, Some(0));

        // 删到只剩最后一首再删空：index 归 null、status 保持 idle。
        actor.queue_remove(0).unwrap();
        assert_eq!(actor.queue.current(), Some("c"));
        let view = actor.queue_remove(0).unwrap();
        assert!(view.tracks.is_empty());
        assert_eq!(view.index, None);
        let state = sink.last_state().unwrap();
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert_eq!(state.queue_length, Some(0));
        assert_eq!(state.queue_index, None);
    }

    #[test]
    fn next_on_the_last_track_with_repeat_off_marks_ended_without_moving() {
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::new(sink.clone());
        actor.queue_add(vec!["a".into(), "b".into()]).unwrap();
        assert_eq!(actor.queue.jump(1), Some(1));

        // 队列上没有打开过 player：`repeat = off` 的末尾不越界，置 ended。
        let state = actor
            .next_track(AdvanceReason::Natural)
            .expect("last track must not error");
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert_eq!(actor.queue.position(), Some(1), "cursor stays on the last");
        assert_eq!(state.queue_index, Some(1));
        assert_eq!(sink.last_state().unwrap().status, PlaybackStatus::Ended);

        // `repeat = all` 的下一步会回绕到第一首（决策由纯函数给出）。
        actor
            .set_mode(PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: false,
            })
            .unwrap();
        assert_eq!(actor.queue.next(AdvanceReason::Manual), QueueStep::Open(0));
        assert_eq!(actor.queue.position(), Some(0));
    }

    /// 端到端切歌：真实 `Player` + 临时 wav。
    ///
    /// 覆盖：坏文件自动跳过 → 自然播完进下一首 → 移除当前项停止播放。
    /// 需要音频设备；无设备环境（如 CI 的 Ubuntu runner）会先探测并跳过。
    #[test]
    fn real_player_skips_broken_track_advances_on_end_and_stops_on_removal() {
        let dir = temp_dir("auto-advance");
        let missing = dir.join("missing.wav");
        let first = dir.join("first.wav");
        let second = dir.join("second.wav");
        write_wav(&first, 300);
        write_wav(&second, 800);

        // 探测设备：先试着打开一次，失败说明没有可用输出设备。
        match Player::open(&first) {
            Ok(probe) => drop(probe),
            Err(error) => {
                eprintln!("[test] no audio device ({error}); skipping auto-advance test");
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        }

        let missing = missing.to_string_lossy().into_owned();
        let first = first.to_string_lossy().into_owned();
        let second = second.to_string_lossy().into_owned();
        let sink = RecordingSink::default();
        let mut actor = PlayerActor::new(sink.clone());

        actor
            .queue_add(vec![missing.clone(), first.clone(), second.clone()])
            .unwrap();

        // 坏文件：打开失败后自动跳到下一首并开始播放。
        actor
            .play(Some(missing.clone()))
            .expect("broken track must be skipped");
        assert_eq!(actor.queue.current(), Some(first.as_str()));
        assert_eq!(
            actor.machine.snapshot().status,
            PlaybackStatus::Playing,
            "the next track must start playing"
        );

        // 等自然播完：tick 应该自动切到第二首。
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            actor.tick();
            if actor.queue.current() == Some(second.as_str()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "natural end did not advance the queue"
            );
            thread::sleep(Duration::from_millis(25));
        }

        let state = sink.last_state().expect("switching must emit a snapshot");
        assert_eq!(state.track_id.as_deref(), Some(second.as_str()));
        assert_eq!(state.queue_index, Some(2));
        assert_eq!(state.status, PlaybackStatus::Playing);

        // 移除正在播放的当前项（最后一首）：停止输出、不自动播下一首，
        // 指针退到新的最后一项。
        let view = actor.queue_remove(2).unwrap();
        assert_eq!(view.tracks, vec![missing.clone(), first.clone()]);
        assert_eq!(view.index, Some(1));
        assert!(
            actor.player.is_none(),
            "the live player must be released on removal"
        );
        let state = sink.last_state().unwrap();
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert!(state.track_id.is_none());
        assert_eq!(state.queue_index, Some(1));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写一个 16-bit PCM 单声道 wav（参照 `np-audio/examples/make_wav.rs`）。
    fn write_wav(path: &Path, duration_ms: u32) {
        use std::f32::consts::TAU;
        use std::io::Write;

        let rate = 44_100_u32;
        let frames = rate * duration_ms / 1_000;
        let fade = (rate as f32 * 0.01) as u32;
        let mut samples = Vec::with_capacity(frames as usize);
        for frame in 0..frames {
            let mut value = (TAU * 440.0 * frame as f32 / rate as f32).sin() * 0.2;
            if frame < fade {
                value *= frame as f32 / fade as f32;
            } else if frame + fade > frames {
                value *= (frames - frame) as f32 / fade as f32;
            }
            samples.push((value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
        }

        let data_len = (samples.len() * 2) as u32;
        let mut out = std::io::BufWriter::new(std::fs::File::create(path).expect("create wav"));
        out.write_all(b"RIFF").unwrap();
        out.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        out.write_all(b"WAVE").unwrap();
        out.write_all(b"fmt ").unwrap();
        out.write_all(&16u32.to_le_bytes()).unwrap();
        out.write_all(&1u16.to_le_bytes()).unwrap();
        out.write_all(&1u16.to_le_bytes()).unwrap();
        out.write_all(&rate.to_le_bytes()).unwrap();
        out.write_all(&(rate * 2).to_le_bytes()).unwrap();
        out.write_all(&2u16.to_le_bytes()).unwrap();
        out.write_all(&16u16.to_le_bytes()).unwrap();
        out.write_all(b"data").unwrap();
        out.write_all(&data_len.to_le_bytes()).unwrap();
        for sample in samples {
            out.write_all(&sample.to_le_bytes()).unwrap();
        }
        out.flush().unwrap();
    }
}
