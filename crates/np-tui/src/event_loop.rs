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

fn playback_mode_label(mode: np_core::PlaybackMode) -> &'static str {
    match (mode.repeat, mode.shuffle) {
        (np_core::RepeatMode::One, false) => "单曲循环",
        (np_core::RepeatMode::Off, true) => "随机播放",
        (np_core::RepeatMode::All, false) => "列表循环",
        _ => "顺序播放",
    }
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

fn is_settings(app: &App) -> bool {
    matches!(
        app.view_kind.get(app.view_index),
        Some(BrowseKind::Settings)
    )
}

fn cycle_search_platform(
    app: &mut App,
    library: &Arc<LibraryHandle>,
    tx: &std::sync::mpsc::Sender<(u64, Block)>,
) {
    crate::search_ui::cycle_platform(app);
    if !matches!(app.view_kind.get(app.view_index), Some(BrowseKind::Search)) {
        return;
    }
    // 已有查询时，Tab 不只是换标签，而是立刻用新平台重跑；旧请求会因 search_id 失配被丢弃。
    if app.search_attempted
        && !app.search_input.trim().is_empty()
        && let Ok(mut draft) = parse_search_input(&app.search_input)
    {
        draft.source = app.search_platform.source();
        app.end_search_edit();
        app.start_search_draft(draft, library, tx);
    }
}

fn queue_item_for(view: &ViewData, index: usize, _origin: &str) -> Option<QueueItem> {
    let track = view.refs.get(index)?.clone();
    let row = view.rows.get(index)?;
    Some(QueueItem::new(
        track,
        row.get(1).cloned().unwrap_or_default(),
        row.get(2).cloned().unwrap_or_default(),
        view.durations
            .get(index)
            .copied()
            .unwrap_or_else(|| row.get(3).map_or(0, |value| parse_duration(value))),
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
                app.status = "搜索完成".to_owned();
                app.search_pending = false;
                app.search_attempted = true;
                draw_needed = true;
            }
        }
        while let Ok(path) = search.play_rx.try_recv() {
            match player.play(Some(path)) {
                Ok(_) => {
                    app.status = "播放中".to_owned();
                    app.flash_transport(TransportButton::Play);
                }
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
                    if app.search_editing_input() {
                        match key.code {
                            KeyCode::Char(character) => app.insert_search_char(character),
                            KeyCode::Backspace => app.backspace_search_char(),
                            KeyCode::Left => app.move_search_cursor(false),
                            KeyCode::Right => app.move_search_cursor(true),
                            KeyCode::Tab => {
                                cycle_search_platform(app, library, search.tx);
                            }
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
                    } else if app.playlist_input_active() {
                        match key.code {
                            KeyCode::Char(character) => app.insert_playlist_char(character),
                            KeyCode::Backspace => app.backspace_playlist_char(),
                            KeyCode::Left => app.move_playlist_cursor(false),
                            KeyCode::Right => app.move_playlist_cursor(true),
                            KeyCode::Enter => {
                                if let Err(error) = app.confirm_playlist_create(&**library) {
                                    app.status = error;
                                }
                            }
                            KeyCode::Esc => app.cancel_playlist_create(),
                            _ => {}
                        }
                        draw_needed = true;
                        continue;
                    } else {
                        if key.code == KeyCode::Enter {
                            if app.pending_add.is_some() {
                                if let Err(error) = app.confirm_add_to_playlist(&**library) {
                                    app.status = error;
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.pending_remove.is_some() {
                                if let Err(error) = app.confirm_remove(player, &**library) {
                                    app.status = error;
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.pending_playlist_delete.is_some() {
                                if let Err(error) = app.confirm_playlist_delete(&**library) {
                                    app.status = error;
                                }
                                draw_needed = true;
                                continue;
                            }
                            if app.quit_armed {
                                return Ok(());
                            }
                        }
                        match key.code {
                            KeyCode::Tab
                                if matches!(
                                    app.view_kind.get(app.view_index),
                                    Some(BrowseKind::Search)
                                ) =>
                            {
                                cycle_search_platform(app, library, search.tx);
                            }
                            KeyCode::Left | KeyCode::Right
                                if app.focus == Focus::List && is_settings(app) =>
                            {
                                let direction = if key.code == KeyCode::Left { -1 } else { 1 };
                                if let Err(error) = crate::settings_page::adjust_setting(
                                    app,
                                    direction,
                                    player,
                                    theme_ctx.settings,
                                    theme_ctx.api,
                                ) {
                                    app.status = error;
                                }
                            }
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
                                            Some(BrowseKind::Settings) => {
                                                if let Err(error) =
                                                    crate::settings_page::adjust_setting(
                                                        app,
                                                        1,
                                                        player,
                                                        theme_ctx.settings,
                                                        theme_ctx.api,
                                                    )
                                                {
                                                    app.status = error;
                                                }
                                            }
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
                                                                app.status = "播放中".to_owned();
                                                                app.flash_transport(
                                                                    TransportButton::Play,
                                                                );
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
                                                                    player,
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
                                                                .start_playlist_prepare(
                                                                    name, index, library,
                                                                )
                                                            {
                                                                app.status = error;
                                                            }
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
                                if app.pending_add.is_some() {
                                    app.cancel_pending_add();
                                } else if app.pending_remove.is_some() {
                                    app.cancel_pending_remove();
                                } else if app.pending_playlist_delete.is_some() {
                                    app.cancel_playlist_delete();
                                } else if app.quit_armed {
                                    app.quit_armed = false;
                                    app.status = "已取消退出".to_owned();
                                } else if app.focus == Focus::List {
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
                            KeyCode::Char('r') if app.focus == Focus::List && is_settings(app) => {
                                if let Err(error) = crate::settings_page::reset_setting(
                                    app,
                                    player,
                                    theme_ctx.settings,
                                    theme_ctx.api,
                                ) {
                                    app.status = error;
                                }
                            }
                            KeyCode::Char('r') if app.focus == Focus::List => {
                                if app.pending_remove.is_some() {
                                    app.status = "请按 Enter 确认移除".to_owned();
                                } else if let Err(error) = app.arm_remove() {
                                    app.status = error;
                                }
                            }
                            KeyCode::Char('a') if app.focus == Focus::List => {
                                if app.pending_add.is_some() {
                                    app.status = "请按 Enter 确认添加".to_owned();
                                } else if let Err(error) = app.begin_add_to_playlist() {
                                    app.status = error;
                                }
                            }
                            KeyCode::Char('s') => {
                                crate::search_ui::open_search(app);
                            }
                            KeyCode::Char('m') => {
                                let mode = player.cycle_mode();
                                let _ = theme_ctx.settings.set_and_save(
                                    "playback.mode",
                                    np_core::SettingValue::Str(player.mode_id().to_owned()),
                                );
                                app.status = format!("模式：{}", playback_mode_label(mode));
                                app.refresh_settings_view(theme_ctx.settings);
                            }
                            KeyCode::Char('[') | KeyCode::Char('{') | KeyCode::Char('【') => {
                                if !player.can_seek_back() {
                                    app.status = "已到开头".to_owned();
                                } else {
                                    let target = player.position_ms().saturating_sub(5_000);
                                    if let Err(error) = PlayerApi::seek(player, target) {
                                        app.status = error.message;
                                    } else {
                                        app.status = "后退 5s".to_owned();
                                        app.flash_transport(TransportButton::SeekBack);
                                    }
                                }
                            }
                            KeyCode::Char(']') | KeyCode::Char('}') | KeyCode::Char('】') => {
                                if !player.can_seek_forward() {
                                    app.status = "已到结尾".to_owned();
                                } else {
                                    let position = player.position_ms().saturating_add(5_000);
                                    let target = player
                                        .duration_ms()
                                        .map_or(position, |duration| position.min(duration));
                                    if let Err(error) = PlayerApi::seek(player, target) {
                                        app.status = error.message;
                                    } else {
                                        app.status = "前进 5s".to_owned();
                                        app.flash_transport(TransportButton::SeekForward);
                                    }
                                }
                            }
                            KeyCode::Char('/') => app.enter_command(),
                            KeyCode::Char(' ') => {
                                let before = player.is_playing();
                                player.toggle();
                                if before != player.is_playing() {
                                    app.flash_transport(TransportButton::Play);
                                }
                            }
                            KeyCode::Char('n') => {
                                if matches!(
                                    app.view_kind.get(app.view_index),
                                    Some(BrowseKind::PlaylistRoot)
                                ) {
                                    if app.pending_add.is_some()
                                        || app.pending_remove.is_some()
                                        || app.pending_playlist_delete.is_some()
                                        || app.quit_armed
                                    {
                                        app.status = "请先完成或取消当前确认".to_owned();
                                    } else if let Err(error) = app.begin_playlist_create() {
                                        app.status = error;
                                    }
                                } else if !player.can_next() {
                                    app.status = "没有下一首".to_owned();
                                } else {
                                    match player.next() {
                                        Ok(_) => {
                                            app.status = "下一首".to_owned();
                                            app.flash_transport(TransportButton::Next);
                                        }
                                        Err(error) => app.status = error.message,
                                    }
                                    app.refresh_queue_view(player, &**library);
                                }
                            }
                            KeyCode::Char('d')
                                if matches!(
                                    app.view_kind.get(app.view_index),
                                    Some(BrowseKind::PlaylistRoot)
                                ) =>
                            {
                                if app.pending_playlist_delete.is_some() {
                                    app.status = "请按 Enter 确认删除".to_owned();
                                } else if app.pending_add.is_some()
                                    || app.pending_remove.is_some()
                                    || app.quit_armed
                                {
                                    app.status = "请先完成或取消当前确认".to_owned();
                                } else if let Err(error) = app.arm_playlist_delete() {
                                    app.status = error;
                                }
                            }
                            KeyCode::Char('p') => {
                                if !player.can_prev() {
                                    app.status = "没有上一首".to_owned();
                                } else {
                                    match player.prev() {
                                        Ok(_) => {
                                            app.status = "上一首".to_owned();
                                            app.flash_transport(TransportButton::Prev);
                                        }
                                        Err(error) => app.status = error.message,
                                    }
                                    app.refresh_queue_view(player, &**library);
                                }
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                let step = (player.volume() * 20.0).round() as i32;
                                let volume = (step + 1).clamp(0, 20) as f32 / 20.0;
                                let _ = PlayerApi::volume(player, volume);
                                let _ = theme_ctx.settings.set_and_save(
                                    "playback.volume",
                                    np_core::SettingValue::Num(f64::from(volume)),
                                );
                                app.status = format!("音量 {}%", (volume * 100.0).round());
                                app.refresh_settings_view(theme_ctx.settings);
                            }
                            KeyCode::Char('-') => {
                                let step = (player.volume() * 20.0).round() as i32;
                                let volume = (step - 1).clamp(0, 20) as f32 / 20.0;
                                let _ = PlayerApi::volume(player, volume);
                                let _ = theme_ctx.settings.set_and_save(
                                    "playback.volume",
                                    np_core::SettingValue::Num(f64::from(volume)),
                                );
                                app.status = format!("音量 {}%", (volume * 100.0).round());
                                app.refresh_settings_view(theme_ctx.settings);
                            }
                            KeyCode::Char('q') => {
                                if !app.quit_armed {
                                    app.quit_armed = true;
                                    app.status = "Enter确认退出 · Esc取消".to_owned();
                                } else {
                                    app.status = "请按 Enter 确认退出".to_owned();
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
