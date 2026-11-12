use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::App;
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
                if let Some(index) = app
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::Search)
                {
                    app.view_index = index;
                    app.focus = Focus::List;
                }
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
                            KeyCode::Up => app.history_up(),
                            KeyCode::Down => app.history_down(),
                            KeyCode::Char(character) => app.insert(character),
                            KeyCode::Backspace => app.backspace(),
                            KeyCode::Left => app.left(),
                            KeyCode::Right => app.right(),
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Left | KeyCode::Right => {
                                app.focus = match app.focus {
                                    Focus::Browse => Focus::List,
                                    Focus::List => Focus::Browse,
                                    Focus::Controller => Focus::Controller,
                                };
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                if app.focus == Focus::Browse {
                                    app.move_view(true);
                                } else {
                                    app.move_row(true);
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                if app.focus == Focus::Browse {
                                    app.move_view(false);
                                } else {
                                    app.move_row(false);
                                }
                            }
                            KeyCode::Enter
                                if key.modifiers.contains(KeyModifiers::SHIFT)
                                    && app.focus == Focus::Browse =>
                            {
                                app.toggle_playlist();
                            }
                            KeyCode::Enter => {
                                if app.focus == Focus::Browse {
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
                            }
                            KeyCode::Esc => {
                                if app.focus == Focus::List {
                                    app.focus = Focus::Browse;
                                }
                            }
                            KeyCode::Char('/') => app.enter_command(),
                            KeyCode::Char(' ') => {
                                player.toggle();
                            }
                            KeyCode::Char('n') => {
                                let _ = player.next();
                                app.status = "下一首".to_owned();
                            }
                            KeyCode::Char('p') => {
                                let _ = player.prev();
                                app.status = "上一首".to_owned();
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                let volume = (player.volume() + 0.05).clamp(0.0, 1.0);
                                let _ = PlayerApi::volume(player, volume);
                                app.status = format!("音量 {}%", (volume * 100.0).round());
                            }
                            KeyCode::Char('-') => {
                                let volume = (player.volume() - 0.05).clamp(0.0, 1.0);
                                let _ = PlayerApi::volume(player, volume);
                                app.status = format!("音量 {}%", (volume * 100.0).round());
                            }
                            KeyCode::Char('q') => {
                                if player.is_playing() && !app.quit_armed {
                                    app.quit_armed = true;
                                    app.status = "播放中，再按 q 确认退出".to_owned();
                                } else {
                                    return Ok(());
                                }
                            }
                            _ => {}
                        }
                    }
                    draw_needed = true;
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
