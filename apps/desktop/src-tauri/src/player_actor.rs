//! 播放器 actor：单线程独占 `np_audio::Player`，串行处理命令并广播事件。
//!
//! `np_audio::Player` 是 `!Send`（`Engine` 持有 `Box<dyn AudioSink>`，trait 没有
//! `Send` 约束），因此 actor 用 `std::thread` + `std::sync::mpsc::recv_timeout`
//! 在同一个循环里处理命令与 4 Hz tick（ARCH-006 §4 / T-0013 §7）。
//!
//! `Player` 的创建与 drop 都发生在 actor 线程；退出时先 drop 再跳出循环。

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use np_audio::{AudioError, Player, clamp_volume};
use np_core::{
    AudioFailure, POSITION_TICK_MS, PlaybackMachine, PlaybackState, PlaybackStatus, PlayerCommand,
    PlayerError, PlayerEvent,
};
use tauri::{AppHandle, Emitter};

/// 状态快照事件（开发方案 §4.3）。
pub(crate) const EVT_PLAYER: &str = "evt.player";
/// 4 Hz 位置事件（开发方案 §4.3）。
pub(crate) const EVT_POSITION: &str = "evt.position";

/// 命令通道容量：满了立即回 `E_BUSY`，不阻塞调用方。
const COMMAND_CAPACITY: usize = 32;
/// 单条命令等待 actor 应答的上限；超时按 `E_BUSY` 返回。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// actor 的 tick 周期（4 Hz）。
const TICK_INTERVAL: Duration = Duration::from_millis(POSITION_TICK_MS);

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

    /// 打开 `path` 并播放；`path = None` 时继续当前曲目。
    pub(crate) fn play(&self, path: Option<String>) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Play { path })
    }

    /// 暂停。
    pub(crate) fn pause(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Pause)
    }

    /// 下一首（队列未实现，固定 `E_UNSUPPORTED`）。
    pub(crate) fn next(&self) -> Result<PlaybackState, PlayerError> {
        self.request(PlayerCommand::Next)
    }

    /// 上一首（队列未实现，固定 `E_UNSUPPORTED`）。
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

    /// 发送一条命令并等待一次性应答。
    fn request(&self, command: PlayerCommand) -> Result<PlaybackState, PlayerError> {
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
    reply: SyncSender<Result<PlaybackState, PlayerError>>,
}

/// actor 控制通道的消息。
enum ActorMessage {
    Command(PendingCommand),
    Shutdown,
}

/// 独占 `Player` 的单线程状态机。
struct PlayerActor {
    app: AppHandle,
    machine: PlaybackMachine,
    player: Option<Player>,
}

impl PlayerActor {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            machine: PlaybackMachine::new(),
            player: None,
        }
    }

    /// 主循环：命令优先，最多每 250 ms 跑一次 tick。
    fn run(mut self, rx: Receiver<ActorMessage>) {
        let mut last_tick = Instant::now();
        loop {
            let mut batch = Vec::new();
            let mut shutdown = false;

            match rx.recv_timeout(TICK_INTERVAL) {
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

            if last_tick.elapsed() >= TICK_INTERVAL {
                self.tick();
                last_tick = Instant::now();
            }
        }

        // 必须在本线程释放 cpal 流与解码线程（T-0013 §7）。
        self.player = None;
    }

    /// 批量执行命令；`seek` / `volume` 只执行最后一次（latest-wins）。
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

    fn execute(&mut self, command: PlayerCommand) -> Result<PlaybackState, PlayerError> {
        match command {
            PlayerCommand::Snapshot => self.snapshot(),
            PlayerCommand::Play { path } => self.play(path),
            PlayerCommand::Pause => self.pause(),
            PlayerCommand::Next => Err(PlayerError::unsupported("队列未实现（T-0015）")),
            PlayerCommand::Prev => Err(PlayerError::unsupported("队列未实现（T-0015）")),
            PlayerCommand::Seek { position_ms } => self.seek(position_ms),
            PlayerCommand::SetVolume { volume } => self.set_volume(volume),
            PlayerCommand::Shutdown => {
                Err(PlayerError::unsupported("Shutdown 由 actor 控制通道处理"))
            }
        }
    }

    fn snapshot(&mut self) -> Result<PlaybackState, PlayerError> {
        let position = self.position_ms();
        self.machine.sync_position(position);
        Ok(self.machine.snapshot())
    }

    fn play(&mut self, path: Option<String>) -> Result<PlaybackState, PlayerError> {
        match path {
            Some(path) => self.open_and_play(path),
            None => self.resume(),
        }
    }

    /// 打开新文件；只有打开成功才替换旧 `Player`（失败时旧曲目继续）。
    fn open_and_play(&mut self, path: String) -> Result<PlaybackState, PlayerError> {
        let volume = self.machine.snapshot().volume;
        let player = Player::open(&path).map_err(map_audio_error)?;
        player.set_volume(volume);
        let duration_ms = player.duration_ms();

        self.player = Some(player);
        let event = self.machine.open(path, duration_ms);
        self.emit(event);

        let play_result = match self.player.as_ref() {
            Some(player) => player.play().map_err(map_audio_error),
            None => Err(PlayerError::busy("播放器状态丢失")),
        };
        if let Err(error) = play_result {
            let position = self.position_ms();
            self.fail(error.clone(), position);
            return Err(error);
        }

        let event = self.machine.play(0);
        self.emit(event);
        Ok(self.machine.snapshot())
    }

    /// 继续当前曲目；播完后重新 `play` 先回到 0。
    fn resume(&mut self) -> Result<PlaybackState, PlayerError> {
        if self.player.is_none() {
            return Err(PlayerError::unsupported("没有可播放的曲目，请先提供 path"));
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

    /// 4 Hz tick：只在 `Playing` 时发位置；结束/错误立即发完整快照。
    fn tick(&mut self) {
        let Some(player) = self.player.as_ref() else {
            return;
        };
        let message = player.last_error();
        let position = player.position_ms();
        let ended = player.is_ended();

        if let Some(message) = message {
            self.fail(PlayerError::decode(message), position);
            return;
        }

        if let Some(event) = self.machine.tick(position, ended) {
            self.emit(event);
        }
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
        let result = match event {
            PlayerEvent::StateChanged(state) => self.app.emit(EVT_PLAYER, state),
            PlayerEvent::PositionTick(tick) => self.app.emit(EVT_POSITION, tick),
        };
        if let Err(error) = result {
            eprintln!("[player] emit failed: {error}");
        }
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
