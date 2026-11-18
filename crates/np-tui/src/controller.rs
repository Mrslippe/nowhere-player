//! 正常模式动作执行器：只接收语义动作，不解析按键。
//! 文本输入（Controller/Search/New Playlist）仍在 event_loop 的输入模式里处理。

use std::sync::Arc;

use crate::app::{App, parse_search_input};
use crate::audio::TuiAudio;
use crate::keymap::Action;
use crate::model::*;
use crate::playback::{QueueItem, TrackKey};
use crate::transport::TransportAvailability;
use np_app::{LibraryHandle, PlayerApi, SettingsHandle, ThemeApi};
use np_core::Block;

pub(crate) fn execute(
    action: Action,
    app: &mut App,
    player: &mut TuiAudio,
    library: &Arc<LibraryHandle>,
    search_tx: &std::sync::mpsc::Sender<(u64, Block)>,
    theme: &dyn ThemeApi,
    settings: &SettingsHandle,
) {
    match action {
        Action::NextSearchPlatform => cycle_search_platform(app, library, search_tx),
        Action::FocusLeft | Action::FocusRight => {
            app.search_input_active = false;
            app.focus = match app.focus {
                Focus::Browse => Focus::List,
                Focus::List => Focus::Browse,
                Focus::Controller => Focus::Controller,
            };
        }
        Action::MoveDown => {
            if app.focus == Focus::Browse {
                app.move_view(true);
            } else {
                app.move_row(true);
            }
        }
        Action::MoveUp => {
            if app.focus == Focus::Browse {
                app.move_view(false);
            } else {
                app.move_row(false);
            }
        }
        Action::TogglePlaylist => app.toggle_playlist(),
        Action::Enter => enter(app, player, library, search_tx, theme, settings),
        Action::Escape => {
            if player.stop_live() {
                app.set_transient_status("已停止直播");
            } else if app.pending_add.is_some() {
                app.cancel_pending_add();
            } else if app.pending_remove.is_some() {
                app.cancel_pending_remove();
            } else if app.pending_playlist_delete.is_some() {
                app.cancel_playlist_delete();
            } else if app.quit_armed {
                app.quit_armed = false;
                app.set_transient_status("已取消退出");
            } else if app.focus == Focus::List {
                app.focus = Focus::Browse;
            }
        }
        Action::ClearSearch => app.reset_search_to_empty(),
        Action::ResetSetting => {
            if let Err(error) = crate::settings_editor::reset_setting(app, player, settings, theme)
            {
                app.set_status(error);
            }
        }
        Action::Remove => {
            if app.pending_remove.is_some() {
                app.set_status("请按 Enter 确认移除".to_owned());
            } else if let Err(error) = app.arm_remove() {
                app.set_status(error);
            }
        }
        Action::Add => {
            if app.pending_add.is_some() {
                app.set_status("请按 Enter 确认添加".to_owned());
            } else if let Err(error) = app.begin_add_to_playlist() {
                app.set_status(error);
            }
        }
        Action::OpenSearch => crate::search_ui::open_search(app),
        Action::CycleMode => {
            let mode = player.cycle_mode();
            let _ = settings.set_and_save(
                "playback.mode",
                np_core::SettingValue::Str(player.mode_id().to_owned()),
            );
            app.set_transient_status(format!("模式：{}", playback_mode_label(mode)));
            app.refresh_settings_view(settings);
        }
        Action::SeekBack => {
            if player.is_live() {
                app.set_status("直播中不可跳转".to_owned());
            } else {
                let availability = TransportAvailability::from_player(player);
                if !availability.enabled(Action::SeekBack) {
                    app.set_transient_status("已到开头");
                } else {
                    let target = player.position_ms().saturating_sub(5_000);
                    if let Err(error) = PlayerApi::seek(player, target) {
                        app.set_status(error.message);
                    } else {
                        app.set_transient_status("后退 5s");
                        app.flash_transport(Action::SeekBack);
                    }
                }
            }
        }
        Action::SeekForward => {
            if player.is_live() {
                app.set_status("直播中不可跳转".to_owned());
            } else {
                let availability = TransportAvailability::from_player(player);
                if !availability.enabled(Action::SeekForward) {
                    app.set_transient_status("已到结尾");
                } else {
                    let position = player.position_ms().saturating_add(5_000);
                    let target = player
                        .duration_ms()
                        .map_or(position, |duration| position.min(duration));
                    if let Err(error) = PlayerApi::seek(player, target) {
                        app.set_status(error.message);
                    } else {
                        app.set_transient_status("前进 5s");
                        app.flash_transport(Action::SeekForward);
                    }
                }
            }
        }
        Action::OpenCommand => app.enter_command(),
        Action::LiveChat => {
            if player.is_live() {
                app.sync_danmaku(player.danmaku_received());
                app.begin_live_chat();
            } else {
                app.set_status("当前没有直播".to_owned());
            }
        }
        Action::ToggleDanmaku => {
            if player.is_live() {
                app.toggle_danmaku_overlay(player.danmaku_received());
            }
        }
        Action::RefreshLive => {
            if player.refresh_live() {
                app.set_transient_status("直播已刷新到最新");
            } else {
                app.set_status("当前没有直播".to_owned());
            }
        }
        Action::TogglePlay => {
            let before = player.is_playing();
            player.toggle();
            if before != player.is_playing() {
                app.flash_transport(Action::TogglePlay);
            }
        }
        Action::CreatePlaylist => {
            if app.pending_add.is_some()
                || app.pending_remove.is_some()
                || app.pending_playlist_delete.is_some()
                || app.quit_armed
            {
                app.set_status("请先完成或取消当前确认".to_owned());
            } else if let Err(error) = app.begin_playlist_create() {
                app.set_status(error);
            }
        }
        Action::NextTrack => {
            if player.is_live() {
                app.set_status("直播中不可切换".to_owned());
            } else {
                let availability = TransportAvailability::from_player(player);
                if !availability.enabled(Action::NextTrack) {
                    app.set_status("没有下一首".to_owned());
                } else {
                    match player.next() {
                        Ok(_) => {
                            app.set_transient_status("下一首");
                            app.flash_transport(Action::NextTrack);
                        }
                        Err(error) => app.set_status(error.message),
                    }
                    app.refresh_queue_view(player, &**library);
                }
            }
        }
        Action::DeletePlaylist => {
            if app.pending_playlist_delete.is_some() {
                app.set_status("请按 Enter 确认删除".to_owned());
            } else if app.pending_add.is_some() || app.pending_remove.is_some() || app.quit_armed {
                app.set_status("请先完成或取消当前确认".to_owned());
            } else if let Err(error) = app.arm_playlist_delete() {
                app.set_status(error);
            }
        }
        Action::PrevTrack => {
            if player.is_live() {
                app.set_status("直播中不可切换".to_owned());
            } else {
                let availability = TransportAvailability::from_player(player);
                if !availability.enabled(Action::PrevTrack) {
                    app.set_status("没有上一首".to_owned());
                } else {
                    match player.prev() {
                        Ok(_) => {
                            app.set_transient_status("上一首");
                            app.flash_transport(Action::PrevTrack);
                        }
                        Err(error) => app.set_status(error.message),
                    }
                    app.refresh_queue_view(player, &**library);
                }
            }
        }
        Action::VolumeUp | Action::VolumeDown => {
            let step = (player.volume() * 20.0).round() as i32;
            let delta = if action == Action::VolumeUp { 1 } else { -1 };
            let volume = (step + delta).clamp(0, 20) as f32 / 20.0;
            let _ = PlayerApi::volume(player, volume);
            let _ = settings.set_and_save(
                "playback.volume",
                np_core::SettingValue::Num(f64::from(volume)),
            );
            app.set_transient_status(format!("音量 {}%", (volume * 100.0).round()));
            app.refresh_settings_view(settings);
        }
        Action::Quit => {
            if !app.quit_armed {
                app.quit_armed = true;
                app.set_status("Enter确认退出 · Esc取消".to_owned());
            } else {
                app.set_status("请按 Enter 确认退出".to_owned());
            }
        }
    }
}

fn enter(
    app: &mut App,
    player: &TuiAudio,
    library: &Arc<LibraryHandle>,
    search_tx: &std::sync::mpsc::Sender<(u64, Block)>,
    theme: &dyn ThemeApi,
    settings: &SettingsHandle,
) {
    if app.focus == Focus::Browse {
        app.focus = Focus::List;
        return;
    }
    let kind = app.view_kind.get(app.view_index).cloned();
    if matches!(kind, Some(BrowseKind::Search))
        && matches!(app.search_display(), Some(SearchDisplay::Empty { .. }))
    {
        app.begin_search_edit();
        return;
    }
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
                if let Some(index) = app
                    .view_kind
                    .iter()
                    .position(|kind| matches!(kind, BrowseKind::Playlist(title) if title == &name))
                {
                    app.view_index = index;
                    app.row_index = 0;
                    app.focus = Focus::List;
                }
            }
        }
        Some(BrowseKind::Search) if app.filtered_rows().is_empty() => {}
        Some(BrowseKind::Settings) => {
            if let Err(error) =
                crate::settings_editor::begin_settings_edit(app, player, settings, theme)
            {
                app.set_status(error);
            }
        }
        _ => play_selected(app, player, library, search_tx),
    }
}

fn play_selected(
    app: &mut App,
    player: &TuiAudio,
    library: &Arc<LibraryHandle>,
    search_tx: &std::sync::mpsc::Sender<(u64, Block)>,
) {
    let kind = app.view_kind.get(app.view_index).cloned();
    let selected_ref = app
        .selected_view()
        .and_then(|view| view.refs.get(app.row_index))
        .cloned();
    match selected_ref {
        Some(TrackRef::Local(_)) => {
            let result = if let Some(BrowseKind::Playlist(name)) = &kind {
                let Some(view) = app.selected_view() else {
                    return;
                };
                let items: Vec<QueueItem> = (0..view.refs.len())
                    .filter_map(|index| queue_item_for(view, index, name))
                    .collect();
                let selected = app
                    .selected_view()
                    .and_then(|view| view.refs.get(app.row_index))
                    .map(TrackKey::from_track);
                player.replace_playlist(items, selected.as_ref())
            } else {
                let item = app
                    .selected_view()
                    .and_then(|view| queue_item_for(view, app.row_index, "选择"));
                if let Some(item) = item {
                    player.play_or_prepend(item)
                } else {
                    Err(np_app::CmdError::new("E_CMD_ARG", "没有可播放的曲目"))
                }
            };
            match result {
                Ok(_) => {
                    app.set_transient_status("播放中");
                    app.flash_transport(Action::TogglePlay);
                }
                Err(error) => app.set_status(error.message),
            }
            app.refresh_queue_view(player, &**library);
        }
        Some(TrackRef::Remote { .. }) => {
            if matches!(kind, Some(BrowseKind::Search)) {
                if let Err(error) =
                    app.play_online_search_result(app.row_index, library, search_tx, player)
                {
                    app.set_status(error);
                }
            } else if let Some(BrowseKind::Playlist(name)) = &kind {
                if let Err(error) = app.start_playlist_prepare(name, app.row_index, library) {
                    app.set_status(error);
                }
            } else {
                app.set_status("远程条目缺少播放上下文".to_owned());
            }
        }
        None => app.set_status("没有可播放的曲目".to_owned()),
    }
}

pub(crate) fn cycle_search_platform(
    app: &mut App,
    library: &Arc<LibraryHandle>,
    tx: &std::sync::mpsc::Sender<(u64, Block)>,
) {
    crate::search_ui::cycle_platform(app);
    if !matches!(app.view_kind.get(app.view_index), Some(BrowseKind::Search)) {
        return;
    }
    if app.search_attempted
        && !app.search_input.trim().is_empty()
        && let Ok(mut draft) = parse_search_input(&app.search_input)
    {
        draft.source = app.search_platform.source();
        app.end_search_edit();
        app.start_search_draft(draft, library, tx);
    }
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
