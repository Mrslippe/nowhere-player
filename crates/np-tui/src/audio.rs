//! TUI 的播放器适配：本地文件真正打开，队列保存可播放路径。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};

use crate::danmaku_overlay::push_recent;
use crate::event_loop::{DanmakuLink, run_danmaku_with_retry, sleep_cancellable};
use crate::model::TrackRef;
use crate::playback::{PlaybackQueue, QueueItem, TrackKey};

mod meta;
use self::meta::{LIVE_META_REFRESH, LiveMeta};

/// 直播的 ring 窗口：比本地文件的 300 ms 更短，启动更快、更贴近 live edge。
pub(crate) const LIVE_RING_MS: u32 = 150;
/// 弹幕环形缓冲容量：只保留最近 500 条，供 Live 浮层读取。
pub(crate) const DANMAKU_BUFFER_CAPACITY: usize = 500;
pub(crate) const MAX_RECONNECT_ATTEMPTS: u32 = 6;
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
/// 后台弹幕线程回传：请求 generation + 发送结果（错误文本不含凭证）。
type DanmakuReply = (u64, Result<(), String>);

/// 一次直播的弹幕接收通道；generation 防串台，stop/换房时整体丢弃。
struct DanmakuSession {
    generation: u64,
    messages: std::sync::mpsc::Receiver<np_live::DanmakuMessage>,
    links: std::sync::mpsc::Receiver<DanmakuLink>,
}

type LiveMetaReply = (u64, LiveMeta);

pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
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
    live_meta: RefCell<Option<LiveMeta>>,
    live_meta_rx: RefCell<Option<std::sync::mpsc::Receiver<LiveMetaReply>>>,
    /// 下一次刷新房间信息（同接数）的时刻；`None` 表示当前没有直播。
    live_meta_refresh_at: Cell<Option<Instant>>,
    live_buffer_target_ms: Cell<u64>,
    live_elapsed_override: Cell<Option<u64>>,
    live_metrics: RefCell<Option<np_live::LiveMetrics>>,
    live_control: RefCell<Option<np_live::LiveControl>>,
    reconnect_attempt: Cell<u32>,
    reconnect_at: Cell<Option<Instant>>,
    login: RefCell<crate::login::LoginUi>,
    bili_auth: RefCell<Option<np_live::BiliAuth>>,
    auth_loaded: Cell<bool>,
    chat_generation: Cell<u64>,
    chat_rx: RefCell<Option<std::sync::mpsc::Receiver<DanmakuReply>>>,
    danmaku_session: RefCell<Option<DanmakuSession>>,
    danmaku_cancel: RefCell<Option<Arc<AtomicBool>>>,
    danmaku_generation: Cell<u64>,
    danmaku_link: Cell<DanmakuLink>,
    pub(crate) danmaku_recent: RefCell<VecDeque<np_live::DanmakuMessage>>,
    pub(crate) danmaku_received: Cell<u64>,
    #[cfg(test)]
    chat_test_result: RefCell<Option<Result<(), String>>>,
    #[cfg(test)]
    chat_test_calls: Cell<u32>,
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
            live_meta: RefCell::new(None),
            live_meta_rx: RefCell::new(None),
            live_meta_refresh_at: Cell::new(None),
            live_buffer_target_ms: Cell::new(crate::live_page::DEFAULT_LIVE_BUFFER_MS),
            live_elapsed_override: Cell::new(None),
            live_metrics: RefCell::new(None),
            live_control: RefCell::new(None),
            reconnect_attempt: Cell::new(0),
            reconnect_at: Cell::new(None),
            login: RefCell::new(crate::login::LoginUi::new()),
            bili_auth: RefCell::new(None),
            auth_loaded: Cell::new(false),
            chat_generation: Cell::new(0),
            chat_rx: RefCell::new(None),
            danmaku_session: RefCell::new(None),
            danmaku_cancel: RefCell::new(None),
            danmaku_generation: Cell::new(0),
            danmaku_link: Cell::new(DanmakuLink::Idle),
            danmaku_recent: RefCell::new(VecDeque::new()),
            danmaku_received: Cell::new(0),
            #[cfg(test)]
            chat_test_result: RefCell::new(None),
            #[cfg(test)]
            chat_test_calls: Cell::new(0),
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

    /// 直播状态是否在播放：Live 页控制条用 `❚❚` / `▶` 区分。
    pub(crate) fn live_state_is_playing(&self) -> bool {
        self.live_state.get() == LiveState::Playing
    }

    /// Player 上边左侧的主播名；资料未到时回退到房间号。
    pub(crate) fn live_anchor(&self) -> String {
        let anchor = self
            .live_meta
            .borrow()
            .as_ref()
            .map(|meta| meta.anchor.trim().to_owned())
            .unwrap_or_default();
        if !anchor.is_empty() {
            return anchor;
        }
        let room = self.live_room.get();
        if room == 0 {
            "直播".to_owned()
        } else {
            format!("房间 {room}")
        }
    }

    /// 信息行右侧的分区；优先二级分区，缺失时用一级分区。
    pub(crate) fn live_area(&self) -> String {
        self.live_meta
            .borrow()
            .as_ref()
            .map(|meta| meta.area.trim().to_owned())
            .unwrap_or_default()
    }

    /// Live 页状态栏的中文状态：`暂停` / `直播`。
    pub(crate) fn live_state_text(&self) -> &'static str {
        if self.live_state.get() == LiveState::Paused {
            "暂停"
        } else {
            "直播"
        }
    }

    /// 开播时长；优先房间 live_time，测试可固定。
    pub(crate) fn live_elapsed_label(&self) -> String {
        let seconds = self.live_elapsed_override.get().or_else(|| {
            let started = self
                .live_meta
                .borrow()
                .as_ref()
                .and_then(|meta| meta.live_started_at)?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some(now.saturating_sub(started))
        });
        let Some(seconds) = seconds else {
            return "--:--:--".to_owned();
        };
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let seconds = seconds % 60;
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }

    /// 直播缓冲目标（`live.buffer_ms`，设置 UI 写入）。
    pub(crate) fn live_buffer_target_ms(&self) -> u64 {
        self.live_buffer_target_ms.get()
    }

    pub(crate) fn set_live_buffer_target(&self, milliseconds: u64) {
        let milliseconds = milliseconds.clamp(
            np_core::settings::LIVE_BUFFER_MS_MIN,
            np_core::settings::LIVE_BUFFER_MS_MAX,
        );
        if self.live_buffer_target_ms.get() != milliseconds {
            self.live_buffer_target_ms.set(milliseconds);
        }
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

    /// 登录态是否具备发弹幕所需的两块凭证；不向调用方暴露 cookie 内容。
    pub(crate) fn has_bili_auth(&self) -> bool {
        self.auth_for_live().is_some_and(|auth| {
            auth.cookie("SESSDATA").is_some() && auth.cookie("bili_jct").is_some()
        })
    }

    /// 当前登录账号的 uid；未登录或凭证缺 `DedeUserID` 时为 `None`。
    ///
    /// 只读已经加载的登录态，不在绘制路径上读文件；未加载时不会误判自己的回显。
    pub(crate) fn own_bili_uid(&self) -> Option<u64> {
        self.bili_auth
            .borrow()
            .as_ref()
            .and_then(np_live::BiliAuth::uid)
    }

    /// 后台线程发送一条弹幕；返回本次请求的 generation。
    ///
    /// 不阻塞 UI，也不打印 cookie / csrf / 请求体；结果由
    /// [`poll_live_danmaku`](Self::poll_live_danmaku) 在主线程收取。
    pub(crate) fn send_live_danmaku(&self, message: &str) -> Result<u64, String> {
        if !self.is_live() {
            return Err("当前没有直播".to_owned());
        }
        let room_id = self.live_room.get();
        if room_id == 0 {
            return Err("房间号无效".to_owned());
        }
        if self.chat_rx.borrow().is_some() {
            return Err("弹幕正在发送中".to_owned());
        }
        let Some(auth) = self.auth_for_live() else {
            return Err("未登录，请先 /login bili".to_owned());
        };
        let generation = self.chat_generation.get().wrapping_add(1);
        self.chat_generation.set(generation);
        let (tx, rx) = std::sync::mpsc::channel();
        *self.chat_rx.borrow_mut() = Some(rx);
        #[cfg(test)]
        {
            self.chat_test_calls
                .set(self.chat_test_calls.get().saturating_add(1));
            if self.chat_test_result.borrow().is_some() {
                let result = self
                    .chat_test_result
                    .borrow_mut()
                    .take()
                    .expect("test danmaku result");
                let _ = tx.send((generation, result));
                return Ok(generation);
            }
        }
        let message = message.to_owned();
        let spawn = std::thread::Builder::new()
            .name("np-live-danmaku".to_owned())
            .spawn(move || {
                let result = match np_sources::ReqwestTransport::new() {
                    Ok(transport) => {
                        np_live::danmaku::send_danmaku(&transport, &auth, room_id, &message)
                            .map_err(|error| error.to_string())
                    }
                    Err(_) => Err("发送弹幕网络请求失败".to_owned()),
                };
                let _ = tx.send((generation, result));
            });
        if let Err(error) = spawn {
            *self.chat_rx.borrow_mut() = None;
            return Err(format!("弹幕发送线程启动失败：{error}"));
        }
        Ok(generation)
    }

    /// 主线程每 tick 调用：收取后台发送结果；旧 generation（换直播间 / 停播）丢弃。
    pub(crate) fn poll_live_danmaku(&self) -> Option<DanmakuReply> {
        let received = {
            let receiver = self.chat_rx.borrow();
            match receiver.as_ref() {
                Some(receiver) => match receiver.try_recv() {
                    Ok(value) => Some(value),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((
                        self.chat_generation.get(),
                        Err("弹幕发送线程意外结束".to_owned()),
                    )),
                },
                None => None,
            }
        };
        let (generation, result) = received?;
        *self.chat_rx.borrow_mut() = None;
        if generation != self.chat_generation.get() {
            return None;
        }
        Some((generation, result))
    }

    /// 当前弹幕连接状态；Live 页状态栏只读它做轻提示。
    pub(crate) fn danmaku_link(&self) -> DanmakuLink {
        self.danmaku_link.get()
    }

    /// 主线程每 tick 调用：把弹幕消息收进环形缓冲，并更新连接状态。
    ///
    /// 返回 `true` 表示缓冲区或状态栏有变化，需要重绘。单条解析失败在
    /// np-live 层已经跳过，这里只面对结构化的 [`np_live::DanmakuMessage`]。
    pub(crate) fn poll_danmaku(&self) -> bool {
        let mut changed = false;
        let mut received = Vec::new();
        let mut links = Vec::new();
        let mut messages_done = false;
        let mut links_done = false;
        {
            let session = self.danmaku_session.borrow();
            let Some(session) = session.as_ref() else {
                return false;
            };
            if session.generation != self.danmaku_generation.get() {
                return false;
            }
            loop {
                match session.messages.try_recv() {
                    Ok(message) => received.push(message),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        messages_done = true;
                        break;
                    }
                }
            }
            loop {
                match session.links.try_recv() {
                    Ok(link) => links.push(link),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        links_done = true;
                        break;
                    }
                }
            }
        }

        if !received.is_empty() {
            changed = true;
            let added = u64::try_from(received.len()).unwrap_or(u64::MAX);
            let mut recent = self.danmaku_recent.borrow_mut();
            for message in received {
                push_recent(&mut recent, message);
            }
            self.danmaku_received
                .set(self.danmaku_received.get().saturating_add(added));
            // 连接成功后不一定马上有弹幕；收到第一条就足以确认链路可用。
            if self.danmaku_link.get() != DanmakuLink::Failed
                && self.danmaku_link.get() != DanmakuLink::Connected
            {
                self.danmaku_link.set(DanmakuLink::Connected);
                changed = true;
            }
        }

        for link in links {
            if link != self.danmaku_link.get() {
                self.danmaku_link.set(link);
                changed = true;
            }
        }

        // 线程退出后两侧通道都会断开；保留缓冲和最终状态，只释放通道。
        if messages_done && links_done {
            if matches!(
                self.danmaku_link.get(),
                DanmakuLink::Connecting | DanmakuLink::Reconnecting { .. }
            ) {
                // 线程异常结束（没有发最终状态）时不要把“连接中”挂死。
                self.danmaku_link.set(DanmakuLink::Failed);
                changed = true;
            }
            *self.danmaku_session.borrow_mut() = None;
            *self.danmaku_cancel.borrow_mut() = None;
        }
        changed
    }

    /// 启动弹幕长连接：断线按 1/2/5/10/20/30s 重连，最多 6 次。
    ///
    /// 每次重连都会重新走 `getDanmuInfo`；日志与状态文本不包含 token / cookie。
    fn start_danmaku(&self, room_id: u64, auth: Option<np_live::BiliAuth>) {
        self.stop_danmaku();
        let generation = self.danmaku_generation.get().wrapping_add(1);
        self.danmaku_generation.set(generation);
        self.danmaku_recent.borrow_mut().clear();
        self.danmaku_link.set(DanmakuLink::Connecting);

        let cancel = Arc::new(AtomicBool::new(false));
        *self.danmaku_cancel.borrow_mut() = Some(Arc::clone(&cancel));
        let (message_tx, message_rx) = std::sync::mpsc::channel();
        let (link_tx, link_rx) = std::sync::mpsc::channel();
        *self.danmaku_session.borrow_mut() = Some(DanmakuSession {
            generation,
            messages: message_rx,
            links: link_rx,
        });

        let spawn = std::thread::Builder::new()
            .name("np-live-danmaku".to_owned())
            .spawn(move || {
                let transport = np_sources::ReqwestTransport::new();
                let transport = transport.as_ref();
                let _ = run_danmaku_with_retry(
                    &cancel,
                    || match transport {
                        Ok(transport) => np_live::run_danmaku_ws(
                            transport,
                            room_id,
                            auth.as_ref(),
                            &cancel,
                            &message_tx,
                        ),
                        Err(_) => Err(np_live::DanmakuWsError::Connect(
                            "网络客户端初始化失败".to_owned(),
                        )),
                    },
                    |delay| sleep_cancellable(&cancel, delay),
                    |link| {
                        let _ = link_tx.send(link);
                    },
                );
            });
        if let Err(error) = spawn {
            self.stop_danmaku();
            self.danmaku_link.set(DanmakuLink::Failed);
            *self.live_note.borrow_mut() = Some(format!("弹幕连接线程启动失败：{error}"));
        }
    }

    /// cancel 并丢弃当前弹幕会话；generation 递增，旧线程的回传全部作废。
    fn stop_danmaku(&self) {
        if let Some(cancel) = self.danmaku_cancel.borrow_mut().take() {
            cancel.store(true, Ordering::Release);
        }
        self.danmaku_generation
            .set(self.danmaku_generation.get().wrapping_add(1));
        *self.danmaku_session.borrow_mut() = None;
        self.danmaku_recent.borrow_mut().clear();
        self.danmaku_link.set(DanmakuLink::Idle);
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
        // 每次直播（含重连）前把 `live.buffer_ms` 写进 np-live 队列目标；
        // 设置里改值后对下一次直播生效（T-0093）。
        np_live::set_target_buffer_ms(self.live_buffer_target_ms.get());
        let auth = self.auth_for_live();
        *self.title.borrow_mut() = format!("直播 {room_id} 准备中…");
        *self.artist.borrow_mut() = "B站直播".to_owned();
        self.duration_ms.set(None);
        self.live_note.borrow_mut().take();
        *self.live_metrics.borrow_mut() = None;
        *self.live_meta.borrow_mut() = None;
        *self.live_meta_rx.borrow_mut() = None;

        // 声音与弹幕各自独立：弹幕线程失败只影响状态栏提示，不影响音频。
        self.start_danmaku(room_id, auth.clone());

        // 主播名 / 分区与音频准备并行：多一条短请求，不拖慢起播。
        // 同接数（`online`）短周期变化，之后每 30s 重新拉一次（T-0100b2）。
        self.spawn_live_meta(generation, room_id);
        self.live_meta_refresh_at
            .set(Some(Instant::now() + LIVE_META_REFRESH));

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
            self.stop_danmaku();
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
        self.refresh_live_meta_if_due();
        let meta_changed = self.drain_live_meta();
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
            return meta_changed;
        };
        if generation != self.live_generation.get() {
            return meta_changed;
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
                            self.stop_danmaku();
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
                        self.stop_danmaku();
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

    /// 收下后台线程拉到的主播名 / 分区；generation 过期就丢掉。
    fn drain_live_meta(&self) -> bool {
        let mut changed = false;
        loop {
            let received = {
                let receiver = self.live_meta_rx.borrow();
                receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv().ok())
            };
            let Some((generation, meta)) = received else {
                break;
            };
            if generation != self.live_generation.get() {
                continue;
            }
            if self.live_meta.borrow().as_ref() != Some(&meta) {
                *self.live_meta.borrow_mut() = Some(meta);
                changed = true;
            }
        }
        changed
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
        *self.live_meta.borrow_mut() = None;
        *self.live_meta_rx.borrow_mut() = None;
        self.live_meta_refresh_at.set(None);
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
        *self.live_meta_rx.borrow_mut() = None;
        self.live_meta_refresh_at.set(None);
        *self.live_metrics.borrow_mut() = None;
        *self.live_control.borrow_mut() = None;
        self.reconnect_at.set(None);
        // 换直播间 / 停播后，旧房间的弹幕结果不再生效。
        self.chat_generation
            .set(self.chat_generation.get().wrapping_add(1));
        *self.chat_rx.borrow_mut() = None;
        self.stop_danmaku();
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
            self.stop_danmaku();
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
        self.live_elapsed_override.set(Some(4402));
        self.duration_ms.set(None);
        *self.title.borrow_mut() = title.to_owned();
        *self.artist.borrow_mut() = "B站直播".to_owned();
        *self.live_metrics.borrow_mut() = Some(np_live::LiveMetrics::default());
    }

    /// 测试用：切换 Live 页的暂停 / 继续展示状态。
    pub(crate) fn set_live_paused_for_test(&self, paused: bool) {
        self.live_state.set(if paused {
            LiveState::Paused
        } else {
            LiveState::Playing
        });
    }

    /// 测试用：直接写入主播名 / 分区；同接数用卡面示例 `2000`。
    pub(crate) fn live_meta_for_test(&self, anchor: &str, area: &str) {
        *self.live_meta.borrow_mut() = Some(LiveMeta {
            anchor: anchor.to_owned(),
            area: area.to_owned(),
            live_started_at: None,
            online: Some(2000),
        });
    }

    /// 测试用：只改同接数，不动主播名 / 分区。
    pub(crate) fn live_online_for_test(&self, online: Option<u64>) {
        if let Some(meta) = self.live_meta.borrow_mut().as_mut() {
            meta.online = online;
        }
    }

    /// 测试用：直接设置登录态，避免读写真实 auth 文件。
    pub(crate) fn bili_auth_for_test(&self, auth: Option<np_live::BiliAuth>) {
        *self.bili_auth.borrow_mut() = auth;
        self.auth_loaded.set(true);
    }

    /// 测试用：下一条弹幕发送直接回传固定结果，不创建网络线程。
    pub(crate) fn danmaku_result_for_test(&self, result: Result<(), String>) {
        *self.chat_test_result.borrow_mut() = Some(result);
    }

    /// 测试用：到目前为止真正进入发送路径的次数。
    pub(crate) fn danmaku_calls_for_test(&self) -> u32 {
        self.chat_test_calls.get()
    }

    /// 测试用：直接设置弹幕连接状态，不建立真实连接。
    pub(crate) fn danmaku_link_for_test(&self, link: DanmakuLink) {
        self.danmaku_link.set(link);
    }

    /// 测试用：建立一条假弹幕通道，模拟后台线程的消息与状态回传。
    pub(crate) fn danmaku_channel_for_test(
        &self,
    ) -> (
        std::sync::mpsc::Sender<np_live::DanmakuMessage>,
        std::sync::mpsc::Sender<DanmakuLink>,
    ) {
        self.stop_danmaku();
        let generation = self.danmaku_generation.get().wrapping_add(1);
        self.danmaku_generation.set(generation);
        let (message_tx, message_rx) = std::sync::mpsc::channel();
        let (link_tx, link_rx) = std::sync::mpsc::channel();
        *self.danmaku_session.borrow_mut() = Some(DanmakuSession {
            generation,
            messages: message_rx,
            links: link_rx,
        });
        self.danmaku_link.set(DanmakuLink::Connecting);
        (message_tx, link_tx)
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
    fn live_state_text_is_chinese_and_flips_on_pause() {
        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST");
        assert_eq!(audio.live_state_text(), "直播");
        audio.live_state.set(LiveState::Paused);
        assert_eq!(audio.live_state_text(), "暂停");
    }

    #[test]
    fn live_anchor_prefers_meta_and_falls_back_to_the_room() {
        let audio = TuiAudio::empty();
        assert_eq!(audio.live_anchor(), "直播");
        assert_eq!(audio.live_area(), "");

        audio.live_for_test(21622811, "BLAST");
        assert_eq!(audio.live_anchor(), "房间 21622811");
        assert_eq!(audio.live_area(), "");

        audio.live_meta_for_test("测试主播", "游戏赛事");
        assert_eq!(audio.live_anchor(), "测试主播");
        assert_eq!(audio.live_area(), "游戏赛事");

        assert!(audio.stop_live());
        assert_eq!(audio.live_anchor(), "直播");
        assert_eq!(audio.live_area(), "");
    }

    #[test]
    fn live_buffer_target_clamps_to_a_sane_range() {
        let audio = TuiAudio::empty();
        assert_eq!(audio.live_buffer_target_ms(), 900);
        audio.set_live_buffer_target(1_200);
        assert_eq!(audio.live_buffer_target_ms(), 1_200);
        audio.set_live_buffer_target(0);
        assert_eq!(
            audio.live_buffer_target_ms(),
            np_core::settings::LIVE_BUFFER_MS_MIN
        );
        audio.set_live_buffer_target(u64::MAX);
        assert_eq!(
            audio.live_buffer_target_ms(),
            np_core::settings::LIVE_BUFFER_MS_MAX
        );
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
