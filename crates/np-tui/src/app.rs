pub(crate) use crate::model::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::audio::TuiAudio;
use crate::keymap::Action;
use crate::playback::{QueueItem, TrackKey};
use crate::settings_editor::SettingsEditor;
use crate::settings_page::{SettingsField, settings_page};
use crate::status::BlockStatusExt;
pub(crate) use crate::status::now_millis;
use crate::theme::Palette;
#[cfg(windows)]
use crate::tray;
use crate::{osc, startup};
use np_app::{
    Execution, LibraryApi, LibraryHandle, NewPlaylistItem, NoopLibraryObserver, PlayerApi,
    SearchHit, Services, SettingsHandle, SourceId, SourceTrack, ThemeApi, complete_for,
    execute_with_services, online_play_hit, online_play_started_block, run_online_play,
};
use np_core::{Block, BlockStatus};

pub(crate) struct PlaylistInput {
    pub(crate) value: String,
    pub(crate) cursor: usize,
}

pub(crate) struct PendingAdd {
    pub(crate) item: QueueItem,
    origin_view: usize,
    origin_row: usize,
    origin_focus: Focus,
}

pub(crate) struct PlaylistReady {
    generation: u64,
    result: Result<(Vec<QueueItem>, Option<TrackKey>), String>,
}

pub(crate) struct App {
    pub(crate) input: Vec<char>,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) status: String,
    /// 瞬时状态到期时间；`None` 表示常驻（错误 / 进行中）。
    pub(crate) status_expires_at: Option<Instant>,
    pub(crate) spectrum: Vec<f32>,
    pub(crate) waveform: Vec<f32>,
    pub(crate) stereo_left: Vec<f32>,
    pub(crate) stereo_right: Vec<f32>,
    pub(crate) spectrum_generation: u64,
    pub(crate) completion: Option<np_core::Completion>,
    pub(crate) completion_index: usize,
    pub(crate) last_edit: std::time::Instant,
    pub(crate) history_nav: bool,
    pub(crate) visual_mode: VisualMode,
    pub(crate) search_id: u64,
    pub(crate) search_pending: bool,
    pub(crate) search_query: String,
    pub(crate) search_input: String,
    pub(crate) search_cursor: usize,
    pub(crate) search_input_active: bool,
    pub(crate) search_attempted: bool,
    pub(crate) search_platform: SearchPlatform,
    pub(crate) show_key_hints: bool,
    pub(crate) transport_feedback: Option<(Action, Instant)>,
    pub(crate) settings_fields: Vec<SettingsField>,
    pub(crate) settings_editor: SettingsEditor,
    pub(crate) no_color_ignored: bool,
    playlist_prepare_tx: std::sync::mpsc::Sender<PlaylistReady>,
    playlist_prepare_rx: std::sync::mpsc::Receiver<PlaylistReady>,
    playlist_generation: u64,
    #[cfg(windows)]
    pub(crate) tray_rx: Option<std::sync::mpsc::Receiver<tray::TrayAction>>,
    pub(crate) palette: Palette,
    pub(crate) theme_id: String,
    pub(crate) theme_name: String,
    pub(crate) focus: Focus,
    pub(crate) quit_armed: bool,
    pub(crate) remote_tx: Option<std::sync::mpsc::Sender<String>>,
    pub(crate) views: Vec<ViewData>,
    pub(crate) view_depth: Vec<u8>,
    pub(crate) view_kind: Vec<BrowseKind>,
    pub(crate) playlist_children: Vec<ViewData>,
    pub(crate) playlist_expanded: bool,
    pub(crate) view_index: usize,
    pub(crate) row_index: usize,
    pub(crate) queue_marker: Option<usize>,
    pub(crate) pending_add: Option<PendingAdd>,
    pub(crate) pending_remove: Option<(usize, usize)>,
    pub(crate) pending_playlist_delete: Option<String>,
    pub(crate) playlist_input: Option<PlaylistInput>,
    pub(crate) chat_input: Option<crate::chat_input::ChatInput>,
    pub(crate) chat_notice: Option<String>,
    pub(crate) chat_notice_expires_at: Option<Instant>,
}

impl App {
    pub(crate) fn new() -> Self {
        let (playlist_prepare_tx, playlist_prepare_rx) = std::sync::mpsc::channel();
        Self {
            input: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            blocks: Vec::new(),
            status: "就绪".to_owned(),
            status_expires_at: None,
            spectrum: vec![0.0; 16],
            waveform: vec![0.0; 2048],
            stereo_left: vec![0.0; 2048],
            stereo_right: vec![0.0; 2048],
            spectrum_generation: 0,
            completion: None,
            completion_index: 0,
            last_edit: std::time::Instant::now(),
            history_nav: false,
            visual_mode: VisualMode::Spectrum,
            search_id: 0,
            search_pending: false,
            search_query: String::new(),
            search_input: String::new(),
            search_cursor: 0,
            search_input_active: false,
            search_attempted: false,
            search_platform: SearchPlatform::All,
            show_key_hints: false,
            transport_feedback: None,
            settings_fields: Vec::new(),
            settings_editor: SettingsEditor::new(),
            no_color_ignored: false,
            playlist_prepare_tx,
            playlist_prepare_rx,
            playlist_generation: 0,
            #[cfg(windows)]
            tray_rx: None,
            palette: Palette::terminal(),
            theme_id: "milkgreen".to_owned(),
            theme_name: "MilkGreen".to_owned(),
            focus: Focus::Browse,
            quit_armed: false,
            remote_tx: None,
            views: Vec::new(),
            view_depth: Vec::new(),
            view_kind: Vec::new(),
            playlist_children: Vec::new(),
            playlist_expanded: false,
            view_index: 0,
            row_index: 0,
            queue_marker: None,
            pending_add: None,
            pending_remove: None,
            pending_playlist_delete: None,
            playlist_input: None,
            chat_input: None,
            chat_notice: None,
            chat_notice_expires_at: None,
        }
    }

    pub(crate) fn input_text(&self) -> String {
        self.input.iter().collect()
    }
    pub(crate) fn flash_transport(&mut self, button: Action) {
        self.transport_feedback = Some((button, Instant::now()));
    }
    pub(crate) fn insert(&mut self, character: char) {
        self.input.insert(self.cursor, character);
        self.cursor += 1;
        self.history_cursor = None;
        self.history_nav = false;
        self.last_edit = std::time::Instant::now();
        self.completion = None;
    }
    pub(crate) fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
            self.history_cursor = None;
            self.history_nav = false;
            self.last_edit = std::time::Instant::now();
            self.completion = None;
        }
    }
    pub(crate) fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(crate) fn right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    pub(crate) fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = self
            .history_cursor
            .map_or(self.history.len() - 1, |index| index.saturating_sub(1));
        self.history_cursor = Some(next);
        self.input = self.history[next].chars().collect();
        self.cursor = self.input.len();
    }

    pub(crate) fn history_down(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_cursor = None;
            self.input.clear();
            self.cursor = 0;
        } else {
            self.history_cursor = Some(index + 1);
            self.history_nav = true;
            self.completion = None;
            self.input = self.history[index + 1].chars().collect();
            self.cursor = self.input.len();
        }
    }

    pub(crate) fn apply_theme_reply(&mut self, reply: np_core::theme::ThemeApplyReply) {
        let (palette, warnings) = Palette::from_tokens(&reply.tokens);
        self.palette = palette;
        if let Some(warning) = warnings.first() {
            self.set_status(format!("主题告警：{warning}"));
        }
        osc::set_background(self.palette.bg);
        self.theme_id = reply.id;
        self.theme_name = reply.name;
        if let Some(warning) = reply.warnings.first() {
            self.set_status(format!("主题告警：{warning}"));
        }
    }

    pub(crate) fn load_theme(&mut self, theme: &dyn ThemeApi, id: &str) {
        match theme.apply(id) {
            Ok(reply) => self.apply_theme_reply(reply),
            Err(error) => {
                if let Some(theme) = np_core::theme::BuiltinTheme::find("milkgreen") {
                    self.apply_theme_reply(np_core::theme::ThemeApplyReply::from_builtin(theme));
                    self.set_status(format!("主题加载失败：{}，已回退 MilkGreen", error.message));
                } else {
                    self.palette = Palette::default_dark();
                    self.theme_id = "milkgreen".to_owned();
                    self.theme_name = "MilkGreen".to_owned();
                    self.set_status(format!("主题加载失败：{}", error.message));
                }
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn poll_tray(&mut self, player: &mut TuiAudio) -> bool {
        let Some(rx) = &self.tray_rx else {
            return false;
        };
        let mut actions = Vec::new();
        while let Ok(action) = rx.try_recv() {
            actions.push(action);
        }
        for action in actions {
            match action {
                tray::TrayAction::Show => tray::focus_console(),
                tray::TrayAction::Toggle => {
                    player.toggle();
                    self.set_transient_status(if player.is_playing() {
                        "托盘：继续播放"
                    } else {
                        "托盘：已暂停"
                    });
                }
                tray::TrayAction::Next => match player.next() {
                    Ok(_) => self.set_transient_status("媒体键：下一首"),
                    Err(error) => self.set_status(format!("下一首失败：{}", error.message)),
                },
                tray::TrayAction::Prev => match player.prev() {
                    Ok(_) => self.set_transient_status("媒体键：上一首"),
                    Err(error) => self.set_status(format!("上一首失败：{}", error.message)),
                },
                tray::TrayAction::HotkeyFailed => {
                    self.set_status("全局热键注册失败（可能被占用）".to_owned());
                }
                tray::TrayAction::Quit => return true,
            }
        }
        false
    }

    pub(crate) fn load_views(
        &mut self,
        player: &TuiAudio,
        library: &dyn LibraryApi,
        settings: &SettingsHandle,
    ) {
        let playlist_root = playlist_root_view(library);
        let library_view = library_view(library);
        let queue = queue_view(player, library);
        self.queue_marker = player.queue_list().ok().and_then(|view| view.index);
        let search = ViewData {
            title: "搜索".to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        };
        let settings_page = settings_page(settings);
        self.settings_fields = settings_page.fields;
        self.settings_editor = SettingsEditor::new();
        let settings_view = settings_page.view;
        let playlists = library.playlist_list().unwrap_or_default();
        let playlist_children: Vec<ViewData> = playlists
            .iter()
            .map(|playlist| playlist_child_view(library, &playlist.name))
            .collect();
        let playlist_expanded = playlist_children.len() <= 5;

        let mut views = vec![playlist_root];
        let mut view_depth = vec![0];
        let mut view_kind = vec![BrowseKind::PlaylistRoot];
        if playlist_expanded {
            for (index, child) in playlist_children.iter().enumerate() {
                views.push(child.clone());
                view_depth.push(1);
                view_kind.push(BrowseKind::Playlist(
                    playlists
                        .get(index)
                        .map(|playlist| playlist.name.clone())
                        .unwrap_or_default(),
                ));
            }
        }
        for (view, kind) in [
            (library_view, BrowseKind::Library),
            (search, BrowseKind::Search),
            (queue, BrowseKind::Queue),
            (settings_view, BrowseKind::Settings),
        ] {
            views.push(view);
            view_depth.push(0);
            view_kind.push(kind);
        }
        self.views = views;
        self.view_depth = view_depth;
        self.view_kind = view_kind;
        self.playlist_children = playlist_children;
        self.playlist_expanded = playlist_expanded;
        self.view_index = 0;
        self.row_index = 0;
    }

    pub(crate) fn refresh_settings_view(&mut self, settings: &SettingsHandle) {
        let page = settings_page(settings);
        self.settings_fields = page.fields;
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::Settings)
        {
            self.views[index] = page.view;
            if self.view_index == index {
                self.row_index = self
                    .row_index
                    .min(self.settings_fields.len().saturating_sub(1));
                self.settings_editor.select(self.row_index);
            }
        }
    }

    pub(crate) fn toggle_playlist(&mut self) {
        if self.playlist_children.is_empty() {
            return;
        }
        let Some(root) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::PlaylistRoot)
        else {
            return;
        };
        if self.view_index != root {
            return;
        }
        if self.playlist_expanded {
            let count = self.playlist_children.len();
            let selected_child = self
                .view_kind
                .get(self.view_index)
                .is_some_and(|kind| matches!(kind, BrowseKind::Playlist(_)));
            self.views.drain(root + 1..root + 1 + count);
            self.view_depth.drain(root + 1..root + 1 + count);
            self.view_kind.drain(root + 1..root + 1 + count);
            self.playlist_expanded = false;
            if selected_child {
                self.view_index = root;
                self.row_index = 0;
            }
        } else {
            let children = self.playlist_children.clone();
            for (offset, child) in children.into_iter().enumerate() {
                let kind = BrowseKind::Playlist(child.title.clone());
                self.views.insert(root + 1 + offset, child);
                self.view_depth.insert(root + 1 + offset, 1);
                self.view_kind.insert(root + 1 + offset, kind);
            }
            self.playlist_expanded = true;
        }
        self.view_index = self.view_index.min(self.views.len().saturating_sub(1));
    }

    pub(crate) fn refresh_queue_view(&mut self, player: &TuiAudio, library: &dyn LibraryApi) {
        self.queue_marker = player.queue_list().ok().and_then(|view| view.index);
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::Queue)
        {
            self.views[index] = queue_view(player, library);
        }
    }

    pub(crate) fn selected_view(&self) -> Option<&ViewData> {
        self.views.get(self.view_index)
    }

    pub(crate) fn filtered_rows(&self) -> Vec<Vec<String>> {
        let Some(view) = self.selected_view() else {
            return Vec::new();
        };
        view.rows.clone()
    }

    pub(crate) fn filtered_row_count(&self) -> usize {
        self.filtered_rows().len()
    }

    pub(crate) fn search_display(&self) -> Option<SearchDisplay> {
        let current = self.view_kind.get(self.view_index)?;
        if *current != BrowseKind::Search {
            return None;
        }
        if self.search_pending {
            return Some(SearchDisplay::Searching);
        }
        if !self.search_attempted {
            return Some(SearchDisplay::Empty {
                query: self.search_input.clone(),
            });
        }
        if self.filtered_row_count() == 0 {
            return Some(SearchDisplay::NoResults);
        }
        None
    }

    pub(crate) fn playlist_input_active(&self) -> bool {
        self.playlist_input.is_some()
    }

    pub(crate) fn begin_playlist_create(&mut self) -> Result<(), String> {
        if !matches!(
            self.view_kind.get(self.view_index),
            Some(BrowseKind::PlaylistRoot)
        ) {
            return Err("只能在 PlayList 页创建歌单".to_owned());
        }
        self.playlist_input = Some(PlaylistInput {
            value: String::new(),
            cursor: 0,
        });
        self.focus = Focus::List;
        self.set_status("输入歌单名 · Enter创建 · Esc取消".to_owned());
        Ok(())
    }

    pub(crate) fn cancel_playlist_create(&mut self) {
        if self.playlist_input.take().is_some() {
            self.set_transient_status("已取消创建");
        }
    }

    pub(crate) fn insert_playlist_char(&mut self, character: char) {
        let Some(input) = self.playlist_input.as_mut() else {
            return;
        };
        let byte = char_to_byte_index(&input.value, input.cursor);
        input.value.insert(byte, character);
        input.cursor += 1;
    }

    pub(crate) fn backspace_playlist_char(&mut self) {
        let Some(input) = self.playlist_input.as_mut() else {
            return;
        };
        if input.cursor > 0 {
            input.cursor -= 1;
            let byte = char_to_byte_index(&input.value, input.cursor);
            input.value.remove(byte);
        }
    }

    pub(crate) fn move_playlist_cursor(&mut self, right: bool) {
        let Some(input) = self.playlist_input.as_mut() else {
            return;
        };
        if right {
            if input.cursor < input.value.chars().count() {
                input.cursor += 1;
            }
        } else {
            input.cursor = input.cursor.saturating_sub(1);
        }
    }

    pub(crate) fn confirm_playlist_create(
        &mut self,
        library: &dyn LibraryApi,
    ) -> Result<(), String> {
        let Some(input) = self.playlist_input.take() else {
            return Err("没有创建中的歌单".to_owned());
        };
        let name = input.value.trim().to_owned();
        if name.is_empty() {
            self.playlist_input = Some(input);
            return Err("歌单名不能为空".to_owned());
        }
        if let Err(error) = library.playlist_create(&name) {
            self.playlist_input = Some(input);
            return Err(error.message);
        }
        self.playlist_expanded = true;
        self.rebuild_playlist_views(library, Some(&name));
        self.set_transient_status(format!("已创建歌单：{name}"));
        Ok(())
    }

    pub(crate) fn arm_playlist_delete(&mut self) -> Result<(), String> {
        if !matches!(
            self.view_kind.get(self.view_index),
            Some(BrowseKind::PlaylistRoot)
        ) {
            return Err("只能在 PlayList 页删除歌单".to_owned());
        }
        let Some(name) = self
            .filtered_rows()
            .get(self.row_index)
            .and_then(|row| row.get(1).cloned())
        else {
            return Err("没有选中歌单".to_owned());
        };
        self.pending_playlist_delete = Some(name.clone());
        self.set_status(format!("Enter确认删除歌单：{name} · Esc取消"));
        Ok(())
    }

    pub(crate) fn cancel_playlist_delete(&mut self) {
        if self.pending_playlist_delete.take().is_some() {
            self.set_transient_status("已取消删除歌单");
        }
    }

    pub(crate) fn confirm_playlist_delete(
        &mut self,
        library: &dyn LibraryApi,
    ) -> Result<(), String> {
        let Some(name) = self.pending_playlist_delete.take() else {
            return Err("没有待删除歌单".to_owned());
        };
        let playlist = library
            .playlist_by_name(&name)
            .map_err(|error| error.message.clone())?
            .ok_or_else(|| format!("歌单不存在：{name}"))?;
        library
            .playlist_delete(playlist.id)
            .map_err(|error| error.message.clone())?;
        self.rebuild_playlist_views(library, None);
        self.set_transient_status(format!("已删除歌单：{name}"));
        Ok(())
    }

    fn rebuild_playlist_views(&mut self, library: &dyn LibraryApi, selected: Option<&str>) {
        let playlists = library.playlist_list().unwrap_or_default();
        let children: Vec<ViewData> = playlists
            .iter()
            .map(|playlist| playlist_child_view(library, &playlist.name))
            .collect();
        let old_views = self.views.clone();
        let old_kinds = self.view_kind.clone();
        let mut views = vec![playlist_root_view(library)];
        let mut depths = vec![0];
        let mut kinds = vec![BrowseKind::PlaylistRoot];
        if self.playlist_expanded {
            for (index, child) in children.iter().enumerate() {
                views.push(child.clone());
                depths.push(1);
                kinds.push(BrowseKind::Playlist(
                    playlists
                        .get(index)
                        .map(|playlist| playlist.name.clone())
                        .unwrap_or_default(),
                ));
            }
        }
        for (index, view) in old_views.into_iter().enumerate() {
            let Some(kind) = old_kinds.get(index).cloned() else {
                continue;
            };
            if matches!(kind, BrowseKind::PlaylistRoot | BrowseKind::Playlist(_)) {
                continue;
            }
            views.push(view);
            depths.push(0);
            kinds.push(kind);
        }
        self.playlist_children = children;
        self.views = views;
        self.view_depth = depths;
        self.view_kind = kinds;
        self.view_index = selected
            .and_then(|name| {
                self.view_kind
                    .iter()
                    .position(|kind| matches!(kind, BrowseKind::Playlist(title) if title == name))
            })
            .unwrap_or_else(|| {
                self.view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::PlaylistRoot)
                    .unwrap_or(0)
            });
        self.row_index = 0;
    }

    pub(crate) fn arm_remove(&mut self) -> Result<(), String> {
        let Some(kind) = self.view_kind.get(self.view_index) else {
            return Err("当前没有列表".to_owned());
        };
        if matches!(kind, BrowseKind::PlaylistRoot | BrowseKind::Settings) {
            return Err("当前列表不是歌曲列表".to_owned());
        }
        let Some(view) = self.selected_view() else {
            return Err("当前没有列表".to_owned());
        };
        if view.refs.get(self.row_index).is_none() {
            return Err("当前行不是歌曲".to_owned());
        }
        self.pending_remove = Some((self.view_index, self.row_index));
        self.set_status("Enter确认移除 · Esc取消".to_owned());
        Ok(())
    }

    pub(crate) fn cancel_pending_remove(&mut self) {
        if self.pending_remove.take().is_some() {
            self.set_transient_status("已取消移除");
        }
    }

    pub(crate) fn confirm_remove(
        &mut self,
        player: &TuiAudio,
        library: &dyn LibraryApi,
    ) -> Result<(), String> {
        let Some((view_index, row_index)) = self.pending_remove.take() else {
            return Err("没有待移除歌曲".to_owned());
        };
        let kind = self
            .view_kind
            .get(view_index)
            .cloned()
            .ok_or_else(|| "当前列表已不存在".to_owned())?;
        let view = self
            .views
            .get(view_index)
            .ok_or_else(|| "当前列表已不存在".to_owned())?;
        let track = view
            .refs
            .get(row_index)
            .cloned()
            .ok_or_else(|| "当前行不是歌曲".to_owned())?;
        match kind {
            BrowseKind::Queue => {
                player
                    .queue_remove(row_index)
                    .map_err(|error| error.message.clone())?;
                self.refresh_queue_view(player, library);
                self.set_transient_status("已从队列移除");
            }
            BrowseKind::Playlist(name) => {
                let playlist = library
                    .playlist_by_name(&name)
                    .map_err(|error| error.message.clone())?
                    .ok_or_else(|| format!("歌单不存在：{name}"))?;
                library
                    .playlist_remove(playlist.id, i64::try_from(row_index).unwrap_or(i64::MAX))
                    .map_err(|error| error.message.clone())?;
                if let Some(index) = self
                    .view_kind
                    .iter()
                    .position(|kind| matches!(kind, BrowseKind::Playlist(title) if title == &name))
                {
                    self.views[index] = playlist_child_view(library, &name);
                }
                self.set_transient_status(format!("已从歌单移除：{name}"));
            }
            BrowseKind::Library | BrowseKind::Search => {
                let title = view
                    .rows
                    .get(row_index)
                    .and_then(|row| row.get(1))
                    .cloned()
                    .unwrap_or_default();
                let artist = view
                    .rows
                    .get(row_index)
                    .and_then(|row| row.get(2))
                    .cloned()
                    .unwrap_or_default();
                let position = player.queue_items_snapshot().iter().position(|item| {
                    item.track == track
                        || (!title.is_empty() && item.title == title && item.artist == artist)
                });
                let Some(position) = position else {
                    return Err("该歌曲不在播放队列中".to_owned());
                };
                player
                    .queue_remove(position)
                    .map_err(|error| error.message.clone())?;
                self.refresh_queue_view(player, library);
                self.set_transient_status("已从播放队列移除");
            }
            _ => return Err("当前列表不支持移除".to_owned()),
        }
        Ok(())
    }

    pub(crate) fn begin_add_to_playlist(&mut self) -> Result<(), String> {
        if self.pending_add.is_some() {
            return Err("已有一首待添加歌曲，请先选择歌单".to_owned());
        }
        let Some(kind) = self.view_kind.get(self.view_index).cloned() else {
            return Err("当前没有列表".to_owned());
        };
        if matches!(kind, BrowseKind::PlaylistRoot | BrowseKind::Settings) {
            return Err("当前列表不是歌曲列表".to_owned());
        }
        let Some(view) = self.selected_view() else {
            return Err("当前没有列表".to_owned());
        };
        let Some(track) = view.refs.get(self.row_index).cloned() else {
            return Err("当前行不是歌曲".to_owned());
        };
        let Some(row) = view.rows.get(self.row_index) else {
            return Err("当前行不是歌曲".to_owned());
        };
        let duration_ms = view
            .durations
            .get(self.row_index)
            .copied()
            .filter(|duration| *duration > 0)
            .or_else(|| row.get(3).and_then(|value| parse_duration(value)))
            .unwrap_or(0);
        let item = QueueItem::new(
            track,
            row.get(1).cloned().unwrap_or_default(),
            row.get(2).cloned().unwrap_or_default(),
            duration_ms,
        );
        let Some(root) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::PlaylistRoot)
        else {
            return Err("没有 Playlist 页面".to_owned());
        };
        self.pending_add = Some(PendingAdd {
            item,
            origin_view: self.view_index,
            origin_row: self.row_index,
            origin_focus: self.focus,
        });
        self.view_index = root;
        self.row_index = 0;
        self.focus = Focus::List;
        self.set_status("选择歌单后按 Enter 添加 · Esc取消".to_owned());
        Ok(())
    }

    pub(crate) fn cancel_pending_add(&mut self) {
        if let Some(pending) = self.pending_add.take() {
            self.view_index = pending.origin_view.min(self.views.len().saturating_sub(1));
            self.row_index = pending.origin_row;
            self.focus = pending.origin_focus;
            self.set_transient_status("已取消添加");
        }
    }

    pub(crate) fn confirm_add_to_playlist(
        &mut self,
        library: &dyn LibraryApi,
    ) -> Result<(), String> {
        let Some(pending) = self.pending_add.take() else {
            return Err("没有待添加歌曲".to_owned());
        };
        let item = pending.item.clone();
        if !matches!(
            self.view_kind.get(self.view_index),
            Some(BrowseKind::PlaylistRoot)
        ) {
            self.pending_add = Some(pending);
            return Err("请选择 Playlist 页中的歌单".to_owned());
        }
        let Some(name) = self
            .filtered_rows()
            .get(self.row_index)
            .and_then(|row| row.get(1).cloned())
        else {
            self.pending_add = Some(pending);
            return Err("没有选中歌单".to_owned());
        };
        let playlist = match library.playlist_by_name(&name) {
            Ok(Some(playlist)) => playlist,
            Ok(None) => {
                self.pending_add = Some(pending);
                return Err(format!("歌单不存在：{name}"));
            }
            Err(error) => {
                self.pending_add = Some(pending);
                return Err(error.message);
            }
        };
        let (kind, path, source, source_id) = match &item.track {
            TrackRef::Local(path) => (
                "local",
                path.to_string_lossy().into_owned(),
                String::new(),
                String::new(),
            ),
            TrackRef::Remote { source, id } => {
                ("remote", String::new(), source.clone(), id.clone())
            }
        };
        let existing = library
            .playlist_get(playlist.id)
            .map(|(_, items)| items)
            .unwrap_or_default();
        let duplicate = existing.iter().any(|existing| {
            if existing.kind == "local" {
                matches!(&item.track, TrackRef::Local(path) if existing.path == path.to_string_lossy())
            } else {
                matches!(
                    &item.track,
                    TrackRef::Remote { source, id }
                        if existing.source == *source && existing.source_id == *id
                )
            }
        });
        if duplicate {
            self.pending_add = Some(pending);
            return Err("歌曲已在歌单中".to_owned());
        }
        let draft = NewPlaylistItem {
            kind: kind.to_owned(),
            path,
            source,
            source_id,
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: String::new(),
            duration_ms: i64::try_from(item.duration_ms).unwrap_or(i64::MAX),
        };
        if let Err(error) = library.playlist_add(playlist.id, &draft) {
            self.pending_add = Some(pending);
            return Err(error.message);
        }
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::PlaylistRoot)
        {
            self.views[index] = playlist_root_view(library);
        }
        if !self.playlist_expanded {
            self.toggle_playlist();
        }
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| matches!(kind, BrowseKind::Playlist(title) if title == &name))
        {
            self.views[index] = playlist_child_view(library, &name);
            self.view_index = index;
            self.row_index = 0;
            self.focus = Focus::List;
        }
        self.set_transient_status(format!("已添加到歌单：{name}"));
        Ok(())
    }

    pub(crate) fn search_editing_input(&self) -> bool {
        self.focus == Focus::List
            && self.search_input_active
            && matches!(self.search_display(), Some(SearchDisplay::Empty { .. }))
    }

    pub(crate) fn begin_search_edit(&mut self) {
        self.search_input_active = true;
        self.search_cursor = self.search_input.chars().count();
    }

    pub(crate) fn end_search_edit(&mut self) {
        self.search_input_active = false;
    }

    pub(crate) fn reset_search_to_empty(&mut self) {
        self.search_id += 1;
        self.search_pending = false;
        self.search_attempted = false;
        self.search_input_active = false;
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::Search)
            && let Some(view) = self.views.get_mut(index)
        {
            view.rows.clear();
            view.columns.clear();
            view.refs.clear();
        }
    }

    pub(crate) fn insert_search_char(&mut self, character: char) {
        let byte = char_to_byte_index(&self.search_input, self.search_cursor);
        self.search_input.insert(byte, character);
        self.search_cursor += 1;
    }

    pub(crate) fn backspace_search_char(&mut self) {
        if self.search_cursor > 0 {
            self.search_cursor -= 1;
            let byte = char_to_byte_index(&self.search_input, self.search_cursor);
            self.search_input.remove(byte);
        }
    }

    pub(crate) fn move_search_cursor(&mut self, right: bool) {
        if right {
            if self.search_cursor < self.search_input.chars().count() {
                self.search_cursor += 1;
            }
        } else {
            self.search_cursor = self.search_cursor.saturating_sub(1);
        }
    }

    pub(crate) fn play_online_search_result(
        &mut self,
        index: usize,
        library: &Arc<LibraryHandle>,
        tx: &std::sync::mpsc::Sender<(u64, Block)>,
        player: &TuiAudio,
    ) -> Result<(), String> {
        let hits = library.search_hits();
        let Some(SearchHit::Online(track)) = hits.get(index) else {
            return Err("该搜索结果不是在线曲目".to_owned());
        };
        player.set_pending_metadata(track.title.clone(), track.artist.clone(), track.duration_ms);
        let remote_tx = self.remote_tx.clone().ok_or("远程播放通道未就绪")?;
        self.search_id += 1;
        let search_id = self.search_id;
        let block_id = format!("tui-online-{}", now_millis());
        let input = format!("play {}", index + 1);
        let template = online_play_started_block(&block_id, &input, index, track);
        self.upsert_block(template.clone());
        self.set_status("准备中…（下载后自动播放）".to_owned());
        let online = library.online();
        let tx = tx.clone();
        let target = RemotePlayback { tx: remote_tx };
        let track = track.clone();
        std::thread::spawn(move || {
            let emit = |block: &Block| {
                let _ = tx.send((search_id, block.clone()));
            };
            run_online_play(
                &track,
                index,
                &*online,
                &target,
                &emit,
                &template,
                &np_app::now_millis,
            );
        });
        Ok(())
    }

    pub(crate) fn start_search_draft(
        &mut self,
        draft: SearchDraft,
        library: &Arc<LibraryHandle>,
        tx: &std::sync::mpsc::Sender<(u64, Block)>,
    ) {
        // 关键词里的 `-source` 优先；没有写时沿用 Tab 选中的平台。
        let source = draft.source.or_else(|| self.search_platform.source());
        if let Some(source) = draft.source {
            self.search_platform = SearchPlatform::from_source(Some(source));
        }
        let options = np_app::SearchOptions {
            keyword: draft.keyword,
            artist: draft.artist,
            source,
            limit: np_app::DEFAULT_SEARCH_RESULTS,
            local: false,
            explicit_limit: false,
            has_keyword: true,
            up: None,
            mid: None,
            no_filter: false,
        };
        self.start_online_search(options, library, tx);
    }

    pub(crate) fn start_online_search(
        &mut self,
        options: np_app::SearchOptions,
        library: &Arc<LibraryHandle>,
        tx: &std::sync::mpsc::Sender<(u64, Block)>,
    ) {
        self.search_id += 1;
        self.search_pending = true;
        self.search_query = options.keyword.clone();
        self.search_attempted = true;
        let search_id = self.search_id;
        let block_id = format!("tui-search-{}", now_millis());
        let template = if options.up.is_some() || options.mid.is_some() {
            np_app::up_started_block(
                &block_id,
                &options.keyword,
                options.up.as_deref().unwrap_or("mid"),
            )
        } else {
            np_app::search_started_block(&block_id, &options.keyword, &options.keyword)
        };
        self.upsert_block(template.clone());
        self.set_status("搜索中…（输入行仍可编辑）".to_owned());
        let online = library.online();
        let library = Arc::clone(library);
        let tx = tx.clone();
        std::thread::spawn(move || {
            let emit = move |block: &Block| {
                let _ = tx.send((search_id, block.clone()));
            };
            if options.up.is_some() || options.mid.is_some() {
                np_app::run_up_search(
                    &options,
                    &*online,
                    &*library,
                    &emit,
                    &template,
                    &np_app::now_millis,
                );
            } else {
                np_app::run_online_search_with_options(
                    &options,
                    &*online,
                    &*library,
                    &emit,
                    &template,
                    &np_app::now_millis,
                );
            }
        });
    }

    pub(crate) fn start_playlist_prepare(
        &mut self,
        name: &str,
        index: usize,
        library: &Arc<LibraryHandle>,
    ) -> Result<(), String> {
        self.playlist_generation += 1;
        let generation = self.playlist_generation;
        let tx = self.playlist_prepare_tx.clone();
        let name = name.to_owned();
        let library = Arc::clone(library);
        self.set_status("准备中…".to_owned());
        std::thread::spawn(move || {
            let result = prepare_playlist(&name, index, &library);
            let _ = tx.send(PlaylistReady { generation, result });
        });
        Ok(())
    }

    pub(crate) fn poll_playlist_prepare(
        &mut self,
        player: &TuiAudio,
        library: &dyn LibraryApi,
    ) -> bool {
        let mut changed = false;
        while let Ok(ready) = self.playlist_prepare_rx.try_recv() {
            changed = true;
            if ready.generation != self.playlist_generation {
                continue;
            }
            match ready.result {
                Ok((items, selected)) => match player.replace_playlist(items, selected.as_ref()) {
                    Ok(_) => self.set_transient_status("播放中"),
                    Err(error) => self.set_status(error.message),
                },
                Err(error) => self.set_status(error),
            }
            self.refresh_queue_view(player, library);
        }
        changed
    }

    pub(crate) fn refresh_search_refs(&mut self, library: &dyn LibraryApi) {
        let hits = library.search_hits();
        let refs = hits
            .iter()
            .map(|hit| match hit {
                SearchHit::Local(track) => TrackRef::Local(PathBuf::from(&track.path)),
                SearchHit::Online(track) => TrackRef::Remote {
                    source: track.source.as_str().to_owned(),
                    id: track.source_id.clone(),
                },
            })
            .collect();
        let durations: Vec<u64> = hits
            .iter()
            .map(|hit| match hit {
                SearchHit::Local(track) => u64::try_from(track.duration_ms).unwrap_or(0),
                SearchHit::Online(track) => track.duration_ms,
            })
            .collect();
        let rows = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let (title, artist, platform) = match hit {
                    SearchHit::Local(track) => (
                        track.title.clone(),
                        track.artist.clone(),
                        "local".to_owned(),
                    ),
                    SearchHit::Online(track) => (
                        track.title.clone(),
                        track.artist.clone(),
                        track.source.as_str().to_owned(),
                    ),
                };
                vec![(index + 1).to_string(), title, artist, platform]
            })
            .collect();
        if let Some(index) = self
            .view_kind
            .iter()
            .position(|kind| *kind == BrowseKind::Search)
            && let Some(view) = self.views.get_mut(index)
        {
            view.columns = vec![
                "序号".to_owned(),
                "标题".to_owned(),
                "艺人".to_owned(),
                "平台".to_owned(),
            ];
            view.rows = rows;
            view.refs = refs;
            view.durations = durations;
        }
    }

    pub(crate) fn move_view(&mut self, down: bool) {
        if self.views.is_empty() {
            return;
        }
        self.view_index = if down {
            (self.view_index + 1) % self.views.len()
        } else {
            (self.view_index + self.views.len() - 1) % self.views.len()
        };
        self.row_index = 0;
    }

    pub(crate) fn move_row(&mut self, down: bool) {
        let len = self.filtered_row_count();
        if len == 0 {
            return;
        }
        self.row_index = if down {
            (self.row_index + 1).min(len - 1)
        } else {
            self.row_index.saturating_sub(1)
        };
    }

    pub(crate) fn enter_command(&mut self) {
        self.focus = Focus::Controller;
        self.input.clear();
        self.cursor = 0;
        self.completion = None;
    }

    pub(crate) fn leave_command(&mut self) {
        self.focus = Focus::List;
        self.input.clear();
        self.cursor = 0;
        self.completion = None;
    }

    pub(crate) fn refresh_completion(&mut self, player: &dyn PlayerApi) -> bool {
        if self.input.is_empty() {
            self.completion = None;
            return false;
        }
        if self.history_nav {
            return self.completion.take().is_some();
        }
        if self.last_edit.elapsed() < std::time::Duration::from_millis(80) {
            return false;
        }
        let completion = complete_for(player, &self.input_text(), self.cursor);
        let changed = self.completion.as_ref().is_none_or(|old| {
            old.items.len() != completion.items.len()
                || old
                    .items
                    .iter()
                    .zip(&completion.items)
                    .any(|(left, right)| left.text != right.text)
        }) && !completion.items.is_empty();
        self.completion = if completion.items.is_empty() {
            None
        } else {
            Some(completion)
        };
        if changed {
            self.completion_index = 0;
        } else if let Some(completion) = &self.completion {
            self.completion_index = self.completion_index.min(completion.items.len() - 1);
        }
        changed
    }

    pub(crate) fn move_completion(&mut self, down: bool) -> bool {
        let Some(completion) = &self.completion else {
            return false;
        };
        let len = completion.items.len();
        if len == 0 {
            return false;
        }
        if down {
            self.completion_index = (self.completion_index + 1) % len;
        } else {
            self.completion_index = (self.completion_index + len - 1) % len;
        }
        true
    }

    pub(crate) fn accept_completion(&mut self) -> bool {
        let Some(completion) = self.completion.take() else {
            return false;
        };
        let Some(item) = completion.items.get(self.completion_index) else {
            return false;
        };
        let start = completion.replace_start.min(self.input.len());
        let end = completion.replace_end.min(self.input.len());
        let replacement = if item.kind == np_core::CompletionKind::Command {
            format!("{} ", item.text.trim_end())
        } else {
            item.text.clone()
        };
        self.input.splice(start..end, replacement.chars());
        self.cursor = start + replacement.chars().count();
        self.history_nav = false;
        self.last_edit = std::time::Instant::now();
        true
    }

    pub(crate) fn upsert_block(&mut self, block: Block) {
        if let Some(existing) = self
            .blocks
            .iter_mut()
            .find(|existing| existing.id == block.id)
        {
            *existing = block;
        } else {
            self.blocks.push(block);
        }
    }

    pub(crate) fn run(
        &mut self,
        input: String,
        player: &TuiAudio,
        library: &Arc<LibraryHandle>,
        tx: &std::sync::mpsc::Sender<(u64, Block)>,
        theme: &dyn ThemeApi,
        settings: &SettingsHandle,
    ) {
        if input.trim().is_empty() {
            return;
        }
        self.history.push(input.clone());
        self.input.clear();
        self.cursor = 0;
        self.history_cursor = None;
        self.set_status(format!("执行：{input}"));
        if let Some(status) = crate::live_command::start_live_command(&input, player) {
            self.set_status(status);
            return;
        }
        if let Some(status) = crate::login_command::run(&input, player) {
            self.set_status(status);
            return;
        }
        let block_id = format!("tui-{}", now_millis());
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "search"
            && let Ok(options) = np_app::SearchOptions::parse(&parsed)
        {
            self.search_query = options.keyword.clone();
            self.search_attempted = true;
        }

        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "search"
            && let Ok(options) = np_app::SearchOptions::parse(&parsed)
            && !options.local
        {
            self.search_id += 1;
            self.search_pending = true;
            let search_id = self.search_id;
            let template = if options.up.is_some() || options.mid.is_some() {
                np_app::up_started_block(&block_id, &input, options.up.as_deref().unwrap_or("mid"))
            } else {
                np_app::search_started_block(&block_id, &input, &options.keyword)
            };
            self.upsert_block(template.clone());
            self.set_status("搜索中…（输入行仍可编辑）".to_owned());
            let online = library.online();
            let library = Arc::clone(library);
            let tx = tx.clone();
            std::thread::spawn(move || {
                let emit = move |block: &Block| {
                    let _ = tx.send((search_id, block.clone()));
                };
                if options.up.is_some() || options.mid.is_some() {
                    np_app::run_up_search(
                        &options,
                        &*online,
                        &*library,
                        &emit,
                        &template,
                        &np_app::now_millis,
                    );
                } else {
                    np_app::run_online_search_with_options(
                        &options,
                        &*online,
                        &*library,
                        &emit,
                        &template,
                        &np_app::now_millis,
                    );
                }
            });
            return;
        }

        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && let Some((index, track)) = online_play_hit(
                &parsed,
                library.active_listing().as_ref(),
                &library.search_hits(),
            )
            && let Some(remote_tx) = self.remote_tx.clone()
        {
            self.search_id += 1;
            let search_id = self.search_id;
            let template = online_play_started_block(&block_id, &input, index, &track);
            self.upsert_block(template.clone());
            self.set_status("准备中…（下载后自动播放）".to_owned());
            let online = library.online();
            let tx = tx.clone();
            let target = RemotePlayback { tx: remote_tx };
            std::thread::spawn(move || {
                let emit = |block: &Block| {
                    let _ = tx.send((search_id, block.clone()));
                };
                run_online_play(
                    &track,
                    index,
                    &*online,
                    &target,
                    &emit,
                    &template,
                    &np_app::now_millis,
                );
            });
            return;
        }

        let online = library.online();
        let execution = execute_with_services(
            player,
            &**library,
            Services {
                settings: Some(settings),
                theme: Some(theme),
                online: Some(&*online),
            },
            &block_id,
            &input,
            Arc::new(NoopLibraryObserver),
        );
        match execution {
            Execution::Ignored => {}
            Execution::Blocks { done, .. } => {
                let text = format!(
                    "{} · {}ms",
                    done.status_label(),
                    done.duration_ms.unwrap_or(0)
                );
                match done.status {
                    BlockStatus::Done | BlockStatus::Cancelled => self.set_transient_status(text),
                    _ => self.set_status(text),
                }
                self.upsert_block(*done);
            }
            Execution::Accepted { running } => {
                self.set_status("后台任务已受理".to_owned());
                self.upsert_block(*running);
            }
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "search"
        {
            self.search_pending = false;
            self.search_attempted = true;
            self.refresh_search_refs(&**library);
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input) {
            if parsed.spec.name == "queue"
                && let Some(index) = self
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::Queue)
            {
                self.views[index] = queue_view(player, &**library);
            } else if parsed.spec.name == "playlist"
                && let Some(index) = self
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::PlaylistRoot)
            {
                self.views[index] = playlist_root_view(&**library);
            }
            self.refresh_queue_view(player, &**library);
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "config"
            && let Some(enabled) = settings.get_bool("startup.enabled")
            && let Err(error) = startup::set_enabled(enabled)
        {
            self.set_status(format!("自启设置失败：{error}"));
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "theme"
            && parsed.subcommand.map(|sub| sub.name) == Some("set")
            && let Some(id) = settings.theme_id()
        {
            self.load_theme(theme, &id);
        }
        self.refresh_settings_view(settings);
    }
}

fn library_view(library: &dyn LibraryApi) -> ViewData {
    let tracks = library.list_recent(200).unwrap_or_default();
    let refs = tracks
        .iter()
        .map(|track| {
            if track.kind == "local" {
                TrackRef::Local(PathBuf::from(&track.path))
            } else {
                TrackRef::Remote {
                    source: track.source.clone(),
                    id: track.source_id.clone(),
                }
            }
        })
        .collect();
    let durations: Vec<u64> = tracks
        .iter()
        .map(|track| u64::try_from(track.duration_ms).unwrap_or(0))
        .collect();
    let rows = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| {
            vec![
                (index + 1).to_string(),
                track.title.clone(),
                track.artist.clone(),
                crate::player_bar::format_time(u64::try_from(track.duration_ms).unwrap_or(0)),
            ]
        })
        .collect();
    ViewData {
        title: "媒体库".to_owned(),
        columns: vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ],
        rows,
        refs,
        durations,
    }
}

fn queue_view(player: &TuiAudio, _library: &dyn LibraryApi) -> ViewData {
    let items = player.queue_items_snapshot();
    let refs = items.iter().map(|item| item.track.clone()).collect();
    let rows = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            vec![
                (index + 1).to_string(),
                item.title.clone(),
                item.artist.clone(),
                crate::player_bar::format_time(item.duration_ms),
            ]
        })
        .collect();
    let durations = items.iter().map(|item| item.duration_ms).collect();
    ViewData {
        title: "队列".to_owned(),
        columns: vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ],
        rows,
        refs,
        durations,
    }
}

fn playlist_child_view(library: &dyn LibraryApi, name: &str) -> ViewData {
    let items = library
        .playlist_by_name(name)
        .ok()
        .flatten()
        .and_then(|playlist| library.playlist_get(playlist.id).ok())
        .map(|(_, items)| items)
        .unwrap_or_default();
    let refs = items
        .iter()
        .map(|item| {
            if item.kind == "local" {
                TrackRef::Local(PathBuf::from(&item.path))
            } else {
                TrackRef::Remote {
                    source: item.source.clone(),
                    id: item.source_id.clone(),
                }
            }
        })
        .collect();
    let durations: Vec<u64> = items
        .iter()
        .map(|item| u64::try_from(item.duration_ms).unwrap_or(0))
        .collect();
    let rows = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            vec![
                (index + 1).to_string(),
                item.title.clone(),
                item.artist.clone(),
                crate::player_bar::format_time(u64::try_from(item.duration_ms).unwrap_or(0)),
            ]
        })
        .collect();
    ViewData {
        title: name.to_owned(),
        columns: vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ],
        rows,
        refs,
        durations,
    }
}

fn prepare_playlist(
    name: &str,
    index: usize,
    library: &LibraryHandle,
) -> Result<(Vec<QueueItem>, Option<TrackKey>), String> {
    let playlist = library
        .playlist_by_name(name)
        .map_err(|error| error.message.clone())?
        .ok_or_else(|| format!("歌单不存在：{name}"))?;
    let (_, items) = library
        .playlist_get(playlist.id)
        .map_err(|error| error.message.clone())?;
    if items.is_empty() {
        return Err(format!("歌单《{name}》为空"));
    }
    let online = library.online();
    let mut queue_items = Vec::with_capacity(items.len());
    let mut selected = None;
    for (item_index, item) in items.iter().enumerate() {
        let path = if item.kind == "local" {
            let path = PathBuf::from(&item.path);
            if !path.is_file() {
                return Err(format!("文件不存在：{}", item.path));
            }
            path
        } else {
            let source = match item.source.as_str() {
                "bilibili" => SourceId::Bilibili,
                "netease" => SourceId::Netease,
                "ytdlp" => SourceId::Ytdlp,
                other => return Err(format!("未知音源：{other}")),
            };
            let track = SourceTrack {
                source,
                source_id: item.source_id.clone(),
                title: item.title.clone(),
                artist: item.artist.clone(),
                duration_ms: u64::try_from(item.duration_ms).unwrap_or(0),
                album: (!item.album.is_empty()).then(|| item.album.clone()),
                cover_url: None,
            };
            online
                .prepare(&track, &mut |_| {})
                .map_err(|error| error.message.clone())?
                .path
        };
        let key = TrackKey::Local(path.clone());
        if item_index == index {
            selected = Some(key.clone());
        }
        queue_items.push(QueueItem::new(
            TrackRef::Local(path),
            item.title.clone(),
            item.artist.clone(),
            u64::try_from(item.duration_ms).unwrap_or(0),
        ));
    }
    Ok((queue_items, selected))
}

fn parse_duration(text: &str) -> Option<u64> {
    let mut parts = text.split(':');
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?.parse::<u64>().ok()?;
    Some((minutes * 60 + seconds) * 1000)
}

fn playlist_root_view(library: &dyn LibraryApi) -> ViewData {
    let rows = library
        .playlist_list()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, playlist)| {
            vec![
                (index + 1).to_string(),
                playlist.name,
                format_date(playlist.created_at),
                playlist.item_count.to_string(),
            ]
        })
        .collect();
    ViewData {
        title: "歌单".to_owned(),
        columns: vec![
            "序号".to_owned(),
            "名称".to_owned(),
            "创建时间".to_owned(),
            "数量".to_owned(),
        ],
        rows,
        refs: Vec::new(),
        durations: Vec::new(),
    }
}

fn char_to_byte_index(input: &str, char_index: usize) -> usize {
    input
        .char_indices()
        .nth(char_index)
        .map_or(input.len(), |(byte, _)| byte)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchDraft {
    pub(crate) keyword: String,
    pub(crate) artist: Option<String>,
    pub(crate) source: Option<SourceId>,
}

pub(crate) fn parse_search_input(input: &str) -> Result<SearchDraft, String> {
    let tokens = split_search_args(input)?;
    let mut keyword = Vec::new();
    let mut artist: Option<String> = None;
    let mut source: Option<String> = None;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "-artist" | "--artist" | "-a" => {
                index += 1;
                let value = tokens.get(index).ok_or("参数缺少作者值")?;
                artist = Some(value.clone());
            }
            "-source" | "--source" | "-s" => {
                index += 1;
                let value = tokens.get(index).ok_or("参数缺少来源平台值")?;
                source = Some(value.clone());
            }
            token if token.starts_with('-') => {
                return Err(format!("未知参数：{token}"));
            }
            token => keyword.push(token.to_owned()),
        }
        index += 1;
    }
    if keyword.is_empty() {
        return Err("请输入关键词".to_owned());
    }
    let source = match source.as_deref() {
        None | Some("all") => None,
        Some("bilibili") => Some(SourceId::Bilibili),
        Some("netease") => Some(SourceId::Netease),
        Some(other) => return Err(format!("未知音源：{other}")),
    };
    Ok(SearchDraft {
        keyword: keyword.join(" "),
        artist,
        source,
    })
}

fn split_search_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = false;
    for character in input.chars() {
        match character {
            '"' => quote = !quote,
            ' ' | '\t' if !quote => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if quote {
        return Err("引号未闭合".to_owned());
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

fn format_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}.{month:02}.{day:02}")
}
