use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use np_app::SettingsHandle;
use np_core::SettingValue;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Alignment;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block as TuiBlock, Borders, Paragraph};

use crate::app::App;
use crate::theme::PaletteToken;
use np_app::ThemeApi;

pub(crate) use crate::logo::splash_art;

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

pub(crate) fn draw_splash<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &App,
) -> Result<(), Box<dyn std::error::Error>>
where
    B::Error: std::error::Error + 'static,
{
    execute!(io::stdout(), BeginSynchronizedUpdate)?;
    let result = terminal.draw(|frame| {
        let area = frame.area();
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
        let mut lines = vec![Line::from("")];
        let art = splash_art(inner.width);
        lines.extend(art.lines().map(Line::from));
        lines.push(Line::from(""));
        lines.push(Line::from(format!("◄  {}  ►", app.theme_name)));
        lines.push(Line::from(""));
        lines.push(Line::from("Press [Enter] to Start"));
        let content_height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let top = inner.height.saturating_sub(content_height) / 2;
        let content_area = ratatui::layout::Rect {
            x: inner.x,
            y: inner.y.saturating_add(top),
            width: inner.width,
            height: content_height.min(inner.height),
        };
        frame.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center).style(
                Style::default()
                    .bg(app.palette.color(PaletteToken::Canvas))
                    .fg(app.palette.color(PaletteToken::Text)),
            ),
            content_area,
        );
    });
    let end = execute!(io::stdout(), EndSynchronizedUpdate);
    result?;
    end?;
    Ok(())
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
    draw_splash(terminal, app)?;
    loop {
        if event::poll(std::time::Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Left => {
                        let id = splash_step(&app.theme_id, -1);
                        app.load_theme(theme, id);
                        draw_splash(terminal, app)?;
                    }
                    KeyCode::Right => {
                        let id = splash_step(&app.theme_id, 1);
                        app.load_theme(theme, id);
                        draw_splash(terminal, app)?;
                    }
                    KeyCode::Enter => {
                        let _ = settings
                            .set_and_save("theme.id", SettingValue::Str(app.theme_id.clone()));
                        return Ok(true);
                    }
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
                    _ => {}
                },
                Event::Resize(_, _) => draw_splash(terminal, app)?,
                _ => {}
            }
        }
    }
}
