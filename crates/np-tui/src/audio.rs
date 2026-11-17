//! TUI 的播放器适配：本地文件真正打开，队列保存可播放路径。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};

use crate::model::TrackRef;
use crate::playback::{PlaybackQueue, QueueItem, TrackKey};

/// 直播的 ring 窗口：比本地文件的 300 ms 更短，启动更快、更贴近 live edge。
pub(crate) const LIVE_RING_MS: u32 = 150;
const MAX_RECONNECT_ATTEMPTS: u32 = 6;
const RECONNECT_DELAYS_SECS: [u64; 6] = [1, 2, 5, 10, 20, 30];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveState {
    Idle,
    Preparing,
    Playing,
    Paused,
    Reconnecting,
}

#[derive(Debug)]
struct LiveFailure {
    message: String,
    retryable: bool,
}

type LivePreparation = (u64, Result<np_live::PreparedLive, LiveFailure>);

fn reconnect_delay(attempt: u32) -> Duration {
    let index = usize::try_from(attempt.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(RECONNECT_DELAYS_SECS.len() - 1);
    Duration::from_secs(RECONNECT_DELAYS_SECS[index])
}

pub struct TuiAudio {
    player: RefCell<Option<Player>>,
    tap: RefCell<Option<SampleTapHandle>>,
    playing: Cell<bool>,
    title: RefCell<String>,
    artist: RefCell<String>,
    duration_ms: Cell<Option<u64>>,
    source_path: RefCell<Option<PathBuf>>,
    volume: Cell<f32>,
    pending_metadata: RefCell<Option<(String, String, u64)>>,
    queue: Mutex<PlaybackQueue>,
    live_state: Cell<LiveState>,
    live_generation: Cell<u64>,
    live_rx: RefCell<Option<std::sync::mpsc::Receiver<LivePreparation>>>,
    live_cancel: RefCell<Option<Arc<AtomicBool>>>,
    live_room: Cell<u64>,
    live_note: RefCell<Option<String>>,
    live_metrics: RefCell<Option<np_live::LiveMetrics>>,
    live_control: RefCell<Option<np_live::LiveControl>>,
    reconnect_attempt: Cell<u32>,
    reconnect_at: Cell<Option<Instant>>,
    login: RefCell<crate::login::LoginUi>,
    bili_auth: RefCell<Option<np_live::BiliAuth>>,
    auth_loaded: Cell<bool>,
}

impl TuiAudio {
    pub fn from_env() -> Self {
        let audio = Self::empty();
        let Some(path) = std::env::var_os("NP_TUI_AUDIO").map(PathBuf::from) else {
            return audio;
        };
        if audio.open_path(&path).is_ok() {
            if let Ok(title) = std::env::var("NP_TUI_TITLE") {
                *audio.title.borrow_mut() = title;
            }
            if let Ok(artist) = std::env::var("NP_TUI_ARTIST") {
                *audio.artist.borrow_mut() = artist;
            }
        }
        audio
    }

    pub fn empty() -> Self {
        Self {
            player: RefCell::new(None),
            tap: RefCell::new(None),
            playing: Cell::new(false),
            title: RefCell::new("未播放".to_owned()),
            artist: RefCell::new("Nowhere Player".to_owned()),
            duration_ms: Cell::new(None),
            source_path: RefCell::new(None),
            volume: Cell::new(1.0),
            pending_metadata: RefCell::new(None),
            queue: Mutex::new(PlaybackQueue::new()),
            live_state: Cell::new(LiveState::Idle),
            live_generation: Cell::new(0),
            live_rx: RefCell::new(None),
            live_cancel: RefCell::new(None),
            live_room: Cell::new(0),
            live_note: RefCell::new(None),
            live_metrics: RefCell::new(None),
            live_control: RefCell::new(None),
            reconnect_attempt: Cell::new(0),
            reconnect_at: Cell::new(None),
            login: RefCell::new(crate::login::LoginUi::new()),
            bili_auth: RefCell::new(None),
            auth_loaded: Cell::new(false),
        }
    }

    fn open_path(&self, path: &Path) -> Result<(), CmdError> {
        if self.is_live() {
            self.stop_live();
        }
        let player = Player::open(path)
            .map_err(|error| CmdError::new("E_AUDIO", format!("打开音频失败：{error}")))?;
        player.set_volume(self.volume.get());
        let tap = player.sample_tap();
        player
            .play()
            .map_err(|error| CmdError::new("E_AUDIO", format!("播放失败：{error}")))?;
        *self.title.borrow_mut() = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_owned());
        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
        self.duration_ms.set(player.duration_ms());
        *self.source_path.borrow_mut() = Some(path.to_path_buf());
        *self.tap.borrow_mut() = Some(tap);
        *self.player.borrow_mut() = Some(player);
        self.playing.set(true);
        Ok(())
    }

    /// 当前是否处于直播独占模式（准备中或播放中）。
    pub(crate) fn is_live(&self) -> bool {
        self.live_state.get() != LiveState::Idle
    }

    #[cfg(test)]
    pub(crate) fn live_state(&self) -> LiveState {
        self.live_state.get()
    }

    #[cfg(test)]
    pub(crate) fn live_room(&self) -> u64 {
        self.live_room.get()
    }

    /// 直播队列里还没播的音频长度（毫秒）；非直播或还没接上时 `None`。
    ///
    /// 目标 1500ms、上限 2000ms、下限 1200ms（T-0091 B1.2g）。
    pub(crate) fn live_buffer_ms(&self) -> Option<u64> {
        if !self.is_live() {
            return None;
        }
        self.live_metrics
            .borrow()
            .as_ref()
            .map(np_live::LiveMetrics::buffer_ms)
    }

    /// 直播诊断标签：队列、under-run、ring under-run、drop、解码错误、读停顿。
    ///
    /// 例：`LIVE 1500ms U:0 Q:0 R:0 D:0 E:0 F:0 S:0`；暂停时 `LIVE PAUSED`。
    pub(crate) fn live_status_label(&self) -> Option<String> {
        if !self.is_live() {
            return None;
        }
        if self.live_state.get() == LiveState::Reconnecting {
            return Some(format!(
                "LIVE RECONNECTING {}/{}",
                self.reconnect_attempt.get(),
                MAX_RECONNECT_ATTEMPTS
            ));
        }
        if self.live_state.get() == LiveState::Paused {
            return Some(match self.live_metrics.borrow().as_ref() {
                Some(metrics) => {
                    let control = self.live_control.borrow();
                    let paused = control
                        .as_ref()
                        .is_some_and(np_live::LiveControl::is_paused);
                    let paused_drops = control
                        .as_ref()
                        .map(np_live::LiveControl::paused_dropped_frames)
                        .unwrap_or(0);
                    format!(
                        "LIVE PAUSED {}ms F:{} C:{} P:{}",
                        metrics.buffer_ms(),
                        metrics.frames_received(),
                        u8::from(paused),
                        paused_drops
                    )
                }
                None => "LIVE PAUSED".to_owned(),
            });
        }
        let metrics = self.live_metrics.borrow();
        let metrics = metrics.as_ref()?;
        let ring_underruns = self
            .player
            .borrow()
            .as_ref()
            .map(Player::underruns)
            .unwrap_or(0);
        Some(format!(
            "LIVE {}ms U:{} Q:{} R:{} D:{} E:{} F:{} S:{}",
            metrics.buffer_ms(),
            metrics.underruns(),
            metrics.queue_empty_events(),
            ring_underruns,
            metrics.drop_events(),
            metrics.decode_errors(),
            metrics.fill_silence_ms(),
            metrics.max_read_stall_ms()
        ))
    }

    /// 取走一次性的直播状态文本（成功 / 失败 / 结束）。
    pub(crate) fn take_live_note(&self) -> Option<String> {
        self.live_note.borrow_mut().take()
    }

    pub(crate) fn start_bili_login(&self) {
        self.login.borrow_mut().start();
    }

    pub(crate) fn start_bili_logout(&self) -> String {
        let note = self.login.borrow_mut().logout();
        *self.bili_auth.borrow_mut() = None;
        self.auth_loaded.set(true);
        note
    }

    pub(crate) fn login_active(&self) -> bool {
        self.login.borrow().active()
    }

    pub(crate) fn poll_login(&self) -> bool {
        let changed = self.login.borrow_mut().poll();
        if let Some(auth) = self.login.borrow().auth() {
            *self.bili_auth.borrow_mut() = Some(auth);
            self.auth_loaded.set(true);
        }
        changed
    }

    pub(crate) fn handle_login_key(&self, key: crossterm::event::KeyCode) -> bool {
        self.login.borrow_mut().handle_key(key)
    }

    pub(crate) fn draw_login(&self, frame: &mut ratatui::Frame, palette: &crate::theme::Palette) {
        self.login.borrow().draw(frame, frame.area(), palette);
    }

    fn auth_for_live(&self) -> Option<np_live::BiliAuth> {
        if !self.auth_loaded.get() {
            self.auth_loaded.set(true);
            match np_live::AuthStore::new(crate::login::auth_path()).load() {
                Ok(Some(auth)) => *self.bili_auth.borrow_mut() = Some(auth),
                Ok(None) => {}
                Err(error) => {
                    *self.live_note.borrow_mut() = Some(format!("B站登录态读取失败：{error}"));
                }
            }
        }
        self.bili_auth.borrow().clone()
    }

    /// 开始一路直播：停止当前播放、保留队列，后台准备网络与首帧。
    ///
    /// 本方法不碰网络；准备线程完成后由 [`poll_live`](Self::poll_live) 在主线程
    /// 打开 `Player`。
    pub(crate) fn start_live(&self, room_id: u64) {
        self.start_live_inner(room_id, true);
    }

    fn start_live_inner(&self, room_id: u64, reset_reconnect: bool) {
        if reset_reconnect {
            self.reconnect_attempt.set(0);
        }
        self.reconnect_at.set(None);
        self.stop_playback();
        let generation = self.live_generation.get().wrapping_add(1);
        self.live_generation.set(generation);
        self.live_state.set(LiveState::Preparing);
        self.live_room.set(room_id);
        let auth = self.auth_for_live();
        *self.title.borrow_mut() = format!("直播 {room_id} 准备中…");
        *self.artist.borrow_mut() = "B站直播".to_owned();
        self.duration_ms.set(None);
        self.live_note.borrow_mut().take();
        *self.live_metrics.borrow_mut() = None;

        let cancel = Arc::new(AtomicBool::new(false));
        *self.live_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (tx, rx) = std::sync::mpsc::channel();
        *self.live_rx.borrow_mut() = Some(rx);

        let spawn = std::thread::Builder::new()
            .name("np-live-prepare".to_owned())
            .spawn(move || {
                let result = prepare_live(room_id, &cancel, auth);
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                let _ = tx.send((generation, result));
            });
        if let Err(error) = spawn {
            self.live_state.set(LiveState::Idle);
            *self.live_rx.borrow_mut() = None;
            *self.live_cancel.borrow_mut() = None;
            *self.title.borrow_mut() = "未播放".to_owned();
            *self.artist.borrow_mut() = "Nowhere Player".to_owned();
            *self.live_note.borrow_mut() = Some(format!("直播准备线程启动失败：{error}"));
        } else if !reset_reconnect {
            *self.live_note.borrow_mut() = Some(format!(
                "直播重连中（{}/{}）",
                self.reconnect_attempt.get(),
                MAX_RECONNECT_ATTEMPTS
            ));
        }
    }

    /// 主线程每 tick 调用：接住准备结果并打开播放器。
    pub(crate) fn poll_live(&self) -> bool {
        let received = {
            let receiver = self.live_rx.borrow();
            match receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(value) => Some(value),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                        self.live_generation.get(),
                        Err(LiveFailure {
                            message: "直播准备线程意外结束".to_owned(),
                            retryable: true,
                        }),
                    )),
                },
                None => None,
            }
        };
        let Some((generation, result)) = received else {
            return false;
        };
        if generation != self.live_generation.get() {
            return false;
        }
        *self.live_rx.borrow_mut() = None;
        *self.live_cancel.borrow_mut() = None;

        match result {
            Ok(prepared) => {
                let metrics = prepared.metrics.clone();
                let control = prepared.control.clone();
                let reconnected = self.reconnect_attempt.get() > 0;
                self.reconnect_attempt.set(0);
                self.reconnect_at.set(None);
                let title = if prepared.title.trim().is_empty() {
                    format!("直播 {}", prepared.room_id)
                } else {
                    prepared.title
                };
                match Player::open_source_with_ring(prepared.source, LIVE_RING_MS) {
                    Ok(player) => {
                        player.set_volume(self.volume.get());
                        let tap = player.sample_tap();
                        if let Err(error) = player.play() {
                            self.live_state.set(LiveState::Idle);
                            *self.title.borrow_mut() = "未播放".to_owned();
                            *self.artist.borrow_mut() = "Nowhere Player".to_owned();
                            *self.live_note.borrow_mut() = Some(format!("直播播放失败：{error}"));
                            return true;
                        }
                        *self.tap.borrow_mut() = Some(tap);
                        *self.player.borrow_mut() = Some(player);
                        self.playing.set(true);
                        self.duration_ms.set(None);
                        self.live_state.set(LiveState::Playing);
                        *self.live_metrics.borrow_mut() = Some(metrics);
                        *self.live_control.borrow_mut() = Some(control);
                        *self.title.borrow_mut() = title.clone();
                        *self.artist.borrow_mut() = "B站直播".to_owned();
                        *self.live_note.borrow_mut() = Some(if reconnected {
                            format!("直播已重连：{title}")
                        } else {
                            format!("直播：{title}")
                        });
                    }
                    Err(error) => {
                        self.live_state.set(LiveState::Idle);
                        *self.title.borrow_mut() = "未播放".to_owned();
                        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
                        *self.live_note.borrow_mut() = Some(format!("直播音频输出失败：{error}"));
                    }
                }
            }
            Err(failure) => {
                if failure.retryable && self.live_state.get() != LiveState::Idle {
                    self.begin_reconnect(Some(failure.message));
                } else {
                    self.fail_live(format!("直播失败：{}", failure.message));
                }
            }
        }
        true
    }

    fn begin_reconnect(&self, reason: Option<String>) {
        if !self.is_live() {
            return;
        }
        let room = self.live_room.get();
        if room == 0 {
            self.fail_live("直播重连失败：房间号丢失".to_owned());
            return;
        }
        let attempt = self.reconnect_attempt.get().saturating_add(1);
        if attempt > MAX_RECONNECT_ATTEMPTS {
            self.fail_live(format!(
                "直播重连失败（已尝试 {MAX_RECONNECT_ATTEMPTS} 次）"
            ));
            return;
        }
        self.reconnect_attempt.set(attempt);
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        *self.live_metrics.borrow_mut() = None;
        *self.live_control.borrow_mut() = None;
        self.playing.set(false);
        self.live_state.set(LiveState::Reconnecting);
        let delay = reconnect_delay(attempt);
        self.reconnect_at.set(Some(Instant::now() + delay));
        let reason = reason.unwrap_or_else(|| "连接中断".to_owned());
        *self.live_note.borrow_mut() = Some(format!(
            "直播断开：{reason}，{}s 后重连（{}/{}）",
            delay.as_secs(),
            attempt,
            MAX_RECONNECT_ATTEMPTS
        ));
    }

    fn fail_live(&self, note: String) {
        self.stop_live();
        *self.live_note.borrow_mut() = Some(note);
    }

    /// `Esc`：停止直播（best-effort），队列保留。
    ///
    /// 若解码线程正阻塞在单次 HTTP read 上，drop `Player` 会等这次 read
    /// 返回；正常网络下应在 2 秒内完成。
    pub(crate) fn stop_live(&self) -> bool {
        if !self.is_live() {
            return false;
        }
        self.shutdown_live();
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        self.playing.set(false);
        self.duration_ms.set(None);
        *self.source_path.borrow_mut() = None;
        *self.title.borrow_mut() = "未播放".to_owned();
        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
        self.live_room.set(0);
        self.reconnect_attempt.set(0);
        self.reconnect_at.set(None);
        true
    }

    /// 取消准备、丢弃结果，但不重置标题等展示字段。
    fn shutdown_live(&self) {
        if let Some(cancel) = self.live_cancel.borrow_mut().take() {
            cancel.store(true, Ordering::Release);
        }
        *self.live_rx.borrow_mut() = None;
        *self.live_metrics.borrow_mut() = None;
        *self.live_control.borrow_mut() = None;
        self.reconnect_at.set(None);
        self.live_state.set(LiveState::Idle);
    }

    /// 停止当前播放（本地或直播），不动队列。
    fn stop_playback(&self) {
        self.shutdown_live();
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        self.playing.set(false);
        self.duration_ms.set(None);
        *self.source_path.borrow_mut() = None;
    }

    pub fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub fn artist(&self) -> String {
        self.artist.borrow().clone()
    }

    pub fn duration_ms(&self) -> Option<u64> {
        if self.is_live() {
            return None;
        }
        self.player
            .borrow()
            .as_ref()
            .and_then(|player| player.duration_ms())
            .or(self.duration_ms.get())
    }

    pub fn position_ms(&self) -> u64 {
        self.player
            .borrow()
            .as_ref()
            .map_or(0, |player| player.position_ms())
    }

    pub fn is_playing(&self) -> bool {
        self.playing.get()
            && self
                .player
                .borrow()
                .as_ref()
                .is_some_and(|player| !player.is_ended())
    }

    pub fn tap(&self) -> Option<SampleTapHandle> {
        self.tap.borrow().clone()
    }

    #[cfg(test)]
    pub fn source_path(&self) -> Option<PathBuf> {
        self.source_path.borrow().clone()
    }

    pub fn volume(&self) -> f32 {
        self.volume.get()
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(player) = self.player.borrow().as_ref() {
            player.set_volume(volume);
        }
        self.volume.set(volume);
    }

    pub fn set_pending_metadata(&self, title: String, artist: String, duration_ms: u64) {
        *self.pending_metadata.borrow_mut() = Some((title, artist, duration_ms));
    }

    pub fn queue_items_snapshot(&self) -> Vec<QueueItem> {
        self.lock_queue().items().to_vec()
    }

    pub fn can_next(&self) -> bool {
        !self.is_live() && self.lock_queue().can_next()
    }

    pub fn can_prev(&self) -> bool {
        !self.is_live() && self.lock_queue().can_prev()
    }

    pub fn can_seek_back(&self) -> bool {
        !self.is_live() && self.position_ms() > 0
    }

    pub fn can_seek_forward(&self) -> bool {
        !self.is_live()
            && self
                .duration_ms()
                .is_some_and(|duration| duration > 0 && self.position_ms() < duration)
    }

    pub fn mode(&self) -> PlaybackMode {
        self.lock_queue().mode().to_playback_mode()
    }

    pub fn mode_id(&self) -> &'static str {
        match self.mode() {
            PlaybackMode {
                repeat: np_core::RepeatMode::One,
                shuffle: false,
            } => "single",
            PlaybackMode {
                repeat: np_core::RepeatMode::Off,
                shuffle: true,
            } => "shuffle",
            PlaybackMode {
                repeat: np_core::RepeatMode::All,
                shuffle: false,
            } => "list",
            _ => "sequential",
        }
    }

    pub fn set_mode_id(&self, id: &str) -> bool {
        let mode = match id {
            "single" => crate::playback::QueueMode::SingleLoop,
            "shuffle" => crate::playback::QueueMode::Shuffle,
            "sequential" => crate::playback::QueueMode::Sequential,
            "list" => crate::playback::QueueMode::ListLoop,
            _ => return false,
        };
        self.lock_queue().set_mode(mode);
        true
    }

    pub fn cycle_mode(&self) -> PlaybackMode {
        let mut queue = self.lock_queue();
        let next = match queue.mode() {
            crate::playback::QueueMode::SingleLoop => crate::playback::QueueMode::Shuffle,
            crate::playback::QueueMode::Shuffle => crate::playback::QueueMode::Sequential,
            crate::playback::QueueMode::Sequential => crate::playback::QueueMode::ListLoop,
            crate::playback::QueueMode::ListLoop => crate::playback::QueueMode::SingleLoop,
        };
        queue.set_mode(next);
        next.to_playback_mode()
    }

    fn lock_queue(&self) -> std::sync::MutexGuard<'_, PlaybackQueue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn queue_item(path: &str, _origin: &str) -> QueueItem {
        let path_buf = PathBuf::from(path);
        let title = path_buf
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_owned());
        QueueItem::new(TrackRef::Local(path_buf), title, "Nowhere Player", 0)
    }

    fn open_queue_item(&self, index: usize) -> Result<(), CmdError> {
        let item = self
            .lock_queue()
            .item_at(index)
            .cloned()
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "队列序号越界"))?;
        match item.track {
            TrackRef::Local(path) => {
                self.open_path(&path)?;
                if !item.title.is_empty() {
                    *self.title.borrow_mut() = item.title;
                }
                if !item.artist.is_empty() {
                    *self.artist.borrow_mut() = item.artist;
                }
                if item.duration_ms > 0 {
                    self.duration_ms.set(Some(item.duration_ms));
                } else if let Some(duration_ms) = self.duration_ms.get() {
                    self.lock_queue().set_duration(index, duration_ms);
                }
                Ok(())
            }
            TrackRef::Remote { .. } => {
                Err(CmdError::new("E_SRC_UNAVAILABLE", "远程队列项需要在线解析"))
            }
        }
    }

    pub fn play_or_prepend(&self, item: QueueItem) -> Result<PlaybackState, CmdError> {
        let index = self.lock_queue().play_or_prepend(item);
        self.open_queue_item(index)?;
        Ok(self.state())
    }

    pub fn replace_playlist(
        &self,
        items: Vec<QueueItem>,
        selected: Option<&TrackKey>,
    ) -> Result<PlaybackState, CmdError> {
        let index = self.lock_queue().replace_playlist(items, selected);
        if let Some(index) = index {
            self.open_queue_item(index)?;
        }
        Ok(self.state())
    }

    pub fn toggle(&mut self) {
        if self.is_live() {
            match self.live_state.get() {
                LiveState::Playing => {
                    self.pause_live();
                }
                LiveState::Paused => {
                    self.resume_live();
                }
                LiveState::Idle | LiveState::Preparing | LiveState::Reconnecting => {}
            }
            return;
        }
        let player = self.player.borrow();
        let Some(player) = player.as_ref() else {
            return;
        };
        if self.playing.get() {
            let _ = player.pause();
            self.playing.set(false);
        } else {
            let _ = player.play();
            self.playing.set(true);
        }
    }

    /// 暂停直播：保留 Player/解码器，网络继续读；缓存超过 30s 丢新帧。
    fn pause_live(&self) {
        if self.live_state.get() != LiveState::Playing {
            return;
        }
        if let Some(control) = self.live_control.borrow().as_ref() {
            control.pause();
        }
        if let Some(player) = self.player.borrow().as_ref() {
            let _ = player.pause();
        }
        self.playing.set(false);
        self.duration_ms.set(None);
        self.live_state.set(LiveState::Paused);
        *self.live_note.borrow_mut() = Some("直播已暂停，继续缓存".to_owned());
    }

    /// 从暂停恢复：继续播暂停缓存；缓存播完后自动追 live。
    fn resume_live(&self) {
        if self.live_state.get() != LiveState::Paused {
            return;
        }
        if let Some(control) = self.live_control.borrow().as_ref() {
            control.resume();
        }
        let played = if let Some(player) = self.player.borrow().as_ref() {
            match player.play() {
                Ok(()) => true,
                Err(error) => {
                    *self.live_note.borrow_mut() = Some(format!("直播恢复失败，重新连接：{error}"));
                    false
                }
            }
        } else {
            false
        };
        if played {
            self.playing.set(true);
            self.live_state.set(LiveState::Playing);
            *self.live_note.borrow_mut() = Some("直播已恢复（先播缓存）".to_owned());
            return;
        }
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        *self.live_control.borrow_mut() = None;
        *self.live_metrics.borrow_mut() = None;
        // 没有 Player（异常/测试）才回退到重连。
        let room = self.live_room.get();
        if room == 0 {
            self.live_state.set(LiveState::Idle);
            *self.live_note.borrow_mut() = Some("直播恢复失败：房间号丢失".to_owned());
            return;
        }
        self.start_live(room);
    }

    /// F5：丢弃暂停缓存，重新接 live edge。
    pub(crate) fn refresh_live(&self) -> bool {
        if !self.is_live() {
            return false;
        }
        let room = self.live_room.get();
        if room == 0 {
            return false;
        }
        self.start_live(room);
        true
    }

    fn queue_view(&self) -> QueueView {
        let queue = self.lock_queue();
        QueueView {
            tracks: queue
                .items()
                .iter()
                .filter_map(|item| match &item.track {
                    TrackRef::Local(path) => Some(path.to_string_lossy().into_owned()),
                    TrackRef::Remote { .. } => None,
                })
                .collect(),
            index: queue.current_index(),
        }
    }

    fn state(&self) -> PlaybackState {
        let (mode, queue_length, queue_index) = {
            let queue = self.lock_queue();
            (
                queue.mode().to_playback_mode(),
                queue.len(),
                queue.current_index(),
            )
        };
        PlaybackState {
            status: if self.playing.get() {
                PlaybackStatus::Playing
            } else if self.live_state.get() == LiveState::Paused
                || self.live_state.get() == LiveState::Reconnecting
                || self.player.borrow().is_some()
            {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Idle
            },
            track_id: if self.is_live()
                && (self.live_state.get() == LiveState::Paused
                    || self.live_state.get() == LiveState::Reconnecting)
            {
                Some(self.title())
            } else {
                self.player.borrow().as_ref().map(|_| self.title())
            },
            position_ms: self.position_ms(),
            duration_ms: self.duration_ms(),
            volume: self.volume.get(),
            mode,
            error: None,
            queue_length: Some(queue_length),
            queue_index,
        }
    }

    fn step(&self, forward: bool) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            return Err(CmdError::new("E_CMD_ARG", "直播中不可切换歌曲"));
        }
        let (index, same) = {
            let mut queue = self.lock_queue();
            if queue.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "队列为空"));
            }
            let before = queue.current_index();
            let index = if forward { queue.next() } else { queue.prev() };
            (index, index == before)
        };
        let Some(index) = index else {
            return Ok(self.state());
        };
        if !same {
            self.open_queue_item(index)?;
        }
        Ok(self.state())
    }
    pub fn handle_ended(&self) -> Result<bool, CmdError> {
        match self.live_state.get() {
            LiveState::Idle => {}
            LiveState::Preparing => return Ok(false),
            LiveState::Reconnecting => {
                let Some(at) = self.reconnect_at.get() else {
                    return Ok(false);
                };
                if Instant::now() < at {
                    return Ok(false);
                }
                let room = self.live_room.get();
                if room == 0 {
                    self.fail_live("直播重连失败：房间号丢失".to_owned());
                    return Ok(true);
                }
                self.reconnect_at.set(None);
                self.start_live_inner(room, false);
                return Ok(true);
            }
            LiveState::Paused => {
                let (ended, error) = {
                    let player = self.player.borrow();
                    player.as_ref().map_or((false, None), |player| {
                        (player.is_ended(), player.last_error())
                    })
                };
                if ended || error.is_some() {
                    let mut note = self.live_note.borrow_mut();
                    if note.is_none() {
                        *note = Some("直播连接已断开，恢复时重新连接".to_owned());
                    }
                    return Ok(true);
                }
                return Ok(false);
            }
            LiveState::Playing => {
                let (ended, error) = {
                    let player = self.player.borrow();
                    player.as_ref().map_or((false, None), |player| {
                        (player.is_ended(), player.last_error())
                    })
                };
                if ended || error.is_some() {
                    self.begin_reconnect(error);
                    return Ok(true);
                }
                return Ok(false);
            }
        }
        let ended = self
            .player
            .borrow()
            .as_ref()
            .is_some_and(|player| player.is_ended());
        if !ended {
            return Ok(false);
        }
        let next = {
            let mut queue = self.lock_queue();
            queue.next()
        };
        if let Some(index) = next {
            self.open_queue_item(index)?;
        } else {
            *self.player.borrow_mut() = None;
            *self.tap.borrow_mut() = None;
            self.playing.set(false);
            *self.source_path.borrow_mut() = None;
        }
        Ok(true)
    }
}

impl PlayerApi for TuiAudio {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        if path.is_none() && self.live_state.get() == LiveState::Paused {
            self.resume_live();
            return Ok(self.state());
        }
        if let Some(path) = path {
            let pending = self.pending_metadata.borrow_mut().take();
            let mut item = Self::queue_item(&path, "play");
            if let Some((title, artist, duration_ms)) = pending {
                item.title = title;
                item.artist = artist;
                item.duration_ms = duration_ms;
            }
            let index = self.lock_queue().play_or_prepend(item);
            self.open_queue_item(index)?;
        } else if let Some(player) = self.player.borrow().as_ref() {
            player
                .play()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
            self.playing.set(true);
        }
        Ok(self.state())
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            self.pause_live();
            return Ok(self.state());
        }
        if let Some(player) = self.player.borrow().as_ref() {
            player
                .pause()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        self.playing.set(false);
        Ok(PlaybackState {
            status: PlaybackStatus::Paused,
            ..self.state()
        })
    }

    fn resume(&self) -> Result<PlaybackState, CmdError> {
        if self.live_state.get() == LiveState::Paused {
            self.resume_live();
            return Ok(self.state());
        }
        self.play(None)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.step(true)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.step(false)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            return Err(CmdError::new("E_CMD_ARG", "直播中不可跳转"));
        }
        if let Some(player) = self.player.borrow().as_ref() {
            player
                .seek(position_ms)
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        Ok(self.state())
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(player) = self.player.borrow().as_ref() {
            player.set_volume(volume);
        }
        self.volume.set(volume);
        Ok(self.state())
    }

    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.lock_queue()
            .set_mode(crate::playback::QueueMode::from_playback_mode(mode));
        Ok(self.state())
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        Ok(self.queue_view())
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        {
            let mut queue = self.lock_queue();
            for path in paths {
                queue.push_back(Self::queue_item(&path, "queue add"));
            }
        }
        Ok(self.queue_view())
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        self.lock_queue().remove(index);
        Ok(self.queue_view())
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        if self.is_live() {
            self.stop_live();
        }
        self.lock_queue().clear();
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        self.playing.set(false);
        *self.title.borrow_mut() = "未播放".to_owned();
        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
        self.duration_ms.set(None);
        *self.source_path.borrow_mut() = None;
        Ok(self.queue_view())
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        if self.lock_queue().jump(index).is_none() {
            return Err(CmdError::new("E_CMD_ARG", "队列序号越界"));
        }
        self.open_queue_item(index)?;
        Ok(self.queue_view())
    }
}

fn prepare_live(
    room_id: u64,
    cancel: &AtomicBool,
    auth: Option<np_live::BiliAuth>,
) -> Result<np_live::PreparedLive, LiveFailure> {
    np_live::prepare_default_auth(room_id, cancel, auth.as_ref()).map_err(|error| LiveFailure {
        message: error.user_message(),
        retryable: error.is_retryable(),
    })
}

#[cfg(test)]
impl TuiAudio {
    /// 只设置直播展示状态，不打开设备 / 网络；供 TUI 状态与渲染测试使用。
    pub(crate) fn live_for_test(&self, room_id: u64, title: &str) {
        self.live_state.set(LiveState::Playing);
        self.live_room.set(room_id);
        self.duration_ms.set(None);
        *self.title.borrow_mut() = title.to_owned();
        *self.artist.borrow_mut() = "B站直播".to_owned();
        *self.live_metrics.borrow_mut() = Some(np_live::LiveMetrics::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_play_opens_a_file_instead_of_returning_unsupported() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        let state = audio
            .play(Some(path.to_string_lossy().into_owned()))
            .expect("local play should open");
        assert!(matches!(state.status, PlaybackStatus::Playing));
        assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
    }

    #[test]
    fn tone_440_meets_the_four_playback_criteria() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        let state = audio
            .play(Some(path.to_string_lossy().into_owned()))
            .expect("no E_AUDIO");
        assert!(matches!(state.status, PlaybackStatus::Playing));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            audio.position_ms() >= 800,
            "position {}",
            audio.position_ms()
        );
        assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
    }

    #[test]
    fn queue_steps_and_volume_are_real() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        audio
            .queue_add(vec![path.to_string_lossy().into_owned()])
            .unwrap();
        audio.queue_jump(0).unwrap();
        PlayerApi::volume(&audio, 0.5).unwrap();
        assert!((audio.snapshot().unwrap().volume - 0.5).abs() < 0.001);
        let mode = PlaybackMode {
            repeat: np_core::RepeatMode::Off,
            shuffle: true,
        };
        PlayerApi::mode(&audio, mode).unwrap();
        audio.next().unwrap();
        assert!(audio.is_playing());
        let state = audio.snapshot().unwrap();
        assert!(
            (state.volume - 0.5).abs() < 0.001,
            "volume must survive track change"
        );
        assert_eq!(state.mode, mode, "mode must survive track change");
    }

    #[test]
    fn prepend_duplicate_and_next_follow_unique_queue() {
        let first =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        let second =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
        if !first.is_file() || !second.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        audio
            .play(Some(first.to_string_lossy().into_owned()))
            .unwrap();
        audio
            .play(Some(second.to_string_lossy().into_owned()))
            .unwrap();
        assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
        audio
            .play(Some(second.to_string_lossy().into_owned()))
            .unwrap();
        assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
        audio.next().unwrap();
        assert_eq!(audio.source_path().as_deref(), Some(first.as_path()));
    }
    #[test]
    fn mode_cycles_through_four_playback_modes() {
        let audio = TuiAudio::empty();
        let list = audio.cycle_mode();
        assert_eq!(list.repeat, np_core::RepeatMode::All);
        assert!(!list.shuffle);
        let one = audio.cycle_mode();
        assert_eq!(one.repeat, np_core::RepeatMode::One);
        assert!(!one.shuffle);
        let shuffle = audio.cycle_mode();
        assert!(shuffle.shuffle);
        let sequential = audio.cycle_mode();
        assert_eq!(sequential.repeat, np_core::RepeatMode::Off);
        assert!(!sequential.shuffle);
    }
    #[test]
    fn reconnect_backoff_steps_and_state() {
        assert_eq!(reconnect_delay(1).as_secs(), 1);
        assert_eq!(reconnect_delay(2).as_secs(), 2);
        assert_eq!(reconnect_delay(3).as_secs(), 5);
        assert_eq!(reconnect_delay(4).as_secs(), 10);
        assert_eq!(reconnect_delay(5).as_secs(), 20);
        assert_eq!(reconnect_delay(6).as_secs(), 30);
        assert_eq!(reconnect_delay(99).as_secs(), 30);

        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST");
        audio.begin_reconnect(Some("测试断流".to_owned()));
        assert_eq!(audio.live_state(), LiveState::Reconnecting);
        assert_eq!(audio.reconnect_attempt.get(), 1);
        assert_eq!(
            audio.live_status_label().as_deref(),
            Some("LIVE RECONNECTING 1/6")
        );
        assert!(audio.stop_live());
        assert_eq!(audio.live_state(), LiveState::Idle);
    }

    #[test]
    fn seek_moves_playback_position() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        audio
            .play(Some(path.to_string_lossy().into_owned()))
            .expect("open tone");
        let target = 1_000;
        PlayerApi::seek(&audio, target).expect("seek");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let position = audio.position_ms();
        assert!(
            position >= 900,
            "seek did not move position: target={target}, position={position}"
        );
    }
}
