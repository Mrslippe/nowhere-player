use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block as TuiBlock, Borders, Paragraph};

use crate::app::{App, VisualMode};
use crate::audio::TuiAudio;
use crate::fft::spectrum;
use crate::glyphs;
use crate::truncate_width;

pub(crate) fn draw_player_strip(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    player: &TuiAudio,
) {
    let block = TuiBlock::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(app.palette.line_strong))
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

    let title = truncate_width(
        &format!("{} · {}", player.title(), player.artist()),
        inner.width as usize,
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
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );

    let duration = player.duration_ms().unwrap_or(0);
    let position = player.position_ms().min(duration);
    let ratio = if duration == 0 {
        0.0
    } else {
        position as f64 / duration as f64
    };
    let bar_width = inner.width.saturating_sub(14).max(4) as usize;
    let filled = ((bar_width as f64 * ratio).round() as usize).min(bar_width);
    let progress = format!(
        "{}{}  {} / {}",
        glyphs::PROGRESS_DONE.repeat(filled),
        glyphs::PROGRESS_TODO.repeat(bar_width.saturating_sub(filled)),
        format_time(position),
        format_time(duration)
    );
    frame.render_widget(
        Paragraph::new(progress),
        Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        },
    );

    let visual_height = inner.height.saturating_sub(3).min(2);
    if visual_height > 0 {
        draw_visual(
            frame,
            Rect {
                x: inner.x,
                y: inner.y + 2,
                width: inner.width,
                height: visual_height,
            },
            app,
            app.palette.accent,
            app.palette.bg,
        );
    }
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
            x: inner.x,
            y: inner.y + 2 + visual_height,
            width: inner.width,
            height: 1,
        },
    );
}

pub(crate) fn draw_spectrum(
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

pub(crate) fn draw_visual(
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

pub(crate) fn draw_vectorscope(
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

pub(crate) fn draw_waveform(
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

pub(crate) fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

pub(crate) fn update_spectrum(app: &mut App, player: &TuiAudio) {
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
