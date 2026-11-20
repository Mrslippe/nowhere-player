//! 直播状态、生命周期、重连、缓冲与状态文案。
//!
//! 所有方法都写 `impl TuiAudio`；子模块可直接访问父模块的私有字段。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use np_audio::Player;

use crate::event_loop::DanmakuLink;

use super::TuiAudio;

/// 直播的 ring 窗口：比本地文件的 300 ms 更短，启动更快、更贴近 live edge。
pub(crate) const LIVE_RING_MS: u32 = 150;
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
pub(super) struct LiveFailure {
    message: String,
    retryable: bool,
}

pub(super) type LivePreparation = (u64, Result<np_live::PreparedLive, LiveFailure>);

/// 一次直播的弹幕接收通道；generation 防串台，stop/换房时整体丢弃。
pub(super) struct DanmakuSession {
    pub(super) generation: u64,
    pub(super) messages: std::sync::mpsc::Receiver<np_live::DanmakuMessage>,
    pub(super) links: std::sync::mpsc::Receiver<DanmakuLink>,
}

pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
    let index = usize::try_from(attempt.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(RECONNECT_DELAYS_SECS.len() - 1);
    Duration::from_secs(RECONNECT_DELAYS_SECS[index])
}

impl TuiAudio {
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
    /// `live.buffer_ms` 是运行期高水位；启动只等 `min(target,850ms)` 安全垫（T-0115）。
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
    /// 例：`LIVE 1000ms U:0 Q:0 R:0 D:0 E:0 F:0 S:0`；暂停时 `LIVE PAUSED`；
    /// 自建视频宿主打开时 `VIDEO`（声音由 mpv 出）。
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
        if self.video_uses_mpv_audio() {
            return Some(
                if self.live_state.get() == LiveState::Paused {
                    "VIDEO PAUSED"
                } else {
                    "VIDEO"
                }
                .to_owned(),
            );
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

    /// 开始一路直播：停止当前播放、保留队列，后台准备网络与首帧。
    ///
    /// 本方法不碰网络；准备线程完成后由 [`poll_live`](Self::poll_live) 在主线程
    /// 打开 `Player`。
    pub(crate) fn start_live(&self, room_id: u64) {
        self.start_live_inner(room_id, true);
    }

    pub(super) fn start_live_inner(&self, room_id: u64, reset_reconnect: bool) {
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
        log::info!(
            target: "live",
            "event=start room={room_id} target_ms={}",
            self.live_buffer_target_ms.get()
        );
        let auth = self.auth_for_live();
        *self.title.borrow_mut() = format!("直播 {room_id} 准备中…");
        *self.artist.borrow_mut() = "B站直播".to_owned();
        self.duration_ms.set(None);
        self.live_note.borrow_mut().take();
        *self.live_metrics.borrow_mut() = None;
        *self.live_meta.borrow_mut() = None;

        // 声音与弹幕各自独立：弹幕线程失败只影响状态栏提示，不影响音频。
        self.start_danmaku(room_id, auth.clone());

        // 资料 30s、同接 3s 各自独立计时；起播时两路立即各拉一次（T-0102）。
        self.begin_live_refresh(room_id);

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
            self.reset_live_refresh();
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
        self.refresh_live_online_if_due();
        // 两路都 drain，不短路：一个失败不能饿死另一个。
        let refresh_changed = self.drain_live_meta() | self.drain_live_online();
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
            return refresh_changed;
        };
        if generation != self.live_generation.get() {
            return refresh_changed;
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
                        if self.video_uses_mpv_audio() {
                            player.set_output_muted(true);
                        }
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
                        log::info!(
                            target: "live",
                            "event=playing room={} title={} reconnected={}",
                            prepared.room_id,
                            title,
                            reconnected
                        );
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
                log::warn!(
                    target: "live",
                    "event=prepare_failed retryable={} error={}",
                    failure.retryable,
                    failure.message
                );
                if failure.retryable && self.live_state.get() != LiveState::Idle {
                    self.begin_reconnect(Some(failure.message));
                } else {
                    self.fail_live(format!("直播失败：{}", failure.message));
                }
            }
        }
        true
    }

    pub(super) fn begin_reconnect(&self, reason: Option<String>) {
        if !self.is_live() {
            return;
        }
        let room = self.live_room.get();
        if room == 0 {
            self.fail_live("直播重连失败：房间号丢失".to_owned());
            return;
        }
        let attempt = self.reconnect_attempt.get().saturating_add(1);
        log::warn!(
            target: "live",
            "event=reconnect attempt={} reason={}",
            attempt,
            reason.as_deref().unwrap_or("连接中断")
        );
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

    pub(super) fn fail_live(&self, note: String) {
        log::error!(target: "live", "event=failed note={note}");
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
        log::info!(target: "live", "event=stop room={}", self.live_room.get());
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
        self.reconnect_attempt.set(0);
        self.reconnect_at.set(None);
        true
    }

    /// 取消准备、丢弃结果，但不重置标题等展示字段。
    pub(super) fn shutdown_live(&self) {
        // 停播、换房、F5 和播本地文件都会经过这里；视频窗口不能留在桌面上。
        self.close_video();
        if let Some(cancel) = self.live_cancel.borrow_mut().take() {
            cancel.store(true, Ordering::Release);
        }
        *self.live_rx.borrow_mut() = None;
        self.reset_live_refresh();
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

    /// 暂停直播：保留 Player/解码器，网络继续读；缓存超过 30s 丢新帧。
    pub(super) fn pause_live(&self) {
        if self.live_state.get() != LiveState::Playing {
            return;
        }
        log::info!(
            target: "live",
            "event=pause video={} room={}",
            self.video_is_active(),
            self.live_room.get()
        );
        if self.video_uses_mpv_audio() {
            // 视频模式声音由 mpv 出；TUI 音频继续静音运行，保持缓冲新鲜。
            self.video.set_paused(true);
            self.playing.set(false);
            self.duration_ms.set(None);
            self.live_state.set(LiveState::Paused);
            *self.live_note.borrow_mut() = Some("视频已暂停".to_owned());
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
    pub(super) fn resume_live(&self) {
        if self.live_state.get() != LiveState::Paused {
            return;
        }
        log::info!(
            target: "live",
            "event=resume video={} room={}",
            self.video_is_active(),
            self.live_room.get()
        );
        if self.video_uses_mpv_audio() {
            self.video.set_paused(false);
            self.playing.set(true);
            self.live_state.set(LiveState::Playing);
            *self.live_note.borrow_mut() = Some("视频已继续".to_owned());
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

    /// `r` / F5：本地丢弃旧队列，从最新缓冲前 300ms 起播，不重连网络。
    pub(crate) fn refresh_live(&self) -> bool {
        if !self.is_live() {
            return false;
        }
        let room = self.live_room.get();
        if room == 0 {
            return false;
        }
        // 视频模式声音在 mpv；先关掉视频并恢复 TUI 音频，再本地刷新。
        if self.video_is_active() {
            let _ = self.toggle_video();
        }
        let player = self.player.borrow();
        let control = self.live_control.borrow();
        let (Some(player), Some(control)) = (player.as_ref(), control.as_ref()) else {
            drop(player);
            drop(control);
            // 准备中 / 重连中没有可裁的队列，退回重新接流。
            self.start_live(room);
            return true;
        };
        if self.live_state.get() == LiveState::Paused {
            if let Err(error) = player.play() {
                *self.live_note.borrow_mut() = Some(format!("直播刷新失败：{error}"));
                return false;
            }
            self.playing.set(true);
            self.live_state.set(LiveState::Playing);
        }
        // resume 清暂停；request_refresh 让解码线程裁到最新 300ms。
        control.resume();
        control.request_refresh();
        *self.live_note.borrow_mut() = Some("直播已跳到最新缓冲".to_owned());
        log::info!(target: "live", "event=refresh_requested room={room}");
        true
    }

    /// 测试用：直接走 F5 / 换房共用的清理路径，不启动网络线程。
    #[cfg(test)]
    pub(crate) fn shutdown_live_for_test(&self) {
        self.shutdown_live();
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
