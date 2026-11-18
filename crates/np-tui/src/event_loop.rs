use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::{App, parse_search_input};
use crate::audio::TuiAudio;
use crate::keymap::Action;
use crate::model::*;
use crate::player_bar::update_spectrum;
use crate::render::*;
use np_app::LibraryHandle;
use np_app::PlayerApi;
use np_app::{SettingsHandle, ThemeApi};
use np_core::Block;
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
        if !player.is_live() {
            if app.live_chat_active() {
                app.cancel_live_chat();
                draw_needed = true;
            }
            app.clear_chat_notice();
        }
        if let Some((generation, result)) = player.poll_live_danmaku()
            && app.apply_live_danmaku_result(generation, result)
        {
            draw_needed = true;
        }
        if player.poll_login() {
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
                        crate::live_page::handle_chat_key(key.code, app, player);
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
                    if app.settings_edit.is_some() {
                        crate::settings_page::handle_settings_edit_key(
                            key.code,
                            app,
                            player,
                            theme_ctx.api,
                            theme_ctx.settings,
                        );
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
                            if app.quit_armed {
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
