use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::App;
use crate::app::capture_table;
use crate::audio::TuiAudio;
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
        while let Ok((search_id, block)) = search.rx.try_recv() {
            if search_id == app.search_id {
                app.upsert_block(block);
                app.refresh_search_refs(&**library);
                app.status = "搜索完成".to_owned();
                app.search_pending = false;
                draw_needed = true;
            }
        }
        while let Ok(path) = search.play_rx.try_recv() {
            match player.play(Some(path)) {
                Ok(_) => app.status = "播放中".to_owned(),
                Err(error) => app.status = error.message.clone(),
            }
            draw_needed = true;
        }
        #[cfg(windows)]
        if app.poll_tray(player) {
            return Ok(());
        }
        if app.refresh_completion(player) {
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
            std::time::Duration::from_millis(500)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
                    {
                        app.panel_visible = !app.panel_visible;
                        draw_needed = true;
                    } else if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F'))
                    {
                        app.immersive = !app.immersive;
                        draw_needed = true;
                    } else {
                        if app.mode == UiMode::Help {
                            app.mode = UiMode::Normal;
                            draw_needed = true;
                        } else if app.mode == UiMode::Filter {
                            match key.code {
                                KeyCode::Esc => {
                                    app.leave_filter(true);
                                    draw_needed = true;
                                }
                                KeyCode::Enter => {
                                    let text = app.input_text();
                                    if let Some(command) = text.strip_prefix('!') {
                                        app.leave_filter(true);
                                        app.run(
                                            command.to_owned(),
                                            player,
                                            library,
                                            search.tx,
                                            theme_ctx.api,
                                            theme_ctx.settings,
                                        );
                                    } else {
                                        app.leave_filter(false);
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char(character) => {
                                    app.insert(character);
                                    app.filter = app.input_text();
                                    app.row_index = 0;
                                    draw_needed = true;
                                }
                                KeyCode::Backspace => {
                                    app.backspace();
                                    app.filter = app.input_text();
                                    app.row_index = 0;
                                    draw_needed = true;
                                }
                                KeyCode::Left => {
                                    app.left();
                                    draw_needed = true;
                                }
                                KeyCode::Right => {
                                    app.right();
                                    draw_needed = true;
                                }
                                _ => {}
                            }
                        } else if app.mode == UiMode::Command {
                            match key.code {
                                KeyCode::Esc => {
                                    app.leave_command();
                                    draw_needed = true;
                                }
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
                                    draw_needed = true;
                                }
                                KeyCode::Up => {
                                    app.history_up();
                                    draw_needed = true;
                                }
                                KeyCode::Down => {
                                    app.history_down();
                                    draw_needed = true;
                                }
                                KeyCode::Char(character) => {
                                    app.insert(character);
                                    draw_needed = true;
                                }
                                KeyCode::Backspace => {
                                    app.backspace();
                                    draw_needed = true;
                                }
                                KeyCode::Left => {
                                    app.left();
                                    draw_needed = true;
                                }
                                KeyCode::Right => {
                                    app.right();
                                    draw_needed = true;
                                }
                                _ => {}
                            }
                        } else {
                            let now = std::time::Instant::now();
                            if !matches!(key.code, KeyCode::Char('g')) {
                                app.g_pending = None;
                            }
                            match key.code {
                                KeyCode::Tab => {
                                    app.toggle_focus();
                                    draw_needed = true;
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    if app.focus == Focus::Library {
                                        app.move_view(true);
                                    } else {
                                        app.move_row(true);
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    if app.focus == Focus::Library {
                                        app.move_view(false);
                                    } else {
                                        app.move_row(false);
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char('g') => {
                                    if app.focus == Focus::List {
                                        let repeated = app.g_pending.is_some_and(|at| {
                                            now.duration_since(at)
                                                <= std::time::Duration::from_millis(300)
                                        });
                                        if repeated {
                                            app.row_index = 0;
                                            app.g_pending = None;
                                        } else {
                                            app.g_pending = Some(now);
                                        }
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char('G') => {
                                    if app.focus == Focus::List {
                                        app.row_index = app.filtered_row_count().saturating_sub(1);
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Enter => {
                                    if app.focus == Focus::Library {
                                        app.focus = Focus::List;
                                    } else if let Some(row) =
                                        app.filtered_rows().get(app.row_index).cloned()
                                        && let Some(sequence) = row.first()
                                    {
                                        app.run(
                                            format!("play {sequence}"),
                                            player,
                                            library,
                                            search.tx,
                                            theme_ctx.api,
                                            theme_ctx.settings,
                                        );
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Esc => {
                                    if app.immersive {
                                        app.immersive = false;
                                    } else if app.focus == Focus::List {
                                        app.focus = Focus::Library;
                                    } else {
                                        return Ok(());
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char(':') | KeyCode::Char('：') => {
                                    app.enter_command();
                                    draw_needed = true;
                                }
                                KeyCode::Char('/') => {
                                    app.enter_filter();
                                    draw_needed = true;
                                }
                                KeyCode::Char('?') => {
                                    app.mode = UiMode::Help;
                                    draw_needed = true;
                                }
                                KeyCode::Char('f') => {
                                    app.visual_mode = app.visual_mode.next();
                                    draw_needed = true;
                                }
                                KeyCode::Char('a') => {
                                    if let Some(row) =
                                        app.filtered_rows().get(app.row_index).cloned()
                                        && let Some(title) = row.get(1)
                                        && player.queue_add(vec![title.clone()]).is_ok()
                                    {
                                        app.views[2] =
                                            capture_table(player, &**library, "queue list", "队列");
                                        app.status = "已加入队列".to_owned();
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char('d') => {
                                    if app.focus == Focus::List
                                        && app.selected_view().map(|view| view.title.as_str())
                                            == Some("歌单")
                                    {
                                        if app.delete_armed {
                                            app.delete_armed = false;
                                            app.status = "删除确认（P3 接入）".to_owned();
                                        } else {
                                            app.delete_armed = true;
                                            app.status = "再按 d 确认删除".to_owned();
                                        }
                                    }
                                    draw_needed = true;
                                }
                                KeyCode::Char(' ') => {
                                    player.toggle();
                                    draw_needed = true;
                                }
                                KeyCode::Char('n') => {
                                    let _ = player.next();
                                    app.status = "下一首".to_owned();
                                    draw_needed = true;
                                }
                                KeyCode::Char('p') => {
                                    let _ = player.prev();
                                    app.status = "上一首".to_owned();
                                    draw_needed = true;
                                }
                                KeyCode::Char('+') | KeyCode::Char('=') => {
                                    let volume = (player.volume() + 0.05).clamp(0.0, 1.0);
                                    let _ = PlayerApi::volume(player, volume);
                                    app.status = format!("音量 {}%", (volume * 100.0).round());
                                    draw_needed = true;
                                }
                                KeyCode::Char('-') => {
                                    let volume = (player.volume() - 0.05).clamp(0.0, 1.0);
                                    let _ = PlayerApi::volume(player, volume);
                                    app.status = format!("音量 {}%", (volume * 100.0).round());
                                    draw_needed = true;
                                }
                                KeyCode::Char('q') => {
                                    if player.is_playing() && !app.quit_armed {
                                        app.quit_armed = true;
                                        app.status = "播放中，再按 q 确认退出".to_owned();
                                        draw_needed = true;
                                    } else {
                                        return Ok(());
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::Paste(text) => {
                    for character in text.chars() {
                        app.insert(character);
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
