//! T-0060 np-tui 骨架：输入行 + 执行 np-app 命令 + 块输出。
//!
//! 这一版只覆盖任务卡的 1/4/5：输入、执行、text/table/error/hint。
//! 在线搜索异步分派、补全、播放器面板留给后续卡片。

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

mod audio;
mod cover;
mod fft;

use audio::TuiAudio;
use cover::CoverView;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fft::{level_char, spectrum};
use np_app::{
    Execution, LibraryHandle, NoopLibraryObserver, Paths, PlayerApi, Services, complete_for,
    execute_with_services,
};
use np_core::{Block, BlockStatus, Output, Span, SpanColor, Tone};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span as TuiSpan},
    widgets::{Block as TuiBlock, Borders, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

/// 最小 dark 调色板（T-0060 §1.3）。
#[derive(Debug, Clone, Copy)]
struct Palette {
    bg: Color,
    fg: Color,
    muted: Color,
    accent: Color,
    danger: Color,
}

const PALETTE: Palette = Palette {
    bg: Color::Rgb(0x0d, 0x0f, 0x12),
    fg: Color::Rgb(0xe9, 0xe4, 0xda),
    muted: Color::Rgb(0x7d, 0x7c, 0x78),
    accent: Color::Rgb(0xff, 0xb4, 0x54),
    danger: Color::Rgb(0xff, 0x8b, 0x7d),
};

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
    spectrum_generation: u64,
    completion: Option<np_core::Completion>,
    completion_index: usize,
    last_edit: std::time::Instant,
    history_nav: bool,
    search_id: u64,
    search_pending: bool,
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
            spectrum_generation: 0,
            completion: None,
            completion_index: 0,
            last_edit: std::time::Instant::now(),
            history_nav: false,
            search_id: 0,
            search_pending: false,
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
            Services::default(),
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let data_dir = tui_data_dir();
    let config_dir = data_dir.join("config");
    let cache_dir = data_dir.join("cache");
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&cache_dir)?;
    let library = Arc::new(LibraryHandle::with_paths(Paths {
        config_dir,
        cache_dir,
        data_dir,
    }));
    let (search_tx, search_rx) = std::sync::mpsc::channel();
    let mut player = TuiAudio::from_env();
    let mut app = App::new();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let cover_path = resolve_cover(&player, &library);
    let mut cover = cover_path
        .as_deref()
        .map(CoverView::load)
        .unwrap_or_else(CoverView::empty);
    terminal.draw(|frame| draw(frame, &mut app, &player, &mut cover))?;
    if let Ok(path) = std::env::var("NP_TUI_STARTUP_FILE") {
        std::fs::write(path, started.elapsed().as_millis().to_string())?;
    }
    let result = run(
        &mut terminal,
        &mut app,
        &mut player,
        &mut cover,
        &library,
        &search_tx,
        &search_rx,
    );
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    result?;
    Ok(())
}

fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    player: &mut TuiAudio,
    cover: &mut CoverView,
    library: &Arc<LibraryHandle>,
    search_tx: &std::sync::mpsc::Sender<(u64, Block)>,
    search_rx: &std::sync::mpsc::Receiver<(u64, Block)>,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    let mut draw_needed = true;
    loop {
        while let Ok((search_id, block)) = search_rx.try_recv() {
            if search_id == app.search_id {
                app.upsert_block(block);
                app.status = "搜索完成".to_owned();
                app.search_pending = false;
                draw_needed = true;
            }
        }
        if app.refresh_completion(player) || cover.poll() {
            draw_needed = true;
        }
        if draw_needed {
            update_spectrum(app, player);
            terminal.draw(|frame| draw(frame, app, player, cover))?;
            draw_needed = false;
        }
        let timeout = if player.is_playing() || app.search_pending || !app.input.is_empty() {
            std::time::Duration::from_millis(50)
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
                        match key.code {
                            KeyCode::Esc => {
                                if app.completion.is_some() {
                                    app.completion = None;
                                    draw_needed = true;
                                } else if app.immersive {
                                    app.immersive = false;
                                    draw_needed = true;
                                } else {
                                    return Ok(());
                                }
                            }
                            KeyCode::Enter => {
                                app.completion = None;
                                let input = app.input_text();
                                app.run(input, player, library, search_tx);
                                draw_needed = true;
                            }
                            KeyCode::Tab => {
                                if app.accept_completion() {
                                    draw_needed = true;
                                }
                            }
                            KeyCode::Char(' ') if app.input.is_empty() => {
                                player.toggle();
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
                            KeyCode::Up => {
                                if app.completion.is_some() {
                                    app.move_completion(false);
                                } else {
                                    app.history_up();
                                }
                                draw_needed = true;
                            }
                            KeyCode::Down => {
                                if app.completion.is_some() {
                                    app.move_completion(true);
                                } else {
                                    app.history_down();
                                }
                                draw_needed = true;
                            }
                            KeyCode::PageUp => {
                                app.scroll = app.scroll.saturating_sub(5);
                                app.auto_scroll = false;
                                draw_needed = true;
                            }
                            KeyCode::PageDown => {
                                app.scroll = app.scroll.saturating_add(5);
                                app.auto_scroll = false;
                                draw_needed = true;
                            }
                            _ => {}
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
    tap.latest(&mut samples);
    app.spectrum = spectrum(&samples, tap.sample_rate(), 16);
    app.spectrum_generation = tap.generation();
}

fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio, cover: &mut CoverView) {
    let area = frame.area();
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
                .fg(PALETTE.accent)
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw("  TUI 骨架"),
    ]))
    .style(Style::default().bg(PALETTE.bg).fg(PALETTE.fg));
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
                .border_style(Style::default().fg(PALETTE.accent))
                .title(" 输入命令 "),
        )
        .style(Style::default().bg(PALETTE.bg).fg(PALETTE.fg));
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
    let status = Paragraph::new(format!(
        " {} · 输入 {} 字 · ↑↓ 历史 · Enter 执行 · {} · Ctrl+F 沉浸 · Esc 退出 ",
        app.status,
        app.input.len(),
        panel_hint
    ))
    .style(Style::default().bg(PALETTE.bg).fg(PALETTE.muted));
    frame.render_widget(status, rows[4]);
}

fn draw_completion(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PALETTE.muted))
        .title(" 候选 ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(completion) = &app.completion else {
        return;
    };
    let mut lines = Vec::new();
    for (index, item) in completion.items.iter().take(5).enumerate() {
        let selected = index == app.completion_index;
        let marker = if selected { "▸" } else { " " };
        let text = format!("{marker} {}  {}", item.text, item.summary);
        let style = if selected {
            Style::default()
                .bg(PALETTE.fg)
                .fg(PALETTE.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(PALETTE.fg)
        };
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&text, inner.width as usize),
            style,
        )));
    }
    if let Some(item) = completion.items.get(app.completion_index) {
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&item.usage, inner.width as usize),
            Style::default().fg(PALETTE.muted),
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
                .border_style(Style::default().fg(PALETTE.muted))
                .title(" 输出 "),
        )
        .style(Style::default().bg(PALETTE.bg).fg(PALETTE.fg))
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
        .border_style(Style::default().fg(PALETTE.muted))
        .title(format!(" 播放 · {} ", cover.protocol()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 4 || inner.height < 4 {
        return;
    }

    let cover_height = if immersive {
        inner.height.saturating_mul(6) / 10
    } else {
        inner.height.saturating_sub(6).clamp(2, 4)
    };
    let cover_area = Rect {
        x: inner.x,
        y: inner.y,
        width: if immersive {
            inner.width.saturating_mul(6) / 10
        } else {
            inner.width
        },
        height: cover_height,
    };
    cover.render(cover_area, frame);

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
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                app_spectrum_chars(&app.spectrum, 0.0, text_width as usize),
                Style::default().fg(PALETTE.accent),
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
    let title = truncate_width(player.title(), text_width as usize);
    let artist = truncate_width(player.artist(), text_width as usize);
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                title,
                Style::default().fg(PALETTE.fg).add_modifier(Modifier::BOLD),
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
                Style::default().fg(PALETTE.muted),
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
        "▮".repeat(filled),
        "▯".repeat(bar_width.saturating_sub(filled)),
        format_time(position),
        format_time(duration)
    );
    if y < inner.y + inner.height {
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                progress,
                Style::default().fg(PALETTE.fg),
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
                "◀◀   ▶   ▶▶",
                Style::default().fg(PALETTE.accent),
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

fn app_spectrum_chars(values: &[f32], threshold: f32, width: usize) -> String {
    values
        .iter()
        .take(width)
        .map(|value| {
            if *value >= threshold {
                level_char(*value)
            } else {
                ' '
            }
        })
        .collect()
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
        TuiSpan::styled("› ", Style::default().fg(PALETTE.accent)),
        TuiSpan::raw(before),
        TuiSpan::styled(
            cursor_char,
            Style::default()
                .bg(PALETTE.fg)
                .fg(PALETTE.bg)
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
                TuiSpan::styled("› ", Style::default().fg(PALETTE.accent)),
                TuiSpan::raw(block.input.clone()),
            ]));
        }
        for output in &block.outputs {
            output_lines(output, &mut lines);
        }
        if let Some(footer) = &block.footer {
            lines.push(Line::styled(
                footer.clone(),
                Style::default().fg(PALETTE.muted),
            ));
        }
    }
    if lines.is_empty() {
        lines.push(Line::styled(
            "输入 help 或 library list --limit=10 开始",
            Style::default().fg(PALETTE.muted),
        ));
    }
    lines
}

fn span_style(span: &Span, tone: Option<Tone>) -> Style {
    let mut style = Style::default();
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.dim {
        style = style.fg(PALETTE.muted);
    }
    let color = span.color.map(|color| match color {
        SpanColor::Accent => PALETTE.accent,
        SpanColor::Danger => PALETTE.danger,
        SpanColor::Warn => PALETTE.accent,
        SpanColor::Success => PALETTE.fg,
        SpanColor::Muted => PALETTE.muted,
    });
    let tone = tone.map(|tone| match tone {
        Tone::Accent => PALETTE.accent,
        Tone::Danger => PALETTE.danger,
        Tone::Warn => PALETTE.accent,
        Tone::Success => PALETTE.fg,
        Tone::Muted => PALETTE.muted,
    });
    style.fg(color.or(tone).unwrap_or(PALETTE.fg))
}

fn output_lines(output: &Output, lines: &mut Vec<Line<'static>>) {
    match output {
        Output::Text { spans, tone } => {
            lines.push(Line::from(
                spans
                    .iter()
                    .map(|span| TuiSpan::styled(span.text.clone(), span_style(span, *tone)))
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
                    .fg(PALETTE.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(
                widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("  "),
                Style::default().fg(PALETTE.muted),
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
                Style::default().fg(PALETTE.danger),
            ));
            if let Some(hint) = hint {
                lines.push(Line::styled(
                    hint.clone(),
                    Style::default().fg(PALETTE.muted),
                ));
            }
        }
        Output::Hint { text, suggestion } => {
            let mut line = text.clone();
            if let Some(suggestion) = suggestion {
                line.push_str(" · ");
                line.push_str(suggestion);
            }
            lines.push(Line::styled(line, Style::default().fg(PALETTE.muted)));
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
                Style::default().fg(PALETTE.accent),
            ));
        }
        Output::Media { kind, .. } => {
            lines.push(Line::styled(
                format!("[媒体 {kind:?}]"),
                Style::default().fg(PALETTE.muted),
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
        assert!(compact.contains("候选"), "{text}");
        assert!(compact.contains("search"), "{text}");
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
        assert!(compact.contains("输入命令"), "{rendered}");
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
