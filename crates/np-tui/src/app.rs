pub(crate) use crate::model::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::audio::TuiAudio;
use crate::theme::Palette;
#[cfg(windows)]
use crate::tray;
use crate::{osc, startup};
use np_app::{
    Execution, LibraryApi, LibraryHandle, NoopLibraryObserver, PlayerApi, SearchHit, Services,
    SettingsHandle, ThemeApi, complete_for, execute_with_services, online_play_hit,
    online_play_started_block, run_online_play,
};
use np_core::{Block, BlockStatus, Output};
pub(crate) struct App {
    pub(crate) input: Vec<char>,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) status: String,
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
    pub(crate) no_color_ignored: bool,
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
}

impl App {
    pub(crate) fn new() -> Self {
        Self {
            input: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            blocks: Vec::new(),
            status: "就绪".to_owned(),
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
            no_color_ignored: false,
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
        }
    }

    pub(crate) fn input_text(&self) -> String {
        self.input.iter().collect()
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
            self.status = format!("主题告警：{warning}");
        }
        osc::set_background(self.palette.bg);
        self.theme_id = reply.id;
        self.theme_name = reply.name;
        if let Some(warning) = reply.warnings.first() {
            self.status = format!("主题告警：{warning}");
        }
    }

    pub(crate) fn load_theme(&mut self, theme: &dyn ThemeApi, id: &str) {
        match theme.apply(id) {
            Ok(reply) => self.apply_theme_reply(reply),
            Err(error) => {
                if let Some(theme) = np_core::theme::BuiltinTheme::find("milkgreen") {
                    self.apply_theme_reply(np_core::theme::ThemeApplyReply::from_builtin(theme));
                    self.status = format!("主题加载失败：{}，已回退 MilkGreen", error.message);
                } else {
                    self.palette = Palette::default_dark();
                    self.theme_id = "milkgreen".to_owned();
                    self.theme_name = "MilkGreen".to_owned();
                    self.status = format!("主题加载失败：{}", error.message);
                }
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn poll_tray(&mut self, player: &mut TuiAudio) -> bool {
        let Some(rx) = &self.tray_rx else {
            return false;
        };
        while let Ok(action) = rx.try_recv() {
            match action {
                tray::TrayAction::Show => tray::focus_console(),
                tray::TrayAction::Toggle => {
                    player.toggle();
                    self.status = if player.is_playing() {
                        "托盘：继续播放".to_owned()
                    } else {
                        "托盘：已暂停".to_owned()
                    };
                }
                tray::TrayAction::Next => match player.next() {
                    Ok(_) => self.status = "媒体键：下一首".to_owned(),
                    Err(error) => self.status = format!("下一首失败：{}", error.message),
                },
                tray::TrayAction::Prev => match player.prev() {
                    Ok(_) => self.status = "媒体键：上一首".to_owned(),
                    Err(error) => self.status = format!("上一首失败：{}", error.message),
                },
                tray::TrayAction::HotkeyFailed => {
                    self.status = "全局热键注册失败（可能被占用）".to_owned();
                }
                tray::TrayAction::Quit => return true,
            }
        }
        false
    }

    pub(crate) fn load_views(
        &mut self,
        player: &dyn PlayerApi,
        library: &dyn LibraryApi,
        settings: &SettingsHandle,
    ) {
        let playlist_root = capture_table(player, library, "playlist list", "歌单");
        let library_view = capture_table(player, library, "library list --limit=200", "媒体库");
        let queue = capture_table(player, library, "queue list", "队列");
        let search = ViewData {
            title: "搜索".to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            refs: Vec::new(),
        };
        let settings_view = ViewData {
            title: "设置".to_owned(),
            columns: vec!["设置项".to_owned(), "值".to_owned()],
            rows: settings
                .entries()
                .into_iter()
                .map(|(key, value)| vec![key, value.display()])
                .collect(),
            refs: Vec::new(),
        };
        let playlists = library.playlist_list().unwrap_or_default();
        let playlist_children: Vec<ViewData> = playlists
            .iter()
            .map(|playlist| {
                let command = format!("playlist show \"{}\"", playlist.name.replace('"', "\\\""));
                let mut view = capture_table(player, library, &command, &playlist.name);
                if let Ok(Some(playlist)) = library.playlist_by_name(&playlist.name)
                    && let Ok((_, items)) = library.playlist_get(playlist.id)
                {
                    view.refs = items
                        .into_iter()
                        .map(|item| {
                            if item.kind == "local" {
                                TrackRef::Local(PathBuf::from(item.path))
                            } else {
                                TrackRef::Remote {
                                    source: item.source,
                                    id: item.source_id,
                                }
                            }
                        })
                        .collect();
                }
                view
            })
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
        let rows = hits
            .iter()
            .enumerate()
            .map(|(index, hit)| {
                let (title, artist, duration) = match hit {
                    SearchHit::Local(track) => (
                        track.title.clone(),
                        track.artist.clone(),
                        u64::try_from(track.duration_ms).unwrap_or(0),
                    ),
                    SearchHit::Online(track) => {
                        (track.title.clone(), track.artist.clone(), track.duration_ms)
                    }
                };
                vec![
                    (index + 1).to_string(),
                    title,
                    artist,
                    crate::player_bar::format_time(duration),
                ]
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
                "时长".to_owned(),
            ];
            view.rows = rows;
            view.refs = refs;
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
        player: &dyn PlayerApi,
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
        self.status = format!("执行：{input}");
        let block_id = format!("tui-{}", now_millis());

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
            self.status = "搜索中…（输入行仍可编辑）".to_owned();
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
            self.status = "准备中…（下载后自动播放）".to_owned();
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
                self.status = format!(
                    "{} · {}ms",
                    done.status_label(),
                    done.duration_ms.unwrap_or(0)
                );
                self.upsert_block(*done);
            }
            Execution::Accepted { running } => {
                self.status = "后台任务已受理".to_owned();
                self.upsert_block(*running);
            }
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input) {
            if parsed.spec.name == "queue"
                && let Some(index) = self
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::Queue)
            {
                self.views[index] = capture_table(player, &**library, "queue list", "队列");
            } else if parsed.spec.name == "playlist"
                && let Some(index) = self
                    .view_kind
                    .iter()
                    .position(|kind| *kind == BrowseKind::PlaylistRoot)
            {
                self.views[index] = capture_table(player, &**library, "playlist list", "歌单");
            }
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "config"
            && let Some(enabled) = settings.get_bool("startup.enabled")
            && let Err(error) = startup::set_enabled(enabled)
        {
            self.status = format!("自启设置失败：{error}");
        }
        if let Ok(Some(parsed)) = np_core::command::parse(&input)
            && parsed.spec.name == "theme"
            && parsed.subcommand.map(|sub| sub.name) == Some("set")
            && let Some(id) = settings.theme_id()
        {
            self.load_theme(theme, &id);
        }
    }
}

impl BlockStatusExt for Block {
    fn status_label(&self) -> &'static str {
        match self.status {
            BlockStatus::Pending => "等待",
            BlockStatus::Running => "执行中",
            BlockStatus::Done => "完成",
            BlockStatus::Error => "失败",
            BlockStatus::Cancelled => "取消",
        }
    }
}

trait BlockStatusExt {
    fn status_label(&self) -> &'static str;
}
pub(crate) fn capture_table(
    player: &dyn PlayerApi,
    library: &dyn LibraryApi,
    command: &str,
    title: &str,
) -> ViewData {
    let execution = execute_with_services(
        player,
        library,
        Services::default(),
        &format!("view-{}", now_millis()),
        command,
        Arc::new(NoopLibraryObserver),
    );
    let mut view = ViewData {
        title: title.to_owned(),
        columns: Vec::new(),
        rows: Vec::new(),
        refs: Vec::new(),
    };
    if let Execution::Blocks { done, .. } = execution {
        for output in done.outputs {
            if let Output::Table { columns, rows, .. } = output {
                view.columns = columns;
                view.rows = rows;
                break;
            }
        }
    }
    if command.starts_with("library") {
        view.refs = library
            .library_listing()
            .into_iter()
            .map(|track| {
                if track.kind == "local" {
                    TrackRef::Local(PathBuf::from(track.path))
                } else {
                    TrackRef::Remote {
                        source: track.source,
                        id: track.source_id,
                    }
                }
            })
            .collect();
    } else if command.starts_with("queue")
        && let Ok(queue) = player.queue_list()
    {
        view.refs = queue
            .tracks
            .into_iter()
            .map(|path| TrackRef::Local(PathBuf::from(path)))
            .collect();
    }
    view
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
