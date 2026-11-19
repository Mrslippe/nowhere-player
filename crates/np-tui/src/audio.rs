//! TUI 的播放器适配：本地文件真正打开，队列保存可播放路径。
//!
//! 按职责拆分：直播状态与生命周期在 live，弹幕在 danmaku，队列与 PlayerApi 在
//! playback，扫码登录与凭证在 auth，资料与同接在 meta，直播画面在 video。
//! 本模块只保留 TuiAudio 的字段、构造与必要的门面。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use np_audio::{Player, SampleTapHandle};

use crate::event_loop::DanmakuLink;
use crate::playback::PlaybackQueue;

mod auth;
mod danmaku;
mod live;
mod meta;
mod playback;
// `src/video.rs` 挂在 audio 下：任务卡只授权新增该文件，不授权改 main.rs 的
// 模块声明，所以用 `#[path]` 明确指向顶层文件。
#[cfg(test)]
mod tests;
#[path = "video.rs"]
mod video;

use self::live::{DanmakuSession, LivePreparation};
pub(crate) use self::live::{LiveState, MAX_RECONNECT_ATTEMPTS, reconnect_delay};
use self::meta::LiveMeta;
use self::video::VideoController;

/// 弹幕环形缓冲容量：只保留最近 500 条，供 Live 浮层读取。
pub(crate) const DANMAKU_BUFFER_CAPACITY: usize = 500;

/// 后台弹幕线程回传：请求 generation + 发送结果（错误文本不含凭证）。
type DanmakuReply = (u64, Result<(), String>);

type LiveMetaReply = (u64, LiveMeta);
/// 同接刷新回传：generation + 本次同接数（失败为 `None`，显示 ` -- `）。
type LiveOnlineReply = (u64, Option<u64>);

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
    live_meta_generation: Cell<u64>,
    live_meta_rx: RefCell<Option<std::sync::mpsc::Receiver<LiveMetaReply>>>,
    /// 下一次刷新房间资料（主播 / 分区）的时刻；`None` 表示当前没有直播。
    live_meta_refresh_at: Cell<Option<Instant>>,
    live_online: Cell<Option<u64>>,
    live_online_generation: Cell<u64>,
    live_online_rx: RefCell<Option<std::sync::mpsc::Receiver<LiveOnlineReply>>>,
    /// 下一次刷新同接人数的时刻；与资料计时相互独立。
    live_online_refresh_at: Cell<Option<Instant>>,
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
    video: VideoController,
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
            live_meta_generation: Cell::new(0),
            live_meta_rx: RefCell::new(None),
            live_meta_refresh_at: Cell::new(None),
            live_online: Cell::new(None),
            live_online_generation: Cell::new(0),
            live_online_rx: RefCell::new(None),
            live_online_refresh_at: Cell::new(None),
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
            video: VideoController::new(),
        }
    }
}
