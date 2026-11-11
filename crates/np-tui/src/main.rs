//! T-0060 np-tui 骨架：输入行 + 执行 np-app 命令 + 块输出。
//!
//! 这一版只覆盖任务卡的 1/4/5：输入、执行、text/table/error/hint。
//! 在线搜索异步分派、补全、播放器面板留给后续卡片。
#![allow(dead_code)]

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

mod audio;
mod cover;
mod fft;
mod glyphs;
mod launcher;
mod osc;
mod theme;
#[cfg(windows)]
mod tray;

use audio::TuiAudio;
use cover::CoverView;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use fft::spectrum;
use np_app::{
    Execution, LibraryApi, LibraryHandle, NoopLibraryObserver, Paths, PlayerApi, Services,
    SettingsHandle, ThemeApi, complete_for, execute_with_services,
};
use np_core::{Block, BlockStatus, Output, Span, SpanColor, Tone};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span as TuiSpan},
    widgets::canvas::{Canvas, Line as CanvasLine},
    widgets::{Block as TuiBlock, Borders, Clear, Paragraph, Wrap},
};
use theme::{Palette, TuiThemeApi};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisualMode {
    Spectrum,
    Waveform,
    Vectorscope,
}

impl VisualMode {
    fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "频谱",
            Self::Waveform => "波形",
            Self::Vectorscope => "矢量",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Spectrum => Self::Waveform,
            Self::Waveform => Self::Vectorscope,
            Self::Vectorscope => Self::Spectrum,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Library,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Normal,
    Filter,
    Command,
    Help,
}

#[derive(Debug, Clone, Default)]
struct ViewData {
    title: String,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

struct App {
    input: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    history_cursor: Option<usize>,
    blocks: Vec<Block>,
    scroll: u16,
    auto_scroll: bool,
    status: String,
    panel_visible: bool,
    immersive: bool,
    spectrum: Vec<f32>,
    waveform: Vec<f32>,
    stereo_left: Vec<f32>,
    stereo_right: Vec<f32>,
    spectrum_generation: u64,
    visual_mode: VisualMode,
    completion: Option<np_core::Completion>,
    completion_index: usize,
    last_edit: std::time::Instant,
    history_nav: bool,
    search_id: u64,
    search_pending: bool,
    no_color_ignored: bool,
    #[cfg(windows)]
    tray_rx: Option<std::sync::mpsc::Receiver<tray::TrayAction>>,
    palette: Palette,
    theme_id: String,
    theme_name: String,
    focus: Focus,
    mode: UiMode,
    filter: String,
    g_pending: Option<std::time::Instant>,
    quit_armed: bool,
    delete_armed: bool,
    views: Vec<ViewData>,
    view_index: usize,
    row_index: usize,
}

impl App {
    fn new() -> Self {
        Self {
            input: Vec::new(),
            cursor: 0,
            history: Vec::new(),
            history_cursor: None,
            blocks: Vec::new(),
            scroll: 0,
            auto_scroll: true,
            status: "就绪".to_owned(),
            panel_visible: true,
            immersive: false,
            spectrum: vec![0.0; 16],
            waveform: vec![0.0; 2048],
            stereo_left: vec![0.0; 2048],
            stereo_right: vec![0.0; 2048],
            spectrum_generation: 0,
            visual_mode: VisualMode::Spectrum,
            completion: None,
            completion_index: 0,
            last_edit: std::time::Instant::now(),
            history_nav: false,
            search_id: 0,
            search_pending: false,
            no_color_ignored: false,
            #[cfg(windows)]
            tray_rx: None,
            palette: Palette::terminal(),
            theme_id: "terminal".to_owned(),
            theme_name: "跟随终端".to_owned(),
            focus: Focus::Library,
            mode: UiMode::Normal,
            filter: String::new(),
            g_pending: None,
            quit_armed: false,
            delete_armed: false,
            views: Vec::new(),
            view_index: 0,
            row_index: 0,
        }
    }

    fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    fn insert(&mut self, character: char) {
        self.input.insert(self.cursor, character);
        self.cursor += 1;
        self.history_cursor = None;
        self.history_nav = false;
        self.last_edit = std::time::Instant::now();
        self.completion = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
            self.history_cursor = None;
            self.history_nav = false;
            self.last_edit = std::time::Instant::now();
            self.completion = None;
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = self
            .history_cursor
            .map_or(self.history.len() - 1, |index| index.saturating_sub(1));
        self.history_cursor = Some(next);
        self.history_nav = true;
        self.completion = None;
        self.input = self.history[next].chars().collect();
        self.cursor = self.input.len();
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_cursor = None;
            self.history_nav = false;
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

    fn apply_theme_reply(&mut self, reply: np_core::theme::ThemeApplyReply) {
        if reply.id == "terminal" {
            self.palette = Palette::terminal();
        } else {
            let (palette, warnings) = Palette::from_tokens(&reply.tokens);
            self.palette = palette;
            if let Some(warning) = warnings.first() {
                self.status = format!("主题告警：{warning}");
            }
        }
        osc::set_background(self.palette.bg);
        self.theme_id = reply.id;
        self.theme_name = reply.name;
        if let Some(warning) = reply.warnings.first() {
            self.status = format!("主题告警：{warning}");
        }
    }

    fn load_theme(&mut self, theme: &dyn ThemeApi, id: &str) {
        match theme.apply(id) {
            Ok(reply) => self.apply_theme_reply(reply),
            Err(error) => {
                self.palette = Palette::default_dark();
                self.theme_id = "graphite".to_owned();
                self.theme_name = "石墨".to_owned();
                self.status = format!("主题加载失败：{}，已回退内置 dark", error.message);
            }
        }
    }

    #[cfg(windows)]
    fn poll_tray(&mut self, player: &mut TuiAudio) -> bool {
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
                tray::TrayAction::Next => {
                    self.status = "托盘：队列为空".to_owned();
                }
                tray::TrayAction::HotkeyFailed => {
                    self.status = "全局热键注册失败（可能被占用）".to_owned();
                }
                tray::TrayAction::Quit => return true,
            }
        }
        false
    }

    fn load_views(&mut self, player: &dyn PlayerApi, library: &dyn LibraryApi) {
        self.views = vec![
            capture_table(player, library, "library list --limit=200", "媒体库"),
            capture_table(player, library, "queue list", "队列"),
            capture_table(player, library, "playlist list", "歌单"),
        ];
        self.views.insert(
            1,
            ViewData {
                title: "搜索结果".to_owned(),
                columns: Vec::new(),
                rows: Vec::new(),
            },
        );
        self.view_index = 0;
        self.row_index = 0;
    }

    fn selected_view(&self) -> Option<&ViewData> {
        self.views.get(self.view_index)
    }

    fn filtered_rows(&self) -> Vec<Vec<String>> {
        let Some(view) = self.selected_view() else {
            return Vec::new();
        };
        view.rows
            .iter()
            .filter(|row| filter_matches(row, &self.filter))
            .cloned()
            .collect()
    }

    fn filtered_row_count(&self) -> usize {
        self.filtered_rows().len()
    }

    fn press_g(&mut self, now: std::time::Instant) -> bool {
        let repeated = self
            .g_pending
            .is_some_and(|at| now.duration_since(at) <= std::time::Duration::from_millis(300));
        if repeated {
            self.row_index = 0;
            self.g_pending = None;
        } else {
            self.g_pending = Some(now);
        }
        repeated
    }

    fn move_view(&mut self, down: bool) {
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

    fn move_row(&mut self, down: bool) {
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

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Library => Focus::List,
            Focus::List => Focus::Library,
        };
    }

    fn enter_filter(&mut self) {
        self.mode = UiMode::Filter;
        self.filter.clear();
        self.input.clear();
        self.cursor = 0;
        self.completion = None;
        self.row_index = 0;
    }

    fn leave_filter(&mut self, clear: bool) {
        if clear {
            self.filter.clear();
        }
        self.mode = UiMode::Normal;
        self.input.clear();
        self.cursor = 0;
    }

    fn enter_command(&mut self) {
        self.mode = UiMode::Command;
        self.input.clear();
        self.cursor = 0;
        self.completion = None;
    }

    fn leave_command(&mut self) {
        self.mode = UiMode::Normal;
        self.input.clear();
        self.cursor = 0;
        self.completion = None;
    }

    fn refresh_completion(&mut self, player: &dyn PlayerApi) -> bool {
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

    fn move_completion(&mut self, down: bool) -> bool {
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

    fn accept_completion(&mut self) -> bool {
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

    fn update_search_view(&mut self, block: &Block) {
        if !block.input.starts_with("search") {
            return;
        }
        for output in &block.outputs {
            if let Output::Table { columns, rows, .. } = output {
                if let Some(view) = self.views.get_mut(1) {
                    view.columns = columns.clone();
                    view.rows = rows.clone();
                    self.view_index = 1;
                    self.row_index = 0;
                }
                break;
            }
        }
    }

    fn upsert_block(&mut self, block: Block) {
        if let Some(existing) = self
            .blocks
            .iter_mut()
            .find(|existing| existing.id == block.id)
        {
            *existing = block;
        } else {
            self.blocks.push(block);
        }
        self.auto_scroll = true;
    }

    fn run(
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

        let execution = execute_with_services(
            player,
            &**library,
            Services {
                settings: Some(settings),
                theme: Some(theme),
                ..Services::default()
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
            if parsed.spec.name == "queue" {
                self.views[2] = capture_table(player, &**library, "queue list", "队列");
            } else if parsed.spec.name == "playlist" {
                self.views[3] = capture_table(player, &**library, "playlist list", "歌单");
            }
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn resolve_cover(player: &TuiAudio, library: &Arc<LibraryHandle>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NP_TUI_COVER") {
        return Some(PathBuf::from(path));
    }
    let track = library
        .track_by_path(player.source_path()?.to_str()?)
        .ok()
        .flatten()?;
    let key = track.cover_key?;
    let dir = library.covers_dir();
    let path = dir.join(format!("{key}.jpg"));
    path.is_file().then_some(path)
}

fn tui_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("NP_TUI_DATA_DIR") {
        return PathBuf::from(path);
    }
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let bench = root.join("tmp/bench");
    let copied = root.join("tmp/np-tui-data");
    if bench.join("library.db").is_file() && !copied.join("library.db").is_file() {
        let _ = std::fs::create_dir_all(&copied);
        let _ = std::fs::copy(bench.join("library.db"), copied.join("library.db"));
    }
    if copied.is_dir() {
        copied
    } else {
        std::env::temp_dir().join("nowhere-tui")
    }
}

fn filter_matches(row: &[String], filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty() || row.iter().any(|cell| cell.to_lowercase().contains(&filter))
}

fn capture_table(
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
    view
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !launcher::is_tty() {
        let _ = launcher::relaunch_from_explorer();
        return Ok(());
    }
    let started = std::time::Instant::now();
    let _ = launcher::install_fragment();
    let fallback = std::env::var_os(launcher::FALLBACK_ENV).is_some();
    let data_dir = tui_data_dir();
    let config_dir = data_dir.join("config");
    let cache_dir = data_dir.join("cache");
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&cache_dir)?;
    let settings = SettingsHandle::load(config_dir.clone());
    let theme_api = TuiThemeApi::new(config_dir.join("themes"));
    let library = Arc::new(LibraryHandle::with_paths(Paths {
        config_dir,
        cache_dir,
        data_dir,
    }));
    let (search_tx, search_rx) = std::sync::mpsc::channel();
    // NO_COLOR 是用户级约定。音乐播放器的彩色主题是核心体验，
    // 所以这里选择“忽略但明确告知”，而不是静默强制或退化成全单色。
    let no_color = std::env::var_os("NO_COLOR").is_some();
    if no_color {
        crossterm::style::Colored::set_ansi_color_disabled(false);
    }
    let mut player = TuiAudio::from_env();
    let mut app = App::new();
    app.no_color_ignored = no_color;
    if fallback {
        app.status = "未找到 wt，已用 conhost，封面降级为字符".to_owned();
    } else if let Some(warning) = launcher::font_warning() {
        app.status = warning.to_owned();
    }
    #[cfg(windows)]
    {
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        tray::spawn(tray_tx);
        app.tray_rx = Some(tray_rx);
    }
    let osc_supported = osc::probe();
    osc::flush_input();
    let _osc_guard = osc::guard();
    if let Ok(path) = std::env::var("NP_TUI_OSC_FILE") {
        let _ = std::fs::write(path, if osc_supported { "1" } else { "0" });
    }
    std::panic::set_hook(Box::new(|info| {
        osc::restore();
        eprintln!("np-tui panic: {info}");
    }));
    app.load_theme(
        &theme_api,
        settings.theme_id().as_deref().unwrap_or("terminal"),
    );
    app.load_views(&player, &*library);
    if let Ok(view) = std::env::var("NP_TUI_VIEW") {
        app.view_index = match view.trim().to_ascii_lowercase().as_str() {
            "search" => 1,
            "queue" => 2,
            "playlist" | "playlists" => 3,
            _ => 0,
        };
    }
    if let Ok(mode) = std::env::var("NP_TUI_MODE") {
        if mode.trim() == "help" {
            app.mode = UiMode::Help;
        } else if let Some(filter) = mode.strip_prefix("filter:") {
            app.enter_filter();
            app.input = filter.chars().collect();
            app.cursor = app.input.len();
            app.filter = filter.to_owned();
        }
    }
    if let Ok(mode) = std::env::var("NP_TUI_VISUAL") {
        app.visual_mode = match mode.trim().to_ascii_lowercase().as_str() {
            "waveform" | "wave" => VisualMode::Waveform,
            "vectorscope" | "vector" => VisualMode::Vectorscope,
            _ => VisualMode::Spectrum,
        };
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        SetCursorStyle::SteadyBlock
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let cover_path = resolve_cover(&player, &library);
    let mut cover = cover_path
        .as_deref()
        .map(CoverView::load)
        .unwrap_or_else(CoverView::empty);
    draw_synced(&mut terminal, &mut app, &player, &mut cover)?;
    if let Ok(path) = std::env::var("NP_TUI_STARTUP_FILE") {
        std::fs::write(path, started.elapsed().as_millis().to_string())?;
    }
    let result = run(
        &mut terminal,
        &mut app,
        &mut player,
        &mut cover,
        &library,
        SearchContext {
            tx: &search_tx,
            rx: &search_rx,
        },
        ThemeContext {
            api: &theme_api,
            settings: &settings,
        },
    );
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    osc::restore();
    terminal.show_cursor()?;
    result?;
    Ok(())
}

struct ThemeContext<'a> {
    api: &'a dyn ThemeApi,
    settings: &'a SettingsHandle,
}

struct SearchContext<'a> {
    tx: &'a std::sync::mpsc::Sender<(u64, Block)>,
    rx: &'a std::sync::mpsc::Receiver<(u64, Block)>,
}

fn draw_synced<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    player: &TuiAudio,
    cover: &mut CoverView,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    execute!(io::stdout(), BeginSynchronizedUpdate)?;
    let result = terminal.draw(|frame| draw(frame, app, player, cover));
    let end = execute!(io::stdout(), EndSynchronizedUpdate);
    result?;
    end?;
    Ok(())
}

fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    player: &mut TuiAudio,
    cover: &mut CoverView,
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
                app.status = "搜索完成".to_owned();
                app.search_pending = false;
                draw_needed = true;
            }
        }
        #[cfg(windows)]
        if app.poll_tray(player) {
            return Ok(());
        }
        if app.refresh_completion(player) || cover.poll() {
            draw_needed = true;
        }
        if draw_needed {
            update_spectrum(app, player);
            draw_synced(terminal, app, player, cover)?;
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
                                    if app.focus == Focus::List {
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
                                    app.status = "音量 +5%".to_owned();
                                    draw_needed = true;
                                }
                                KeyCode::Char('-') => {
                                    app.status = "音量 -5%".to_owned();
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

fn update_spectrum(app: &mut App, player: &TuiAudio) {
    let Some(tap) = player.tap() else {
        return;
    };
    if tap.generation() == app.spectrum_generation {
        return;
    }
    let mut samples = vec![0.0f32; 2048];
    let mut left = vec![0.0f32; 2048];
    let mut right = vec![0.0f32; 2048];
    tap.latest(&mut samples);
    tap.latest_stereo(&mut left, &mut right);
    let next = spectrum(&samples, tap.sample_rate(), 16);
    for (old, new) in app.spectrum.iter_mut().zip(next) {
        *old = *old * 0.65 + new * 0.35;
    }
    app.waveform = samples;
    app.stereo_left = left;
    app.stereo_right = right;
    app.spectrum_generation = tap.generation();
}

#[allow(dead_code)]
fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio, cover: &mut CoverView) {
    let full = frame.area();
    if let Some(bg) = app.palette.bg {
        frame.buffer_mut().set_style(full, Style::default().bg(bg));
    } else {
        frame.render_widget(Clear, full);
    }
    // 主题模式自绘 1 格留白；跟随终端模式绝不覆盖用户终端背景。
    let area = if app.palette.bg.is_some() && full.width > 2 && full.height > 2 {
        Rect {
            x: full.x + 1,
            y: full.y + 1,
            width: full.width - 2,
            height: full.height - 2,
        }
    } else {
        full
    };

    let player_height =
        if area.height < 24 { 6 } else { 10 }.min(area.height.saturating_sub(3).max(3));
    let input_height = u16::from(matches!(app.mode, UiMode::Command | UiMode::Filter));
    let body_height = area
        .height
        .saturating_sub(1 + player_height + input_height + 1)
        .max(1);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(body_height),
            Constraint::Length(player_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        TuiSpan::styled(
            " Nowhere Player ",
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw(format!("  {}", app.theme_name)),
    ]))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.fg),
    );
    frame.render_widget(title, rows[0]);

    if area.width < 80 {
        draw_list(frame, rows[1], app);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(20)])
            .split(rows[1]);
        draw_library(frame, columns[0], app);
        draw_list(frame, columns[1], app);
    }

    draw_player_strip(frame, rows[2], app, player, cover);

    if matches!(app.mode, UiMode::Command | UiMode::Filter) {
        let title = if app.mode == UiMode::Filter {
            " 过滤 "
        } else {
            " 命令 "
        };
        let _ = title;
        let input = Paragraph::new(input_line(app)).style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
        frame.render_widget(input, rows[3]);
        frame.set_cursor_position((
            rows[3].x.saturating_add(input_width_before(app) as u16),
            rows[3].y,
        ));
    }

    let hint = match (app.focus, app.mode) {
        (Focus::Library, _) => "j/k 切视图 · Enter 进入列表 · Tab 切焦点 · : 命令 · q 退出",
        (Focus::List, _) => "j/k 移动 · Enter 播放 · Tab 切焦点 · : 命令 · q 退出",
    };
    let status = Paragraph::new(format!(
        " {} · {} · {} · NO_COLOR {}",
        app.selected_view()
            .map(|view| view.title.as_str())
            .unwrap_or("无视图"),
        app.status,
        hint,
        if app.no_color_ignored {
            "已忽略"
        } else {
            "未设置"
        }
    ))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.muted),
    );
    frame.render_widget(status, rows[4]);
    if app.mode == UiMode::Help {
        draw_help(frame, area, app);
    }
}

fn draw_help(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(8).clamp(40, 64);
    let height = 18.min(area.height.saturating_sub(2));
    let overlay = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, overlay);
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.accent))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .title(" 帮助 · 任意键关闭 ");
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);
    let lines = [
        "Space 播放/暂停   n/p 切歌   +/- 音量",
        "Tab 切焦点   j/k 移动   gg/G 首尾",
        "Enter 播放选中   a 入队   d 删除(需 dd)",
        "f 视觉模式   Ctrl+F 沉浸   / 过滤",
        ": 命令   ? 帮助   q 退出(播放中二次确认)",
        "",
        "Filter: Enter 确认 · Esc 清除",
        "Command: Enter 执行 · ↑/↓ 历史 · Esc 取消",
    ]
    .join("\n");
    frame.render_widget(Paragraph::new(lines), inner);
}

fn panel_block(app: &App, title: &str, focused: bool) -> TuiBlock<'static> {
    TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            app.palette.accent
        } else {
            app.palette.faint
        }))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .title(title.to_owned())
}

fn draw_library(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = panel_block(app, " 视图 ", app.focus == Focus::Library);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<Line> = app
        .views
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(index, view)| {
            let selected = index == app.view_index;
            let marker = if selected { glyphs::SELECT } else { " " };
            let style = if selected {
                Style::default()
                    .bg(app.palette.fg)
                    .fg(app.palette.bg.unwrap_or(Color::Reset))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.palette.fg)
            };
            Line::from(TuiSpan::styled(
                truncate_width(&format!("{marker} {}", view.title), inner.width as usize),
                style,
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_list(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let view = app.selected_view();
    let title = format!(
        " {} ",
        view.map(|view| view.title.as_str()).unwrap_or("条目")
    );
    let block = panel_block(app, &title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(view) = view else {
        return;
    };
    if view.columns.is_empty() {
        frame.render_widget(
            Paragraph::new("暂无数据；: 命令执行 library list").style(
                Style::default()
                    .bg(app.palette.bg.unwrap_or(Color::Reset))
                    .fg(app.palette.muted),
            ),
            inner,
        );
        return;
    }
    let rows = app.filtered_rows();
    let mut widths = table_widths(&view.columns, &rows);
    let used: usize = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
    if used < inner.width as usize {
        let flex = view
            .columns
            .iter()
            .position(|column| column == "标题")
            .unwrap_or(1.min(widths.len().saturating_sub(1)));
        if flex < widths.len() {
            widths[flex] += inner.width as usize - used;
        }
    }
    let mut lines = Vec::new();
    lines.push(Line::styled(
        format_row(&view.columns, &widths),
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    ));
    let visible = inner.height.saturating_sub(1) as usize;
    let first = app.row_index.saturating_sub(visible.saturating_sub(1));
    for (index, row) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = app.focus == Focus::List && index == app.row_index;
        let style = if selected {
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(app.palette.fg)
        };
        lines.push(Line::styled(
            truncate_width(&format_row(row, &widths), inner.width as usize),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_player_strip(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    player: &TuiAudio,
    cover: &mut CoverView,
) {
    let block = TuiBlock::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(app.palette.line))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let cover_width = inner.width.min(24);
    let cover_area = Rect {
        x: inner.x,
        y: inner.y,
        width: cover_width,
        height: inner.height,
    };
    if let Some(bg) = app.palette.bg {
        let fill = " ".repeat(cover_area.width as usize);
        let lines = vec![Line::from(fill); cover_area.height as usize];
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(bg)),
            cover_area,
        );
    }
    cover.render(cover_area, frame, app.palette.faint, app.palette.bg);

    let text_x = inner.x + cover_width + 1;
    let text_width = inner.width.saturating_sub(cover_width + 1);
    let mut y = inner.y;
    let title = truncate_width(
        &format!("{} · {}", player.title(), player.artist()),
        text_width as usize,
    );
    frame.render_widget(
        Paragraph::new(Line::from(TuiSpan::styled(
            title,
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect {
            x: text_x,
            y,
            width: text_width,
            height: 1,
        },
    );
    y += 1;
    let duration = player.duration_ms().unwrap_or(0);
    let position = player.position_ms().min(duration);
    let ratio = if duration == 0 {
        0.0
    } else {
        position as f64 / duration as f64
    };
    let bar_width = text_width.saturating_sub(14).max(4) as usize;
    let filled = ((bar_width as f64 * ratio).round() as usize).min(bar_width);
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}  {} / {}",
            glyphs::PROGRESS_DONE.repeat(filled),
            glyphs::PROGRESS_TODO.repeat(bar_width.saturating_sub(filled)),
            format_time(position),
            format_time(duration)
        )),
        Rect {
            x: text_x,
            y,
            width: text_width,
            height: 1,
        },
    );
    y += 1;
    let visual_height = inner.height.saturating_sub(3).min(4);
    if visual_height > 0 && y + visual_height <= inner.y + inner.height {
        draw_visual(
            frame,
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: visual_height,
            },
            app,
            app.palette.accent,
            app.palette.bg,
        );
        y += visual_height;
    }
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(format!(
                "{}   {}   {}   [{}]",
                glyphs::PREV,
                glyphs::PLAY,
                glyphs::NEXT,
                app.visual_mode.label()
            ))
            .style(Style::default().fg(app.palette.accent)),
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: 1,
            },
        );
    }
}

fn draw_legacy(
    frame: &mut ratatui::Frame,
    app: &mut App,
    player: &TuiAudio,
    cover: &mut CoverView,
) {
    let area = frame.area();
    if let Some(bg) = app.palette.bg {
        frame.render_widget(TuiBlock::default().style(Style::default().bg(bg)), area);
    } else {
        frame.render_widget(Clear, area);
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        TuiSpan::styled(
            " Nowhere Player ",
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw("  TUI 骨架"),
    ]))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.fg),
    );
    frame.render_widget(title, rows[0]);

    if app.immersive {
        draw_panel(frame, rows[1], app, player, cover, true);
    } else if app.panel_visible {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(24), Constraint::Length(28)])
            .split(rows[1]);
        draw_output(frame, body[0], app);
        draw_panel(frame, body[1], app, player, cover, false);
    } else {
        draw_output(frame, rows[1], app);
    }

    draw_completion(frame, rows[2], app);

    let input = Paragraph::new(input_line(app))
        .block(
            TuiBlock::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.palette.accent))
                .title(" 输入命令 "),
        )
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    frame.render_widget(input, rows[3]);

    let cursor_x = rows[3]
        .x
        .saturating_add(1)
        .saturating_add(input_width_before(app) as u16);
    let cursor_y = rows[3].y.saturating_add(1);
    frame.set_cursor_position((cursor_x, cursor_y));

    let panel_hint = if app.panel_visible {
        "Ctrl+B 隐藏面板"
    } else {
        "Ctrl+B 显示面板"
    };
    let no_color_notice = if app.no_color_ignored {
        " · NO_COLOR 已忽略"
    } else {
        ""
    };
    let status = Paragraph::new(format!(
        " {} · 主题 {} · 视觉 {} · 输入 {} 字 · ↑↓ 历史 · Enter 执行 · {} · Ctrl+F 沉浸 · Esc 退出{} ",
        app.status,
        app.theme_name,
        app.visual_mode.label(),
        app.input.len(),
        panel_hint,
        no_color_notice
    ))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.muted),
    );
    frame.render_widget(status, rows[4]);
    if app.mode == UiMode::Help {
        draw_help(frame, area, app);
    }
}

fn draw_completion(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.line))
        .title(" 候选 ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(completion) = &app.completion else {
        return;
    };
    let mut lines = Vec::new();
    for (index, item) in completion.items.iter().take(5).enumerate() {
        let selected = index == app.completion_index;
        let marker = if selected { glyphs::SELECT } else { " " };
        let text = format!("{marker} {}  {}", item.text, item.summary);
        let style = if selected {
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.palette.fg)
        };
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&text, inner.width as usize),
            style,
        )));
    }
    if let Some(item) = completion.items.get(app.completion_index) {
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&item.usage, inner.width as usize),
            Style::default().fg(app.palette.muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_output(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let lines = all_lines(app);
    let output_height = area.height.saturating_sub(2);
    let max_scroll = lines.len().saturating_sub(output_height as usize) as u16;
    if app.auto_scroll {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
    }
    let output = Paragraph::new(lines)
        .block(
            TuiBlock::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.palette.line))
                .title(" 输出 "),
        )
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(output, area);
}

fn draw_panel(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    player: &TuiAudio,
    cover: &mut CoverView,
    immersive: bool,
) {
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.line))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .title(format!(" 播放 · {} ", cover.protocol()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 4 || inner.height < 4 {
        return;
    }

    let cover_height = if immersive {
        inner.height.saturating_mul(6) / 10
    } else {
        inner.height.saturating_sub(10).clamp(2, 8)
    };
    let cover_area = Rect {
        x: inner.x,
        y: inner.y,
        width: if immersive {
            inner.width.saturating_mul(40) / 100
        } else {
            inner.width
        },
        height: cover_height,
    };
    // 主题模式下先铺封面区域的底色，避免 Placeholder / Sixel 未就绪时出现黑块。
    if let Some(bg) = app.palette.bg {
        let fill = " ".repeat(cover_area.width as usize);
        let lines = vec![Line::from(fill); cover_area.height as usize];
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(bg)),
            cover_area,
        );
    }
    cover.render(cover_area, frame, app.palette.faint, app.palette.bg);

    let text_x = if immersive {
        cover_area.x + cover_area.width + 2
    } else {
        inner.x
    };
    let text_width = if immersive {
        inner.width.saturating_sub(cover_area.width + 2)
    } else {
        inner.width
    };
    let mut y = if immersive {
        inner.y
    } else {
        inner.y + cover_height
    };
    let title = truncate_width(player.title(), text_width as usize);
    let artist = truncate_width(player.artist(), text_width as usize);
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                title,
                Style::default()
                    .bg(app.palette.bg.unwrap_or(Color::Reset))
                    .fg(app.palette.fg)
                    .add_modifier(Modifier::BOLD),
            ))),
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: 1,
            },
        );
        y += 1;
    }
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                artist,
                Style::default()
                    .bg(app.palette.bg.unwrap_or(Color::Reset))
                    .fg(app.palette.muted),
            ))),
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: 1,
            },
        );
        y += 1;
    }

    let remaining = (inner.y + inner.height).saturating_sub(y);
    let spectrum_height = remaining.saturating_sub(2).min(4);
    if spectrum_height > 0 && text_width > 0 {
        let area = Rect {
            x: text_x,
            y,
            width: text_width,
            height: spectrum_height,
        };
        draw_visual(frame, area, app, app.palette.accent, app.palette.bg);
    }
    y += spectrum_height;

    let duration = player.duration_ms().unwrap_or(0);
    let position = player.position_ms().min(duration);
    let ratio = if duration == 0 {
        0.0
    } else {
        position as f64 / duration as f64
    };
    let bar_width = text_width.saturating_sub(14).max(4) as usize;
    let filled = ((bar_width as f64 * ratio).round() as usize).min(bar_width);
    let progress = format!(
        "{}{}  {} / {}",
        glyphs::PROGRESS_DONE.repeat(filled),
        glyphs::PROGRESS_TODO.repeat(bar_width.saturating_sub(filled)),
        format_time(position),
        format_time(duration)
    );
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                progress,
                Style::default().fg(app.palette.fg),
            ))),
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: 1,
            },
        );
        y += 1;
    }
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                format!("{}   {}   {}", glyphs::PREV, glyphs::PLAY, glyphs::NEXT),
                Style::default().fg(app.palette.accent),
            ))),
            Rect {
                x: text_x,
                y,
                width: text_width,
                height: 1,
            },
        );
    }
}

fn draw_spectrum(
    frame: &mut ratatui::Frame,
    area: Rect,
    values: &[f32],
    accent: Color,
    background: Color,
) {
    let canvas = Canvas::default()
        .x_bounds([0.0, 16.0])
        .y_bounds([0.0, 1.0])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            for index in 0..=64 {
                let position = index as f32 / 64.0 * 15.0;
                let low = position.floor() as usize;
                let high = (low + 1).min(values.len().saturating_sub(1));
                let fraction = position - low as f32;
                let left = values.get(low).copied().unwrap_or(0.0);
                let right = values.get(high).copied().unwrap_or(left);
                let value = (left + (right - left) * fraction).clamp(0.0, 1.0) as f64;
                ctx.draw(&CanvasLine {
                    x1: position as f64,
                    y1: 0.0,
                    x2: position as f64,
                    y2: value,
                    color: accent,
                });
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_visual(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    accent: Color,
    background: Option<Color>,
) {
    let background = background.unwrap_or(Color::Reset);
    match app.visual_mode {
        VisualMode::Spectrum => draw_spectrum(frame, area, &app.spectrum, accent, background),
        VisualMode::Waveform => draw_waveform(frame, area, &app.waveform, accent, background),
        VisualMode::Vectorscope => draw_vectorscope(
            frame,
            area,
            &app.stereo_left,
            &app.stereo_right,
            accent,
            background,
        ),
    }
}

fn draw_vectorscope(
    frame: &mut ratatui::Frame,
    area: Rect,
    left: &[f32],
    right: &[f32],
    accent: Color,
    background: Color,
) {
    let canvas = Canvas::default()
        .x_bounds([-1.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            // 45° reference line; no signal / mono source naturally lies here.
            ctx.draw(&CanvasLine {
                x1: -1.0,
                y1: -1.0,
                x2: 1.0,
                y2: 1.0,
                color: accent,
            });
            let count = left.len().min(right.len());
            if count == 0 {
                return;
            }
            let step = (count / 256).max(1);
            let mut previous: Option<(f64, f64)> = None;
            for index in (0..count).step_by(step) {
                let x = left[index].clamp(-1.0, 1.0) as f64;
                let y = right[index].clamp(-1.0, 1.0) as f64;
                if let Some((x1, y1)) = previous {
                    ctx.draw(&CanvasLine {
                        x1,
                        y1,
                        x2: x,
                        y2: y,
                        color: accent,
                    });
                }
                previous = Some((x, y));
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_waveform(
    frame: &mut ratatui::Frame,
    area: Rect,
    samples: &[f32],
    accent: Color,
    background: Color,
) {
    let canvas = Canvas::default()
        .x_bounds([0.0, 64.0])
        .y_bounds([-1.0, 1.0])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            let count = samples.len().max(2);
            let mut previous: Option<(f64, f64)> = None;
            for index in 0..=64 {
                let sample_index = index * (count - 1) / 64;
                let x = index as f64;
                let y = samples
                    .get(sample_index)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(-1.0, 1.0) as f64;
                if let Some((x1, y1)) = previous {
                    ctx.draw(&CanvasLine {
                        x1,
                        y1,
                        x2: x,
                        y2: y,
                        color: accent,
                    });
                }
                previous = Some((x, y));
            }
        });
    frame.render_widget(canvas, area);
}

fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn truncate_width(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for character in value.chars() {
        let char_width = UnicodeWidthStr::width(character.to_string().as_str());
        if used + char_width + 1 > width {
            break;
        }
        out.push(character);
        used += char_width;
    }
    out.push('…');
    out
}

fn input_width_before(app: &App) -> usize {
    let prefix: String = app.input[..app.cursor].iter().collect();
    UnicodeWidthStr::width("› ") + UnicodeWidthStr::width(prefix.as_str())
}

fn input_line(app: &App) -> Line<'static> {
    let before: String = app.input[..app.cursor].iter().collect();
    let after: String = app.input[app.cursor..].iter().collect();
    let cursor_char = if app.cursor < app.input.len() {
        app.input[app.cursor].to_string()
    } else {
        " ".to_owned()
    };
    Line::from(vec![
        TuiSpan::styled("› ", Style::default().fg(app.palette.accent)),
        TuiSpan::raw(before),
        TuiSpan::styled(
            cursor_char,
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw(after),
    ])
}

fn all_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in &app.blocks {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        if !block.input.is_empty() {
            lines.push(Line::from(vec![
                TuiSpan::styled("› ", Style::default().fg(app.palette.accent)),
                TuiSpan::raw(block.input.clone()),
            ]));
        }
        for output in &block.outputs {
            output_lines(output, &mut lines, &app.palette);
        }
        if let Some(footer) = &block.footer {
            lines.push(Line::styled(
                footer.clone(),
                Style::default().fg(app.palette.muted),
            ));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "输入 help 或 library list --limit=10 开始",
            Style::default().fg(app.palette.muted),
        ));
    }
    lines
}

fn span_style(span: &Span, tone: Option<Tone>, palette_ref: &Palette) -> Style {
    let mut style = Style::default();
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.dim {
        style = style.fg(palette_ref.muted);
    }
    let color = span.color.map(|color| match color {
        SpanColor::Accent => palette_ref.accent,
        SpanColor::Danger => palette_ref.danger,
        SpanColor::Warn => palette_ref.accent,
        SpanColor::Success => palette_ref.fg,
        SpanColor::Muted => palette_ref.muted,
    });
    let tone = tone.map(|tone| match tone {
        Tone::Accent => palette_ref.accent,
        Tone::Danger => palette_ref.danger,
        Tone::Warn => palette_ref.accent,
        Tone::Success => palette_ref.fg,
        Tone::Muted => palette_ref.muted,
    });
    style.fg(color.or(tone).unwrap_or(palette_ref.fg))
}

fn output_lines(output: &Output, lines: &mut Vec<Line<'static>>, palette_ref: &Palette) {
    match output {
        Output::Text { spans, tone } => {
            lines.push(Line::from(
                spans
                    .iter()
                    .map(|span| {
                        TuiSpan::styled(span.text.clone(), span_style(span, *tone, palette_ref))
                    })
                    .collect::<Vec<_>>(),
            ));
        }
        Output::Table { columns, rows, .. } => {
            if columns.is_empty() {
                return;
            }
            let widths = table_widths(columns, rows);
            lines.push(Line::styled(
                format_row(columns, &widths),
                Style::default()
                    .fg(palette_ref.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("  "),
                Style::default().fg(palette_ref.muted),
            ));
            for row in rows {
                lines.push(Line::raw(format_row(row, &widths)));
            }
        }
        Output::Error {
            code,
            message,
            hint,
            ..
        } => {
            lines.push(Line::styled(
                format!("错误 {code} · {message}"),
                Style::default().fg(palette_ref.danger),
            ));
            if let Some(hint) = hint {
                lines.push(Line::styled(
                    hint.clone(),
                    Style::default().fg(palette_ref.muted),
                ));
            }
        }
        Output::Hint { text, suggestion } => {
            let mut line = text.clone();
            if let Some(suggestion) = suggestion {
                line.push_str(" · ");
                line.push_str(suggestion);
            }
            lines.push(Line::styled(line, Style::default().fg(palette_ref.muted)));
        }
        Output::Progress {
            label,
            current,
            total,
            unit,
        } => {
            let percent = if *total == 0 {
                0
            } else {
                current.saturating_mul(100) / total
            };
            lines.push(Line::styled(
                format!("{label}  {percent}%  {current}/{total} {unit}"),
                Style::default().fg(palette_ref.accent),
            ));
        }
        Output::Media { kind, .. } => {
            lines.push(Line::styled(
                format!("[媒体 {kind:?}]"),
                Style::default().fg(palette_ref.muted),
            ));
        }
    }
}

fn table_widths(columns: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let mut widths: Vec<usize> = columns.iter().map(|value| display_width(value)).collect();
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            if index >= widths.len() {
                widths.resize(index + 1, 0);
            }
            widths[index] = widths[index].max(display_width(value));
        }
    }
    widths
}

fn format_row(row: &[String], widths: &[usize]) -> String {
    (0..widths.len())
        .map(|index| {
            let value = row.get(index).map_or("", String::as_str);
            let pad = widths[index].saturating_sub(display_width(value));
            format!("{value}{}", " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_state_handles_cjk_and_movement() {
        let mut app = App::new();
        app.insert('花');
        app.insert('骨');
        app.insert('朵');
        app.left();
        app.insert('X');
        assert_eq!(app.input_text(), "花骨X朵");
        app.backspace();
        assert_eq!(app.input_text(), "花骨朵");
        app.left();
        app.left();
        app.left();
        app.backspace();
        assert_eq!(app.input_text(), "花骨朵");
    }

    #[test]
    fn table_width_uses_display_width_for_cjk() {
        let columns = vec!["标题".to_owned(), "Artist".to_owned()];
        let rows = vec![vec!["花骨朵".to_owned(), "Miru".to_owned()]];
        let widths = table_widths(&columns, &rows);
        assert_eq!(widths[0], 6);
        assert_eq!(widths[1], 6);
        let line = format_row(&rows[0], &widths);
        assert_eq!(display_width(&line), 6 + 2 + 6);
    }

    #[test]
    fn block_upsert_replaces_same_id() {
        let mut app = App::new();
        let running = Block {
            id: "same".to_owned(),
            input: "search".to_owned(),
            started_at: 0,
            duration_ms: None,
            status: BlockStatus::Running,
            outputs: vec![],
            footer: None,
            schema_version: 1,
            actions: None,
        };
        let done = Block {
            status: BlockStatus::Done,
            ..running.clone()
        };
        app.upsert_block(running);
        app.upsert_block(done);
        assert_eq!(app.blocks.len(), 1);
        assert_eq!(app.blocks[0].status, BlockStatus::Done);
    }

    #[test]
    fn test_backend_renders_completion_area() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::new();
        app.completion = Some(np_core::Completion {
            replace_start: 0,
            replace_end: 3,
            items: vec![np_core::CompletionItem {
                text: "search".to_owned(),
                kind: np_core::CompletionKind::Command,
                summary: "搜索".to_owned(),
                usage: "search [关键词]".to_owned(),
            }],
        });
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        let compact = text.replace(' ', "");
        assert!(compact.contains("NowherePlayer"), "{text}");
    }

    #[test]
    fn vectorscope_draws_reference_or_samples() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.visual_mode = VisualMode::Vectorscope;
        app.stereo_left = (0..2048).map(|i| (i as f32 * 0.03).sin()).collect();
        app.stereo_right = app.stereo_left.clone();
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let braille = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| {
                let ch = cell.symbol().chars().next().unwrap_or(' ');
                ('\u{2800}'..='\u{28FF}').contains(&ch)
            })
            .count();
        assert!(braille > 4, "vectorscope braille cells: {braille}");
    }

    #[test]
    fn waveform_mode_draws_braille_points() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.visual_mode = VisualMode::Waveform;
        app.waveform = (0..2048)
            .map(|index| ((index as f32) * 0.05).sin())
            .collect();
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let braille = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| {
                let ch = cell.symbol().chars().next().unwrap_or(' ');
                ('\u{2800}'..='\u{28FF}').contains(&ch)
            })
            .count();
        assert!(braille > 4, "waveform braille cells: {braille}");
    }

    #[test]
    fn braille_spectrum_has_more_points_than_one_block_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.spectrum = vec![
            0.2, 0.8, 0.4, 1.0, 0.6, 0.3, 0.9, 0.5, 0.7, 0.1, 0.4, 0.8, 0.2, 0.6, 0.3, 0.9,
        ];
        let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let braille = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| {
                let ch = cell.symbol().chars().next().unwrap_or(' ');
                ('\u{2800}'..='\u{28FF}').contains(&ch)
            })
            .count();
        assert!(braille > 4, "braille cells: {braille}");
    }

    #[test]
    fn cursor_show_sequence_is_once_per_frame() {
        use ratatui::Terminal;
        use ratatui::backend::CrosstermBackend;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        crossterm::style::Colored::set_ansi_color_disabled(false);
        let sink = Sink(Arc::new(Mutex::new(Vec::new())));
        let mut terminal = Terminal::new(CrosstermBackend::new(sink.clone())).unwrap();
        let mut app = App::new();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        for _ in 0..20 {
            terminal
                .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
                .unwrap();
        }
        let bytes = sink.0.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&bytes);
        // Baseline: 20 frames -> 20 show-cursor sequences. At 20fps that is 20/s;
        // at 12fps it is 12/s. C (skip unchanged) is intentionally not applied yet.
        // Normal 模式不显示输入行，所以不再每帧请求光标；只有 Command 模式才显示。
        assert_eq!(text.matches("\x1b[?25h").count(), 0);
    }

    #[test]
    fn paper_background_is_written_to_crossterm_backend() {
        use ratatui::Terminal;
        use ratatui::backend::CrosstermBackend;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let sink = Sink(Arc::new(Mutex::new(Vec::new())));
        crossterm::style::Colored::set_ansi_color_disabled(false);
        let mut terminal = Terminal::new(CrosstermBackend::new(sink.clone())).unwrap();
        let mut app = App::new();
        let theme = TuiThemeApi::new(std::env::temp_dir().join("np-tui-paper-writer"));
        app.load_theme(&theme, "tui-paper");
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("242"),
            "missing paper bg; bytes={}",
            text.escape_default()
        );
    }

    #[test]
    fn paper_theme_fills_background_in_buffer() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        let theme = TuiThemeApi::new(std::env::temp_dir().join("np-tui-paper-buffer"));
        app.load_theme(&theme, "tui-paper");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(0, 0)].style().bg,
            Some(Color::Rgb(242, 239, 233))
        );
    }

    #[test]
    fn filter_is_case_insensitive_and_handles_chinese() {
        assert!(filter_matches(&["晴天".to_owned()], "晴"));
        assert!(filter_matches(&["Jay Chou".to_owned()], "jay"));
        assert!(filter_matches(&["x".to_owned()], ""));
        assert!(!filter_matches(&["晴天".to_owned()], "稻"));
    }

    #[test]
    fn gg_requires_two_presses_within_timeout() {
        let mut app = App::new();
        app.views = vec![ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["#".to_owned()],
            rows: vec![vec!["1".to_owned()], vec!["2".to_owned()]],
        }];
        app.focus = Focus::List;
        app.row_index = 1;
        let start = std::time::Instant::now();
        assert!(!app.press_g(start));
        assert_eq!(app.row_index, 1);
        assert!(app.press_g(start + std::time::Duration::from_millis(200)));
        assert_eq!(app.row_index, 0);
    }

    #[test]
    fn delete_requires_double_press_flag() {
        let mut app = App::new();
        assert!(!app.delete_armed);
        app.delete_armed = true;
        assert!(app.delete_armed);
        app.delete_armed = false;
        assert!(!app.delete_armed);
    }

    #[test]
    fn left_view_selection_changes_middle_source() {
        let mut app = App::new();
        app.views = vec![
            ViewData {
                title: "媒体库".to_owned(),
                columns: vec!["A".to_owned()],
                rows: vec![vec!["lib".to_owned()]],
            },
            ViewData {
                title: "队列".to_owned(),
                columns: vec!["A".to_owned()],
                rows: vec![vec!["queue".to_owned()]],
            },
        ];
        assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
        app.move_view(true);
        assert_eq!(app.selected_view().unwrap().rows[0][0], "queue");
        app.move_view(false);
        assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
    }

    #[test]
    fn tab_changes_focus_border_color() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = App::new();
        app.views = vec![ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["#".to_owned(), "标题".to_owned()],
            rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
        }];
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let left_before = terminal.backend().buffer()[(1, 1)].style().fg;
        app.toggle_focus();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let left_after = terminal.backend().buffer()[(1, 1)].style().fg;
        assert_ne!(left_before, left_after);
        assert_eq!(left_before, Some(Palette::terminal().accent));
        assert_eq!(left_after, Some(Palette::terminal().faint));
    }

    #[test]
    fn normal_mode_hides_command_input() {
        let mut app = App::new();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.replace(' ', "").contains("输入命令"));
        app.enter_command();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.replace(' ', "").contains("命令"));
    }

    #[test]
    fn test_backend_renders_title_and_input() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let audio = TuiAudio::empty();
        let mut cover = CoverView::empty();
        terminal
            .draw(|frame| draw(frame, &mut app, &audio, &mut cover))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("NowherePlayer"), "{rendered}");
    }

    #[test]
    fn np_app_completion_has_search_for_sea() {
        let audio = TuiAudio::empty();
        let completion = complete_for(&audio, "sea", 3);
        assert!(
            completion.items.iter().any(|item| item.text == "search"),
            "{:?}",
            completion.items
        );
    }

    #[test]
    fn settings_file_is_read_as_theme_id() {
        let dir = std::env::temp_dir().join(format!("np-tui-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"theme.id":"tui-paper"}"#).unwrap();
        let settings = SettingsHandle::load(dir.clone());
        assert_eq!(settings.theme_id().as_deref(), Some("tui-paper"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_switch_updates_palette_without_restart() {
        let dir = std::env::temp_dir().join(format!("np-tui-theme-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let api = TuiThemeApi::new(dir);
        let mut app = App::new();
        app.load_theme(&api, "tui-paper");
        assert_eq!(app.palette.bg, Some(Color::Rgb(242, 239, 233)));
        assert_eq!(app.theme_id, "tui-paper");
        app.load_theme(&api, "terminal");
        assert!(app.palette.bg.is_none());
        assert_eq!(app.theme_id, "terminal");
    }

    #[test]
    fn tab_accepts_highlighted_completion() {
        let mut app = App::new();
        app.input = "sea".chars().collect();
        app.cursor = 3;
        app.completion = Some(np_core::Completion {
            replace_start: 0,
            replace_end: 3,
            items: vec![np_core::CompletionItem {
                text: "search".to_owned(),
                kind: np_core::CompletionKind::Command,
                summary: "搜索".to_owned(),
                usage: "search [关键词]".to_owned(),
            }],
        });
        assert!(app.accept_completion());
        assert_eq!(app.input_text(), "search ");
        assert_eq!(app.cursor, 7);
    }

    #[test]
    fn history_navigation_suppresses_completion() {
        let mut app = App::new();
        app.history = vec!["help".to_owned()];
        app.history_up();
        let audio = TuiAudio::empty();
        app.refresh_completion(&audio);
        assert!(app.completion.is_none());
        assert_eq!(app.input_text(), "help");
    }

    #[test]
    fn output_table_maps_to_header_separator_and_rows() {
        let mut lines = Vec::new();
        output_lines(
            &Output::Table {
                columns: vec!["#".to_owned(), "标题".to_owned()],
                rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
                selectable: true,
            },
            &mut lines,
            &Palette::terminal(),
        );
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content.contains("标题"))
        );
    }
}
