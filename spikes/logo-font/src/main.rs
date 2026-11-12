//! LOGO 字体实验台。
//!
//! 用途：在真实 Windows Terminal 里对比不同字体下的 NOWHERE logo 对齐情况，
//! 并提供一个最小的“切换默认字体”入口（写 WT fragment + 新窗口预览）。
//!
//! 重要：终端字体属于宿主，运行中的进程无法改当前窗口的字体。
//! 本工具的做法是写一个独立的 Windows Terminal fragment（Nowhere Logo Preview），
//! 再用 `wt.exe -w new -p "Nowhere Logo Preview"` 打开新窗口。

#[path = "../../../crates/np-tui/src/logo.rs"]
mod logo;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

const PROFILE_NAME: &str = "Nowhere Logo Preview";

struct FontChoice {
    key: char,
    label: &'static str,
    face: Option<&'static str>,
}

const FONTS: &[FontChoice] = &[
    FontChoice {
        key: '1',
        label: "Sarasa Mono CL",
        face: Some("Sarasa Mono CL"),
    },
    FontChoice {
        key: '2',
        label: "Sarasa Mono TC",
        face: Some("Sarasa Mono TC"),
    },
    FontChoice {
        key: '3',
        label: "Maple Mono NF CN",
        face: Some("Maple Mono NF CN"),
    },
    FontChoice {
        key: '4',
        label: "Cascadia Mono",
        face: Some("Cascadia Mono"),
    },
    FontChoice {
        key: '5',
        label: "Consolas",
        face: Some("Consolas"),
    },
    FontChoice {
        key: '0',
        label: "终端默认（不写 font）",
        face: None,
    },
];

#[derive(Debug, Default)]
struct Args {
    font: Option<String>,
    glyph: Option<char>,
    print: bool,
    help: bool,
}

struct App {
    font: Option<String>,
    glyph: Option<char>,
    status: String,
    guide: bool,
    quit: bool,
}

impl App {
    fn new(font: Option<String>, glyph: Option<char>) -> Self {
        Self {
            font,
            glyph,
            status: "1–5/0 选字体 · g 标尺 · w 写 fragment · r 新窗口 · q 退出".to_owned(),
            guide: true,
            quit: false,
        }
    }

    fn font_label(&self) -> &str {
        self.font.as_deref().unwrap_or("终端默认")
    }

    fn target_label(&self) -> String {
        match self.glyph {
            Some(glyph) => format!("单字 {glyph}"),
            None => "NOWHERE".to_owned(),
        }
    }

    fn art(&self, width: u16) -> String {
        match self.glyph.and_then(logo::glyph_for) {
            Some(glyph) => logo::compose(&[glyph]).join("\n"),
            None => logo::splash_art(width),
        }
    }

    fn select(&mut self, key: char) {
        let Some(choice) = FONTS.iter().find(|choice| choice.key == key) else {
            return;
        };
        self.font = choice.face.map(str::to_owned);
        match write_fragment(self.font.as_deref(), self.glyph) {
            Ok(path) => {
                self.status = format!(
                    "已写 {} · 按 r 打开新窗口（若 profile 不存在，先关闭所有 WT 再试）",
                    path.display()
                );
            }
            Err(error) => {
                self.status = format!("写 fragment 失败：{error}");
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    if args.print {
        print_logo(args.font.as_deref(), args.glyph);
        return Ok(());
    }
    run_tui(App::new(args.font, args.glyph))
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => args.help = true,
            "--print" => args.print = true,
            "--glyph" => {
                let value = iter.next().ok_or("--glyph 需要一个字母，例如 D")?;
                let mut chars = value.chars();
                let glyph = chars
                    .next()
                    .ok_or("--glyph 需要一个字母，例如 D")?
                    .to_ascii_uppercase();
                if chars.next().is_some() {
                    return Err("--glyph 只接受一个字母".into());
                }
                if logo::glyph_for(glyph).is_none() {
                    return Err(format!("暂无字形：{glyph}（N/O/W/H/E/R/D）").into());
                }
                args.glyph = Some(glyph);
            }
            "--font" => {
                let value = iter.next().ok_or("--font 需要一个字体名")?;
                args.font = if value == "default" || value == "终端默认" {
                    None
                } else {
                    Some(value)
                };
            }
            other => return Err(format!("未知参数：{other}（--help 查看用法）").into()),
        }
    }
    Ok(args)
}

fn print_help() {
    println!("LOGO 字体实验台");
    println!();
    println!("用法：");
    println!("  cargo run --release                 交互预览");
    println!("  cargo run --release -- --print      只打印 logo / 网格宽度");
    println!("  cargo run --release -- --glyph D    单字预览（N/O/W/H/E/R/D）");
    println!("  cargo run --release -- --font \"Sarasa Mono CL\"");
    println!();
    println!("交互键：1–5/0 选字体，g 标尺，w 写 fragment，r 新窗口，q 退出。");
}

fn print_logo(font: Option<&str>, glyph: Option<char>) {
    let art = match glyph.and_then(logo::glyph_for) {
        Some(glyph) => logo::compose(&[glyph]).join("\n"),
        None => logo::splash_art(80),
    };
    let widths: Vec<usize> = art.lines().map(UnicodeWidthStr::width).collect();
    let art_width = widths.iter().copied().max().unwrap_or(0);
    let (tens, units) = ruler(art_width);
    println!("font: {}", font.unwrap_or("终端默认"));
    println!(
        "target: {}",
        glyph
            .map(|glyph| format!("单字 {glyph}"))
            .unwrap_or_else(|| "NOWHERE".to_owned())
    );
    println!("rows:");
    for line in art.lines() {
        println!("  {:>2}  {line}", UnicodeWidthStr::width(line));
    }
    println!("row_widths: {widths:?}");
    println!("art_width: {art_width}");
    println!();
    println!("{tens}");
    println!("{units}");
    println!("{art}");
}

fn run_tui(mut app: App) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|frame| draw(frame, &app))?;
            if event::poll(std::time::Duration::from_millis(150))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
                    KeyCode::Char('g') => app.guide = !app.guide,
                    KeyCode::Char('w') => match write_fragment(app.font.as_deref(), app.glyph) {
                        Ok(path) => app.status = format!("已写 {}", path.display()),
                        Err(error) => app.status = format!("写 fragment 失败：{error}"),
                    },
                    KeyCode::Char('r') => match launch_preview() {
                        Ok(()) => app.status = "已请求打开 Nowhere Logo Preview".to_owned(),
                        Err(error) => app.status = format!("启动 wt 失败：{error}"),
                    },
                    KeyCode::Char(key @ ('0'..='5')) => app.select(key),
                    _ => {}
                }
            }
            if app.quit {
                break;
            }
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn draw(frame: &mut ratatui::Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" NOWHERE · LOGO FONT LAB "),
        area,
    );
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    if inner.width < 8 || inner.height < 8 {
        return;
    }

    let art = app.art(inner.width);
    let widths: Vec<usize> = art.lines().map(UnicodeWidthStr::width).collect();
    let distinct: HashSet<usize> = widths.iter().copied().collect();
    let art_width = widths.iter().copied().max().unwrap_or(0);
    let grid_state = if distinct.len() <= 1 {
        "aligned"
    } else {
        "MISALIGNED"
    };

    let (tens, units) = ruler(art_width);
    let ruler_style = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(Span::styled(
            format!("font: {}", app.font_label()),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "grid: {grid_state} · {art_width} cols · {}",
                app.target_label()
            ),
            ruler_style,
        )),
        Line::from(""),
    ];
    if app.guide {
        lines.push(Line::from(Span::styled(tens, ruler_style)));
        lines.push(Line::from(Span::styled(units, ruler_style)));
        lines.push(Line::from(""));
    }
    lines.extend(art.lines().map(|line| Line::from(line.to_owned())));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("row widths: {widths:?}"),
        ruler_style,
    )));
    lines.push(Line::from(""));
    for choice in FONTS {
        let selected = choice.face == app.font.as_deref();
        let marker = if selected { "▸" } else { " " };
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{marker} [{}] {}", choice.key, choice.label),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        &app.status,
        Style::default().fg(Color::Yellow),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Gray)),
        inner,
    );
}

fn ruler(width: usize) -> (String, String) {
    let mut tens = String::with_capacity(width);
    let mut units = String::with_capacity(width);
    for column in 0..width {
        units.push(char::from(b'0' + (column % 10) as u8));
        if column % 10 == 0 {
            tens.push(char::from(b'0' + ((column / 10) % 10) as u8));
        } else {
            tens.push(' ');
        }
    }
    (tens, units)
}

fn write_fragment(face: Option<&str>, glyph: Option<char>) -> io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA 未设置"))?;
    let path = fragment_path(&base);
    let exe = std::env::current_exe()?;
    let mut commandline = format!("\"{}\"", exe.display());
    if let Some(face) = face {
        commandline.push_str(&format!(" --font \"{face}\""));
    }
    if let Some(glyph) = glyph {
        commandline.push_str(&format!(" --glyph {glyph}"));
    }
    let font_line = face
        .map(|face| {
            format!(
                "      \"font\": {{ \"face\": \"{}\", \"size\": 12 }},\n",
                json_escape(face)
            )
        })
        .unwrap_or_default();
    let json = format!(
        r#"{{
  "profiles": [
    {{
      "name": "{PROFILE_NAME}",
      "commandline": "{}",
{}      "padding": "0,0,0,0",
      "opacity": 96,
      "useAcrylic": true,
      "cursorShape": "filledBox",
      "suppressApplicationTitle": true,
      "tabTitle": "Nowhere Logo"
    }}
  ]
}}
"#,
        json_escape(&commandline),
        font_line
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, json.as_bytes())?;
    Ok(path)
}

fn fragment_path(local_app_data: &Path) -> PathBuf {
    local_app_data
        .join("Microsoft")
        .join("Windows Terminal")
        .join("Fragments")
        .join("NowhereLogo")
        .join("nowhere-logo.json")
}

fn launch_preview() -> io::Result<()> {
    Command::new("wt.exe")
        .args(["-w", "new", "-p", PROFILE_NAME])
        .spawn()?;
    Ok(())
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_has_a_unique_key() {
        let keys: HashSet<char> = FONTS.iter().map(|choice| choice.key).collect();
        assert_eq!(keys.len(), FONTS.len());
    }

    #[test]
    fn ruler_is_column_accurate() {
        let (tens, units) = ruler(23);
        assert_eq!(units, "01234567890123456789012");
        assert_eq!(tens.chars().filter(|ch| *ch != ' ').count(), 3);
        assert!(tens.starts_with('0'));
    }

    #[test]
    fn fragment_path_and_json_escape() {
        let path = fragment_path(Path::new("C:/base"));
        assert!(path.ends_with("nowhere-logo.json"));
        assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
    }
}
