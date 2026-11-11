//! T-0060 np-tui 骨架：输入行 + 执行 np-app 命令 + 块输出。
//!
//! 这一版只覆盖任务卡的 1/4/5：输入、执行、text/table/error/hint。
//! 在线搜索异步分派、补全、播放器面板留给后续卡片。

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use np_app::{
    Execution, LibraryApi, LibraryHandle, NoopLibraryObserver, Paths, PlayerApi, Services,
    execute_with_services,
};
use np_core::{Block, BlockStatus, Output, PlaybackState, QueueView, Span, SpanColor, Tone};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
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

/// 骨架阶段没有音频；所有播放调用都返回空闲状态。
struct NullPlayer;

impl PlayerApi for NullPlayer {
    fn snapshot(&self) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn play(&self, _path: Option<String>) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn pause(&self) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn resume(&self) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn next(&self) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn prev(&self) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn seek(&self, _position_ms: u64) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn volume(&self, _volume: f32) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn mode(&self, _mode: np_core::PlaybackMode) -> Result<PlaybackState, np_app::CmdError> {
        Ok(PlaybackState::default())
    }
    fn queue_list(&self) -> Result<QueueView, np_app::CmdError> {
        Ok(QueueView {
            tracks: Vec::new(),
            index: None,
        })
    }
    fn queue_add(&self, _paths: Vec<String>) -> Result<QueueView, np_app::CmdError> {
        self.queue_list()
    }
    fn queue_remove(&self, _index: usize) -> Result<QueueView, np_app::CmdError> {
        self.queue_list()
    }
    fn queue_clear(&self) -> Result<QueueView, np_app::CmdError> {
        self.queue_list()
    }
    fn queue_jump(&self, _index: usize) -> Result<QueueView, np_app::CmdError> {
        self.queue_list()
    }
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
        }
    }

    fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    fn insert(&mut self, character: char) {
        self.input.insert(self.cursor, character);
        self.cursor += 1;
        self.history_cursor = None;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.input.remove(self.cursor);
            self.history_cursor = None;
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
        self.input = self.history[next].chars().collect();
        self.cursor = self.input.len();
    }

    fn history_down(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_cursor = None;
            self.input.clear();
            self.cursor = 0;
        } else {
            self.history_cursor = Some(index + 1);
            self.input = self.history[index + 1].chars().collect();
            self.cursor = self.input.len();
        }
    }

    fn run(&mut self, input: String, player: &dyn PlayerApi, library: &dyn LibraryApi) {
        if input.trim().is_empty() {
            return;
        }
        self.history.push(input.clone());
        self.input.clear();
        self.cursor = 0;
        self.history_cursor = None;
        self.status = format!("执行：{input}");
        let block_id = format!("tui-{}", now_millis());
        let execution = execute_with_services(
            player,
            library,
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
                self.blocks.push(*done);
                self.auto_scroll = true;
            }
            Execution::Accepted { running } => {
                self.status = "后台任务已受理".to_owned();
                self.blocks.push(*running);
                self.auto_scroll = true;
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    let data_dir = std::env::var_os("NP_TUI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let local = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("tmp/bench");
            if local.is_dir() {
                local
            } else {
                std::env::temp_dir().join("nowhere-tui")
            }
        });
    let config_dir = data_dir.join("config");
    let cache_dir = data_dir.join("cache");
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&cache_dir)?;
    let library = LibraryHandle::with_paths(Paths {
        config_dir,
        cache_dir,
        data_dir,
    });
    let player = NullPlayer;
    let mut app = App::new();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| draw(frame, &mut app))?;
    if let Ok(path) = std::env::var("NP_TUI_STARTUP_FILE") {
        std::fs::write(path, started.elapsed().as_millis().to_string())?;
    }
    let result = run(&mut terminal, &mut app, &player, &library);
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
    player: &dyn PlayerApi,
    library: &dyn LibraryApi,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Enter => {
                        let input = app.input_text();
                        app.run(input, player, library);
                    }
                    KeyCode::Char(character) => app.insert(character),
                    KeyCode::Backspace => app.backspace(),
                    KeyCode::Left => app.left(),
                    KeyCode::Right => app.right(),
                    KeyCode::Up => app.history_up(),
                    KeyCode::Down => app.history_down(),
                    KeyCode::PageUp => {
                        app.scroll = app.scroll.saturating_sub(5);
                        app.auto_scroll = false;
                    }
                    KeyCode::PageDown => {
                        app.scroll = app.scroll.saturating_add(5);
                        app.auto_scroll = false;
                    }
                    _ => {}
                },
                Event::Paste(text) => {
                    for character in text.chars() {
                        app.insert(character);
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
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

    let lines = all_lines(app);
    let output_height = rows[1].height.saturating_sub(2);
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
    frame.render_widget(output, rows[1]);

    let input_line = input_line(app);
    let input = Paragraph::new(input_line)
        .block(
            TuiBlock::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(PALETTE.accent))
                .title(" 输入命令 "),
        )
        .style(Style::default().bg(PALETTE.bg).fg(PALETTE.fg));
    frame.render_widget(input, rows[2]);

    let cursor_x = rows[2]
        .x
        .saturating_add(1)
        .saturating_add(input_width_before(app) as u16);
    let cursor_y = rows[2].y.saturating_add(1);
    frame.set_cursor_position((cursor_x, cursor_y));

    let status = Paragraph::new(format!(
        " {} · 输入 {} 字 · ↑↓ 历史 · Enter 执行 · PgUp/PgDn 滚动 · Esc 退出 ",
        app.status,
        app.input.len()
    ))
    .style(Style::default().bg(PALETTE.bg).fg(PALETTE.muted));
    frame.render_widget(status, rows[3]);
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
    fn test_backend_renders_title_and_input() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        let compact = rendered.replace(' ', "");
        assert!(compact.contains("NowherePlayer"), "{rendered}");
        assert!(compact.contains("输入命令"), "{rendered}");
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
