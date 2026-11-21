use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::{App, parse_search_input};
use crate::audio::{MAX_RECONNECT_ATTEMPTS, TuiAudio, reconnect_delay};
use crate::keymap::Action;
use crate::model::*;
use crate::player_bar::update_spectrum;
use crate::render::*;
use np_app::LibraryHandle;
use np_app::PlayerApi;
use np_app::{SettingsHandle, ThemeApi};
use np_core::Block;

/// 弹幕长连接状态；只用于 Live 页状态栏轻提示，不参与布局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DanmakuLink {
    /// 没有直播，或弹幕连接尚未启动。
    Idle,
    /// 后台线程正在取 token / 连接 / 认证。
    Connecting,
    /// 至少收到一条 `DANMU_MSG`。
    Connected,
    /// 断线后退避等待重连；`attempt` 从 1 数到 6。
    Reconnecting { attempt: u32 },
    /// 6 次重连都失败，或线程无法启动。
    Failed,
}

impl DanmakuLink {
    /// 状态栏右侧的轻提示；`Idle` 不占位，保证现有 golden 不变。
    pub(crate) fn hint(self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Connecting => Some("弹幕连接中".to_owned()),
            Self::Connected => Some("弹幕已连接".to_owned()),
            Self::Reconnecting { attempt } => {
                Some(format!("弹幕重连 {attempt}/{MAX_RECONNECT_ATTEMPTS}"))
            }
            Self::Failed => Some("弹幕未连接".to_owned()),
        }
    }
}

/// 后台弹幕线程的结束方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DanmakuRunEnd {
    Cancelled,
    SenderClosed,
    Exhausted,
}

/// 断线重连驱动：成功、取消或 6 次重连失败后返回。
///
/// 由 `np-live-danmaku` 线程调用；单测用假 runner / 假 sleeper 验证退避与上限。
pub(crate) fn run_danmaku_with_retry<F, S, R>(
    cancel: &AtomicBool,
    mut attempt_once: F,
    mut wait: S,
    mut report: R,
) -> DanmakuRunEnd
where
    F: FnMut() -> Result<(), np_live::DanmakuWsError>,
    S: FnMut(Duration) -> bool,
    R: FnMut(DanmakuLink),
{
    let mut attempt = 0u32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return DanmakuRunEnd::Cancelled;
        }
        match attempt_once() {
            // 接收端被丢弃（stop / 换房）：正常收尾，不重连。
            Ok(()) => return DanmakuRunEnd::SenderClosed,
            Err(np_live::DanmakuWsError::Cancelled) => return DanmakuRunEnd::Cancelled,
            Err(_) => {
                if cancel.load(Ordering::Relaxed) {
                    return DanmakuRunEnd::Cancelled;
                }
                attempt = attempt.saturating_add(1);
                if attempt > MAX_RECONNECT_ATTEMPTS {
                    report(DanmakuLink::Failed);
                    return DanmakuRunEnd::Exhausted;
                }
                let delay = reconnect_delay(attempt);
                report(DanmakuLink::Reconnecting { attempt });
                if !wait(delay) {
                    return DanmakuRunEnd::Cancelled;
                }
            }
        }
    }
}

/// 以 100ms 粒度睡眠，期间检查取消；返回 `false` 表示被取消。
pub(crate) fn sleep_cancellable(cancel: &AtomicBool, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        let slice = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(100));
        std::thread::sleep(slice);
    }
}

pub(crate) struct ThemeContext<'a> {
    pub(crate) api: &'a dyn ThemeApi,
    pub(crate) settings: &'a SettingsHandle,
}

pub(crate) struct SearchContext<'a> {
    pub(crate) tx: &'a std::sync::mpsc::Sender<(u64, Block)>,
    pub(crate) rx: &'a std::sync::mpsc::Receiver<(u64, Block)>,
    pub(crate) play_rx: &'a std::sync::mpsc::Receiver<String>,
}

pub(crate) fn draw_synced<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    player: &TuiAudio,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    execute!(io::stdout(), BeginSynchronizedUpdate)?;
    let result = terminal.draw(|frame| draw(frame, app, player));
    let end = execute!(io::stdout(), EndSynchronizedUpdate);
    result?;
    end?;
    Ok(())
}

pub(crate) fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    player: &mut TuiAudio,
    library: &Arc<LibraryHandle>,
    search: SearchContext<'_>,
    theme_ctx: ThemeContext<'_>,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    let mut draw_needed = true;
    loop {
        let now = std::time::Instant::now();
        // 终端尺寸用于 d/c 的可用性判断（高度 <10 时禁用弹幕栏）。
        if let Ok(size) = terminal.size()
            && app.set_viewport_height(size.height)
        {
            draw_needed = true;
        }
        // 成功 / 取消类状态 3 秒后回默认；错误与进行中状态没有到期时间。
        let default_status = if player.is_live() {
            player.live_state_text()
        } else {
            "就绪"
        };
        if app.expire_status(now, default_status) {
            draw_needed = true;
        }
        if app.expire_chat_notice(now) {
            draw_needed = true;
        }
        // `live.buffer_ms` 由设置页 / config set 修改，主循环每 tick 同步一次。
        let target = crate::settings_page::live_buffer_ms(theme_ctx.settings);
        if target != player.live_buffer_target_ms() {
            player.set_live_buffer_target(target);
            draw_needed = true;
        }
        let ctrl_down = crate::ctrl_hint::control_down();
        if ctrl_down != app.show_key_hints {
            app.show_key_hints = ctrl_down;
            draw_needed = true;
        }
        if app
            .transport_feedback
            .is_some_and(|(_, at)| at.elapsed() >= std::time::Duration::from_millis(180))
        {
            app.transport_feedback = None;
            draw_needed = true;
        }
        while let Ok((search_id, block)) = search.rx.try_recv() {
            if search_id == app.search_id {
                app.upsert_block(block);
                app.refresh_search_refs(&**library);
                if let Some(index) = app
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::Search)
                {
                    app.view_index = index;
                    app.focus = Focus::List;
                }
                app.set_transient_status("搜索完成");
                app.search_pending = false;
                app.search_attempted = true;
                draw_needed = true;
            }
        }
        while let Ok(path) = search.play_rx.try_recv() {
            match player.play(Some(path)) {
                Ok(_) => {
                    app.set_transient_status("播放中");
                    app.flash_transport(Action::TogglePlay);
                }
                Err(error) => app.set_status(error.message.clone()),
            }
            app.refresh_queue_view(player, &**library);
            draw_needed = true;
        }
        if player.poll_live() {
            draw_needed = true;
        }
        // 主播名回填：Browse 直播页开播后，meta 一到就写进历史。
        if crate::live_entry::poll_anchor(app, player) {
            draw_needed = true;
        }
        // 外部播放器窗口：接住取流结果，回收用户/播放器自己关掉的进程。
        if player.poll_video() {
            draw_needed = true;
        }
        // 视频窗口主题跟随当前 TUI Palette；变化时只发 `theme ...`，不重启宿主。
        player.sync_video_theme(&app.palette);
        // 弹幕收包与状态提示独立于音频；每 tick 只做非阻塞 try_recv。
        if player.poll_danmaku() {
            draw_needed = true;
        }
        // 浮层未读/跟随跟随收包计数；本地回显也走同一个计数。
        if app.sync_danmaku(player.danmaku_received()) {
            draw_needed = true;
        }
        if !player.is_live() {
            app.clear_live_exit();
            if app.live_chat_active() {
                app.cancel_live_chat();
                draw_needed = true;
            }
            app.clear_chat_notice();
            app.reset_danmaku_overlay();
        }
        if let Some((generation, result)) = player.poll_live_danmaku()
            && app.apply_live_danmaku_result(generation, result)
        {
            draw_needed = true;
        }
        if player.poll_login() {
            draw_needed = true;
        }
        if let Some(result) = crate::mpv_install::poll() {
            match result {
                Ok(_) => app.set_status("mpv 安装完成，按 v 打开视频".to_owned()),
                Err(error) => app.set_status(format!("mpv 安装失败：{error}")),
            }
            draw_needed = true;
        }
        match player.handle_ended() {
            Ok(true) => {
                app.refresh_queue_view(player, &**library);
                draw_needed = true;
            }
            Ok(false) => {}
            Err(error) => {
                app.set_status(error.message.clone());
                draw_needed = true;
            }
        }
        if let Some(note) = player.take_live_note() {
            app.set_status(note);
            draw_needed = true;
        }
        #[cfg(windows)]
        if app.poll_tray(player) {
            player.close_video();
            return Ok(());
        }
        if app.poll_playlist_prepare(player, &**library) {
            draw_needed = true;
        }
        if app.refresh_completion(player) {
            draw_needed = true;
        }
        if app.search_pending {
            draw_needed = true;
        }
        if draw_needed {
            update_spectrum(app, player);
            draw_synced(terminal, app, player)?;
            draw_needed = false;
        }
        let timeout = if app.search_pending || !app.input.is_empty() {
            std::time::Duration::from_millis(50)
        } else if player.is_playing() {
            // 12fps: Braille + truecolor 下 CPU 超过 2.8% 时按卡降帧。
            std::time::Duration::from_millis(83)
        } else {
            // 空闲时也定期检查 Ctrl，保证按住后键位提示能及时出现。
            std::time::Duration::from_millis(100)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if player.login_active() {
                        player.handle_login_key(key.code);
                        draw_needed = true;
                        continue;
                    }
                    // Live 弹幕输入框优先于 live / 全局键：`c` 在这里只是普通字符。
                    if app.live_chat_active() {
                        crate::chat_input::handle_chat_key(key.code, app, player);
                        draw_needed = true;
                        continue;
                    }
                    if app.search_editing_input() {
                        match key.code {
                            KeyCode::Char(character) => app.insert_search_char(character),
                            KeyCode::Backspace => app.backspace_search_char(),
                            KeyCode::Left => app.move_search_cursor(false),
                            KeyCode::Right => app.move_search_cursor(true),
                            KeyCode::Tab => {
                                crate::controller::cycle_search_platform(app, library, search.tx);
                            }
                            KeyCode::Enter => match parse_search_input(&app.search_input) {
                                Ok(draft) => {
                                    app.end_search_edit();
                                    app.start_search_draft(draft, library, search.tx);
                                }
                                Err(error) => app.set_status(error),
                            },
                            KeyCode::Esc => app.end_search_edit(),
                            _ => {}
                        }
                        draw_needed = true;
                        continue;
                    }
                    // 设置编辑态优先于全局键：←/→ 只改 draft，不会切焦点。
                    if app.settings_editor.is_editing() {
                        crate::settings_editor::handle_settings_edit_key(
                            key.code,
                            app,
                            player,
                            theme_ctx.api,
                            theme_ctx.settings,
                        );
                        draw_needed = true;
                        continue;
                    }
                    // 弹幕浮层可见时 ↑/↓/f 优先给弹幕；输入框、设置编辑态
                    // 和命令输入已在前面的分支拦截（命令模式 / 歌单命名在
                    // keys_owned 里显式让出），状态机不变。
                    if player.is_live()
                        && crate::danmaku_overlay::keys_owned(app)
                        && crate::danmaku_overlay::handle_key(app, key.code)
                    {
                        draw_needed = true;
                        continue;
                    }
                    if app.focus == Focus::Controller {
                        match key.code {
                            KeyCode::Esc => app.leave_command(),
                            KeyCode::Enter => {
                                let input = app.input_text();
                                app.leave_command();
                                app.run(
                                    input,
                                    player,
                                    library,
                                    search.tx,
                                    theme_ctx.api,
                                    theme_ctx.settings,
                                );
                            }
                            KeyCode::Tab => {
                                app.accept_completion();
                            }
                            KeyCode::Up if app.completion.is_some() => {
                                app.move_completion(false);
                            }
                            KeyCode::Down if app.completion.is_some() => {
                                app.move_completion(true);
                            }
                            KeyCode::Up => app.history_up(),
                            KeyCode::Down => app.history_down(),
                            KeyCode::Char(character) => app.insert(character),
                            KeyCode::Backspace => app.backspace(),
                            KeyCode::Left => app.left(),
                            KeyCode::Right => app.right(),
                            _ => {}
                        }
                    } else if app.playlist_input_active() {
                        match key.code {
                            KeyCode::Char(character) => app.insert_playlist_char(character),
                            KeyCode::Backspace => app.backspace_playlist_char(),
                            KeyCode::Left => app.move_playlist_cursor(false),
                            KeyCode::Right => app.move_playlist_cursor(true),
                            KeyCode::Enter => {
                                if let Err(error) = app.confirm_playlist_create(&**library) {
                                    app.set_status(error);
                                }
                            }
                            KeyCode::Esc => app.cancel_playlist_create(),
                            _ => {}
                        }
                        draw_needed = true;
                        continue;
                    } else {
                        // Browse 直播页的输入态（List 再 Enter）：数字 / 历史 /
                        // Enter / Delete / Esc 由 live_entry 独占，避免 `q` / `s` /
                        // `f` 误触全局键。未激活时数字等按键走全局键位。
                        if !player.is_live()
                            && app.live_entry.input_active
                            && app.focus == Focus::List
                            && matches!(app.view_kind.get(app.view_index), Some(BrowseKind::Live))
                            && crate::live_entry::handle_key(key.code, key.modifiers, app, player)
                        {
                            draw_needed = true;
                            continue;
                        }
                        app.clear_chat_notice();
                        if key.code == KeyCode::Enter {
                            if app.pending_add.is_some() {
                                if let Err(error) = app.confirm_add_to_playlist(&**library) {
                                    app.set_status(error);
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.pending_remove.is_some() {
                                if let Err(error) = app.confirm_remove(player, &**library) {
                                    app.set_status(error);
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.pending_playlist_delete.is_some() {
                                if let Err(error) = app.confirm_playlist_delete(&**library) {
                                    app.set_status(error);
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.live_exit_armed() {
                                app.confirm_live_exit(player);
                                draw_needed = true;
                                continue;
                            }
                            if app.quit_armed {
                                player.close_video();
                                return Ok(());
                            }
                        }
                        let action = {
                            let context = crate::keymap::InputContext {
                                focus: app.focus,
                                kind: app.view_kind.get(app.view_index).cloned(),
                                live: player.is_live(),
                            };
                            crate::keymap::resolve(&context, key.code, key.modifiers)
                        };
                        if let Some(action) = action {
                            crate::controller::execute(
                                action,
                                app,
                                player,
                                library,
                                search.tx,
                                theme_ctx.api,
                                theme_ctx.settings,
                            );
                        }
                    }
                    draw_needed = true;
                }
                Event::Paste(text) => {
                    if app.live_chat_active() {
                        for character in text.chars() {
                            app.insert_live_chat_char(character);
                        }
                    } else {
                        for character in text.chars() {
                            app.insert(character);
                        }
                    }
                    draw_needed = true;
                }
                Event::Resize(_, _) => draw_needed = true,
                _ => {}
            }
        } else if player.is_playing() {
            draw_needed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::DANMAKU_BUFFER_CAPACITY;

    fn danmaku_message(index: usize) -> np_live::DanmakuMessage {
        np_live::DanmakuMessage {
            user: format!("用户{index}"),
            uid: None,
            text: format!("弹幕-{index}"),
            color: 16_777_215,
            timestamp: index as u64,
        }
    }

    #[test]
    fn danmaku_ring_keeps_only_the_latest_500() {
        let audio = TuiAudio::empty();
        let (messages, _links) = audio.danmaku_channel_for_test();
        for index in 0..DANMAKU_BUFFER_CAPACITY + 1 {
            messages
                .send(danmaku_message(index))
                .expect("fake channel should stay open");
        }
        assert!(audio.poll_danmaku());
        let recent = audio.danmaku_messages();
        assert_eq!(recent.len(), DANMAKU_BUFFER_CAPACITY);
        assert_eq!(recent.first().expect("oldest").text, "弹幕-1");
        assert_eq!(recent.last().expect("newest").text, "弹幕-500");
    }

    #[test]
    fn poll_danmaku_sets_connected_on_first_message() {
        let audio = TuiAudio::empty();
        let (messages, _links) = audio.danmaku_channel_for_test();
        assert_eq!(audio.danmaku_link(), DanmakuLink::Connecting);
        messages.send(danmaku_message(0)).expect("send");
        assert!(audio.poll_danmaku());
        assert_eq!(audio.danmaku_link(), DanmakuLink::Connected);
        assert_eq!(audio.danmaku_messages().len(), 1);
        // 没有新消息时不必重绘。
        assert!(!audio.poll_danmaku());
    }

    #[test]
    fn poll_danmaku_applies_reconnect_and_failure_status() {
        let audio = TuiAudio::empty();
        let (_messages, links) = audio.danmaku_channel_for_test();
        links
            .send(DanmakuLink::Reconnecting { attempt: 2 })
            .expect("send");
        assert!(audio.poll_danmaku());
        assert_eq!(
            audio.danmaku_link(),
            DanmakuLink::Reconnecting { attempt: 2 }
        );
        links.send(DanmakuLink::Failed).expect("send");
        assert!(audio.poll_danmaku());
        assert_eq!(audio.danmaku_link(), DanmakuLink::Failed);
        assert_eq!(DanmakuLink::Failed.hint().as_deref(), Some("弹幕未连接"));
    }

    #[test]
    fn stop_live_cancels_and_ignores_late_danmaku() {
        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST");
        let (messages, links) = audio.danmaku_channel_for_test();
        messages.send(danmaku_message(0)).expect("send");
        assert!(audio.poll_danmaku());
        assert_eq!(audio.danmaku_messages().len(), 1);

        assert!(audio.stop_live());
        assert_eq!(audio.danmaku_link(), DanmakuLink::Idle);
        assert!(audio.danmaku_messages().is_empty());
        // 旧线程即使还在跑，也已拿不到接收端；消息不会串到新会话。
        assert!(messages.send(danmaku_message(1)).is_err());
        assert!(links.send(DanmakuLink::Connected).is_err());
        assert!(!audio.poll_danmaku());
        assert!(audio.danmaku_messages().is_empty());
    }

    #[test]
    fn retry_loop_steps_through_the_configured_backoff() {
        let cancel = AtomicBool::new(false);
        let mut waits = Vec::new();
        let mut statuses = Vec::new();
        let end = run_danmaku_with_retry(
            &cancel,
            || Err(np_live::DanmakuWsError::Timeout("测试断线")),
            |delay| {
                waits.push(delay.as_secs());
                true
            },
            |link| statuses.push(link),
        );
        assert_eq!(end, DanmakuRunEnd::Exhausted);
        assert_eq!(waits, vec![1, 2, 5, 10, 20, 30]);
        assert_eq!(
            statuses,
            vec![
                DanmakuLink::Reconnecting { attempt: 1 },
                DanmakuLink::Reconnecting { attempt: 2 },
                DanmakuLink::Reconnecting { attempt: 3 },
                DanmakuLink::Reconnecting { attempt: 4 },
                DanmakuLink::Reconnecting { attempt: 5 },
                DanmakuLink::Reconnecting { attempt: 6 },
                DanmakuLink::Failed,
            ]
        );
    }

    #[test]
    fn retry_loop_stops_after_a_successful_connection() {
        let cancel = AtomicBool::new(false);
        let mut calls = 0u32;
        let mut waits = Vec::new();
        let mut statuses = Vec::new();
        let end = run_danmaku_with_retry(
            &cancel,
            || {
                calls += 1;
                if calls <= 2 {
                    Err(np_live::DanmakuWsError::Closed)
                } else {
                    Ok(())
                }
            },
            |delay| {
                waits.push(delay.as_secs());
                true
            },
            |link| statuses.push(link),
        );
        assert_eq!(end, DanmakuRunEnd::SenderClosed);
        assert_eq!(calls, 3);
        assert_eq!(waits, vec![1, 2]);
        assert_eq!(statuses.len(), 2);
        assert!(!statuses.contains(&DanmakuLink::Failed));
    }

    #[test]
    fn retry_loop_is_cancelled_by_stop() {
        let cancel = AtomicBool::new(false);
        let mut waits = 0u32;
        let mut statuses = Vec::new();
        let end = run_danmaku_with_retry(
            &cancel,
            || Err(np_live::DanmakuWsError::Closed),
            |_| {
                waits += 1;
                false
            },
            |link| statuses.push(link),
        );
        assert_eq!(end, DanmakuRunEnd::Cancelled);
        assert_eq!(waits, 1);
        assert_eq!(statuses, vec![DanmakuLink::Reconnecting { attempt: 1 }]);

        let cancel = AtomicBool::new(true);
        let end = run_danmaku_with_retry(
            &cancel,
            || panic!("cancel must be checked before connecting"),
            |_| panic!("must not wait"),
            |_| panic!("must not report"),
        );
        assert_eq!(end, DanmakuRunEnd::Cancelled);

        let cancel = AtomicBool::new(false);
        let end = run_danmaku_with_retry(
            &cancel,
            || Err(np_live::DanmakuWsError::Cancelled),
            |_| panic!("must not wait after Cancelled"),
            |_| panic!("must not report after Cancelled"),
        );
        assert_eq!(end, DanmakuRunEnd::Cancelled);
    }

    #[test]
    fn cancellable_sleep_wakes_on_cancel() {
        let cancel = AtomicBool::new(true);
        assert!(!sleep_cancellable(&cancel, Duration::from_secs(1)));

        let cancel = AtomicBool::new(false);
        assert!(sleep_cancellable(&cancel, Duration::from_millis(5)));

        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let handle = std::thread::spawn(move || sleep_cancellable(&flag, Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(20));
        cancel.store(true, Ordering::Release);
        assert!(!handle.join().expect("sleeper thread"));
    }
}
