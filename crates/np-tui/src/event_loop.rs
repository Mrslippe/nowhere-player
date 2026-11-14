use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::app::{App, parse_search_input};
use crate::audio::TuiAudio;
use crate::model::*;
use crate::playback::{QueueItem, TrackKey};
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

fn parse_duration(text: &str) -> u64 {
    let mut parts = text.split(':');
    let minutes = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let seconds = parts
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    (minutes * 60 + seconds) * 1000
}

fn queue_item_for(view: &ViewData, index: usize, _origin: &str) -> Option<QueueItem> {
    let track = view.refs.get(index)?.clone();
    let row = view.rows.get(index)?;
    Some(QueueItem::new(
        track,
        row.get(1).cloned().unwrap_or_default(),
        row.get(2).cloned().unwrap_or_default(),
        row.get(3).map_or(0, |value| parse_duration(value)),
    ))
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
                app.search_attempted = true;
                draw_needed = true;
            }
        }
        while let Ok(path) = search.play_rx.try_recv() {
            match player.play(Some(path)) {
                Ok(_) => app.status = "播放中".to_owned(),
                Err(error) => app.status = error.message.clone(),
            }
            app.refresh_queue_view(player, &**library);
            draw_needed = true;
        }
        match player.handle_ended() {
            Ok(true) => {
                app.refresh_queue_view(player, &**library);
                draw_needed = true;
            }
            Ok(false) => {}
            Err(error) => {
                app.status = error.message.clone();
                draw_needed = true;
            }
        }
        #[cfg(windows)]
        if app.poll_tray(player) {
            return Ok(());
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
            std::time::Duration::from_millis(500)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.search_editing_input() {
                        match key.code {
                            KeyCode::Char(character) => app.insert_search_char(character),
                            KeyCode::Backspace => app.backspace_search_char(),
                            KeyCode::Left => app.move_search_cursor(false),
                            KeyCode::Right => app.move_search_cursor(true),
                            KeyCode::Enter => match parse_search_input(&app.search_input) {
                                Ok(draft) => {
                                    app.end_search_edit();
                                    app.start_search_draft(draft, library, search.tx);
                                }
                                Err(error) => app.status = error,
                            },
                            KeyCode::Esc => app.end_search_edit(),
                            _ => {}
                        }
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
                    } else {
                        match key.code {
                            KeyCode::Left | KeyCode::Right => {
                                app.search_input_active = false;
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
                                } else {
                                    let kind = app.view_kind.get(app.view_index).cloned();
                                    if matches!(kind, Some(BrowseKind::Search))
                                        && matches!(
                                            app.search_display(),
                                            Some(crate::model::SearchDisplay::Empty { .. })
                                        )
                                    {
                                        app.begin_search_edit();
                                    } else {
                                        match kind {
                                            Some(BrowseKind::PlaylistRoot) => {
                                                let name = app
                                                    .filtered_rows()
                                                    .get(app.row_index)
                                                    .and_then(|row| row.get(1))
                                                    .cloned();
                                                if let Some(name) = name {
                                                    if !app.playlist_expanded {
                                                        app.toggle_playlist();
                                                    }
                                                    if let Some(index) = app.view_kind.iter().position(
                                                    |kind| matches!(kind, BrowseKind::Playlist(title) if title == &name),
                                                ) {
                                                    app.view_index = index;
                                                    app.row_index = 0;
                                                    app.focus = Focus::List;
                                                }
                                                }
                                            }
                                            Some(BrowseKind::Search)
                                                if app.filtered_rows().is_empty() => {}
                                            Some(BrowseKind::Settings) => {}
                                            _ => {
                                                let selected_ref = app
                                                    .selected_view()
                                                    .and_then(|view| view.refs.get(app.row_index))
                                                    .cloned();
                                                match selected_ref {
                                                    Some(TrackRef::Local(_)) => {
                                                        let result = if let Some(
                                                            BrowseKind::Playlist(name),
                                                        ) = &kind
                                                        {
                                                            let Some(view) = app.selected_view()
                                                            else {
                                                                continue;
                                                            };
                                                            let items: Vec<QueueItem> =
                                                                (0..view.refs.len())
                                                                    .filter_map(|index| {
                                                                        queue_item_for(
                                                                            view, index, name,
                                                                        )
                                                                    })
                                                                    .collect();
                                                            let selected = app
                                                                .selected_view()
                                                                .and_then(|view| {
                                                                    view.refs.get(app.row_index)
                                                                })
                                                                .map(TrackKey::from_track);
                                                            player.replace_playlist(
                                                                items,
                                                                selected.as_ref(),
                                                            )
                                                        } else {
                                                            let item = app
                                                                .selected_view()
                                                                .and_then(|view| {
                                                                    queue_item_for(
                                                                        view,
                                                                        app.row_index,
                                                                        "选择",
                                                                    )
                                                                });
                                                            if let Some(item) = item {
                                                                player.play_or_prepend(item)
                                                            } else {
                                                                Err(np_app::CmdError::new(
                                                                    "E_CMD_ARG",
                                                                    "没有可播放的曲目",
                                                                ))
                                                            }
                                                        };
                                                        match result {
                                                            Ok(_) => {
                                                                app.status = "播放中".to_owned()
                                                            }
                                                            Err(error) => {
                                                                app.status = error.message.clone()
                                                            }
                                                        }
                                                        app.refresh_queue_view(player, &**library);
                                                    }
                                                    Some(TrackRef::Remote { .. }) => {
                                                        if matches!(kind, Some(BrowseKind::Search))
                                                        {
                                                            if let Err(error) = app
                                                                .play_online_search_result(
                                                                    app.row_index,
                                                                    library,
                                                                    search.tx,
                                                                )
                                                            {
                                                                app.status = error;
                                                            }
                                                        } else if let Some(BrowseKind::Playlist(
                                                            name,
                                                        )) = &kind
                                                        {
                                                            let index = app.row_index;
                                                            if let Err(error) = app
                                                                .play_playlist_item(
                                                                    name, index, player, library,
                                                                )
                                                            {
                                                                app.status = error;
                                                            }
                                                            app.refresh_queue_view(
                                                                player, &**library,
                                                            );
                                                        } else {
                                                            app.status =
                                                                "远程条目缺少播放上下文".to_owned();
                                                        }
                                                    }
                                                    None => {
                                                        app.status = "没有可播放的曲目".to_owned();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                if app.focus == Focus::List {
                                    app.focus = Focus::Browse;
                                }
                            }
                            KeyCode::Char('c')
                                if app.focus == Focus::List
                                    && matches!(
                                        app.view_kind.get(app.view_index),
                                        Some(BrowseKind::Search)
                                    )
                                    && (matches!(
                                        app.search_display(),
                                        Some(crate::model::SearchDisplay::Searching)
                                            | Some(crate::model::SearchDisplay::NoResults)
                                    ) || !app.filtered_rows().is_empty()) =>
                            {
                                app.reset_search_to_empty();
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
                                let step = (player.volume() * 20.0).round() as i32;
                                let volume = (step + 1).clamp(0, 20) as f32 / 20.0;
                                let _ = PlayerApi::volume(player, volume);
                                app.status = format!("音量 {}%", (volume * 100.0).round());
                            }
                            KeyCode::Char('-') => {
                                let step = (player.volume() * 20.0).round() as i32;
                                let volume = (step - 1).clamp(0, 20) as f32 / 20.0;
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
