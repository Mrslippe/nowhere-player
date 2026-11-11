//! T-0058 可行性 spike：Ratatui 宿主 + Windows Terminal + 真实封面。
//! 不进入主 workspace，不依赖任何产品 crate。

use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    crossterm::{
        event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use ratatui_image::{
    StatefulImage,
    picker::Picker,
    protocol::StatefulProtocol,
};

struct App {
    cover: StatefulProtocol,
    input: String,
    should_quit: bool,
    protocol_debug: String,
}

impl App {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let picker = Picker::from_query_stdio()?;
        let protocol_debug = format!("{:?}", picker.protocol_type());
        fs::write("spike-protocol.txt", &protocol_debug)?;

        let image = image::ImageReader::open("assets/cover.jpg")?.decode()?;
        let cover = picker.new_resize_protocol(image);
        Ok(Self {
            cover,
            input: String::new(),
            should_quit: false,
            protocol_debug,
        })
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // title
            Constraint::Min(10),   // table + cover
            Constraint::Length(3), // input
            Constraint::Length(1), // status
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("Nowhere Player · TUI Host Spike", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("   中英混排 / 真实封面 / 输入法"),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(title, rows[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(rows[1]);

    let header = Row::new(["#", "中文标题", "English Artist", "时长"])
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .bottom_margin(0);
    let songs = [
        ("1", "♪ 晴天", "Jay Chou", "04:29"),
        ("2", "花骨朵（Live）", "Miru", "03:17"),
        ("3", "最长的电影", "Jay Chou", "03:56"),
        ("4", "不为谁而作的歌", "JJ Lin", "04:12"),
    ];
    let table_rows = songs.iter().map(|(n, title, artist, duration)| {
        Row::new(vec![
            Cell::from(*n),
            Cell::from(*title),
            Cell::from(*artist),
            Cell::from(*duration),
        ])
    });
    let table = Table::new(
        table_rows,
        [
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(18),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" 搜索结果 · 4 rows "));
    frame.render_widget(table, body[0]);

    let cover_area = body[1];
    frame.render_widget(
        Block::default().borders(Borders::ALL).title(format!(
            " 专辑封面 · {} ",
            app.protocol_debug
        )),
        cover_area,
    );
    let inner = Rect {
        x: cover_area.x + 1,
        y: cover_area.y + 1,
        width: cover_area.width.saturating_sub(2),
        height: cover_area.height.saturating_sub(2),
    };
    frame.render_stateful_widget(StatefulImage::new(), inner, &mut app.cover);

    let input = Paragraph::new(Line::from(vec![
        Span::styled("输入：", Style::default().fg(Color::Green)),
        Span::raw(app.input.as_str()),
        Span::styled("█", Style::default().fg(Color::Green)),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" 中文输入 / IME "));
    frame.render_widget(input, rows[2]);

    let status = Paragraph::new(format!(
        "协议 {} · 输入 {} 字 · Esc 退出",
        app.protocol_debug,
        app.input.chars().count()
    ))
    .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(status, rows[3]);
}

fn run<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Esc => app.should_quit = true,
                    KeyCode::Enter => app.input.push('\n'),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Char(character) => app.input.push(character),
                    _ => {}
                },
                Event::Paste(text) => app.input.push_str(&text),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut app = App::new()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| draw(frame, &mut app))?;

    let startup_ms = started.elapsed().as_millis();
    fs::write("spike-startup-ms.txt", startup_ms.to_string())?;
    fs::write("spike-pid.txt", std::process::id().to_string())?;

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    // 保持窗口几秒，方便 PowerShell 截屏；随后自然退出。
    if Path::new("spike-hold.txt").exists() {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    result?;
    Ok(())
}
