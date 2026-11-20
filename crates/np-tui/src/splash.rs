use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use np_app::SettingsHandle;
use np_core::SettingValue;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block as TuiBlock, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::theme::PaletteToken;
use crate::ui::motion;
use np_app::ThemeApi;

pub(crate) use crate::logo::splash_art;

/// LOGO 行数；只有这 6 行参与浮动 / 摇摆。
const LOGO_LINES: u16 = 6;
/// 动效布局总行数：空行 + LOGO + 空行 + 主题 + 空行 + Press。
const ANIMATED_LINES: u16 = LOGO_LINES + 5;
/// 30fps 重绘间隔。
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// 静态回退的轮询间隔。
const STATIC_INTERVAL: Duration = Duration::from_millis(200);
/// LOGO 颜色最多向 accent 靠拢多少：克制，不做闪烁。
const BREATH_DEPTH: f32 = 0.35;
/// 启动后 450ms 的亮度渐入；phase=0 仍是正文色。
const FADE_IN_SECONDS: f32 = 0.45;

pub(crate) fn splash_theme_ids() -> Vec<&'static str> {
    np_core::theme::BUILTIN_THEMES
        .iter()
        .map(|theme| theme.id)
        .collect()
}

pub(crate) fn splash_step(current: &str, delta: i32) -> &'static str {
    let ids = splash_theme_ids();
    let Some(index) = ids.iter().position(|id| *id == current) else {
        return ids.first().copied().unwrap_or("milkgreen");
    };
    let len = ids.len() as i32;
    let next = (index as i32 + delta).rem_euclid(len) as usize;
    ids[next]
}

/// phase=0 的静态入口；golden 只比较字符网格，因此与旧输出逐字符一致。
pub(crate) fn draw_splash<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &App,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    draw_splash_at(terminal, app, 0.0)
}

/// 启动页动效入口：`phase` 是启动以来的秒数，由 `run_splash` 提供。
pub(crate) fn draw_splash_at<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &App,
    phase: f32,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    execute!(io::stdout(), BeginSynchronizedUpdate)?;
    let result = terminal.draw(|frame| draw_splash_content(frame, app, phase));
    let end = execute!(io::stdout(), EndSynchronizedUpdate);
    result?;
    end?;
    Ok(())
}

fn draw_splash_content(frame: &mut Frame, app: &App, phase: f32) {
    let area = frame.area();
    let canvas = app.palette.color(PaletteToken::Canvas);
    let text = app.palette.color(PaletteToken::Text);
    frame.buffer_mut().set_style(
        area,
        Style::default()
            .bg(app.palette.color(PaletteToken::Canvas))
            .fg(app.palette.color(PaletteToken::Text)),
    );
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.color(PaletteToken::Border)))
        .style(
            Style::default()
                .bg(app.palette.color(PaletteToken::Canvas))
                .fg(app.palette.color(PaletteToken::Text)),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let art = splash_art(inner.width);
    let full_logo = art.contains('\n');
    if full_logo && inner.height >= ANIMATED_LINES {
        // 左对齐、按精确 x 坐标画 LOGO；主题名与 Press Enter 单独居中，
        // 不跟着 LOGO 晃。
        let logo_width = u16::try_from(
            art.lines()
                .map(UnicodeWidthStr::width)
                .max()
                .unwrap_or_default(),
        )
        .unwrap_or(inner.width);
        let top = inner.height.saturating_sub(ANIMATED_LINES) / 2;
        let content_y = inner.y.saturating_add(top);
        // 终端太矮 / 太窄时自动禁止位移，保持原居中位置，绝不越界。
        let can_bob = inner.height >= ANIMATED_LINES.saturating_add(2);
        let can_sway = inner.width >= logo_width.saturating_add(4);
        let delta_x = if can_sway { motion::sway(phase) } else { 0 };
        let delta_y = if can_bob { motion::bob(phase) } else { 0 };
        let base_x = inner.x + inner.width.saturating_sub(logo_width) / 2;
        let x = shifted(
            base_x,
            delta_x,
            inner.x,
            inner.right().saturating_sub(logo_width),
        );
        let base_y = content_y.saturating_add(1);
        let y = shifted(
            base_y,
            delta_y,
            inner.y,
            inner.bottom().saturating_sub(LOGO_LINES),
        );
        // phase=0 时就是正文色（旧静态外观），450ms 内渐入，随后在 accent
        // 与正文色之间呼吸；幅度只到 35%，不做闪烁。
        let fade_in = if phase.is_finite() {
            (phase / FADE_IN_SECONDS).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let logo_color = motion::blend(
            text,
            app.palette.color(PaletteToken::Accent),
            BREATH_DEPTH * motion::breath(phase) * fade_in,
        );
        let logo = Paragraph::new(
            art.lines()
                .map(|line| Line::from(line.to_owned()))
                .collect::<Vec<_>>(),
        )
        .style(
            Style::default()
                .bg(app.palette.color(PaletteToken::Canvas))
                .fg(logo_color),
        );
        frame.render_widget(
            logo,
            Rect {
                x,
                y,
                width: logo_width,
                height: LOGO_LINES,
            },
        );
        let footer = Paragraph::new(vec![
            Line::from(format!("◄  {}  ►", app.theme_name)),
            Line::from(""),
            Line::from("Press [Enter] to Start"),
        ])
        .alignment(Alignment::Center)
        .style(Style::default().bg(canvas).fg(text));
        frame.render_widget(
            footer,
            Rect {
                x: inner.x,
                y: content_y.saturating_add(8),
                width: inner.width,
                height: 3,
            },
        );
        return;
    }
    // 窄 / 矮终端：保持原始居中排版（字符网格与旧 golden 一致）。
    let mut lines = vec![Line::from("")];
    lines.extend(art.lines().map(Line::from));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("◄  {}  ►", app.theme_name)));
    lines.push(Line::from(""));
    lines.push(Line::from("Press [Enter] to Start"));
    let content_height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let top = inner.height.saturating_sub(content_height) / 2;
    let content_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(top),
        width: inner.width,
        height: content_height.min(inner.height),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(canvas).fg(text)),
        content_area,
    );
}

/// 把 `base + delta` 夹到 `min..=max`，避免 LOGO 画到缓冲区外。
fn shifted(base: u16, delta: i16, min: u16, max: u16) -> u16 {
    let value = i32::from(base) + i32::from(delta);
    value.clamp(i32::from(min), i32::from(max).max(i32::from(min))) as u16
}

pub(crate) fn run_splash<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    settings: &SettingsHandle,
    theme: &dyn ThemeApi,
) -> Result<bool, Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    if std::env::var_os("NP_TUI_NO_SPLASH").is_some()
        || settings.get_bool("ui.splash") == Some(false)
        || terminal.size()?.height < 20
    {
        return Ok(true);
    }
    let animated = settings.get_bool("ui.motion").unwrap_or(true)
        && std::env::var_os("NP_TUI_NO_ANIM").is_none();
    let started = Instant::now();
    draw_current(terminal, app, animated, started)?;
    let interval = if animated {
        FRAME_INTERVAL
    } else {
        STATIC_INTERVAL
    };
    loop {
        if !event::poll(interval)? {
            // 30fps：没有事件也重绘；静态模式只等按键。
            if animated {
                draw_current(terminal, app, animated, started)?;
            }
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Left => {
                    let id = splash_step(&app.theme_id, -1);
                    app.load_theme(theme, id);
                    // 切主题后用当前 phase 重绘，动画相位不重置。
                    draw_current(terminal, app, animated, started)?;
                }
                KeyCode::Right => {
                    let id = splash_step(&app.theme_id, 1);
                    app.load_theme(theme, id);
                    draw_current(terminal, app, animated, started)?;
                }
                KeyCode::Enter => {
                    let _ =
                        settings.set_and_save("theme.id", SettingValue::Str(app.theme_id.clone()));
                    return Ok(true);
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
                _ => {}
            },
            Event::Resize(_, _) => draw_current(terminal, app, animated, started)?,
            _ => {}
        }
    }
}

fn draw_current<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &App,
    animated: bool,
    started: Instant,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    if animated {
        draw_splash_at(terminal, app, started.elapsed().as_secs_f32())
    } else {
        draw_splash(terminal, app)
    }
}
