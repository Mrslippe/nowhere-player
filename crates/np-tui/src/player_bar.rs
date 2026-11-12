use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::Paragraph;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use unicode_width::UnicodeWidthStr;

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
    if area.width < 8 || area.height < 3 {
        return;
    }
    draw_player_frame(frame, area, app);
    let content = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if content.width == 0 || content.height < 2 {
        return;
    }

    let visual_height = content.height.saturating_sub(2);
    if visual_height > 0 {
        draw_visual(
            frame,
            Rect {
                x: content.x,
                y: content.y,
                width: content.width,
                height: visual_height,
            },
            app,
            app.palette.accent,
            app.palette.bg,
        );
    }

    let duration = player.duration_ms().unwrap_or(0);
    let position = player.position_ms().min(duration);
    let title = truncate_width(
        &format!(
            "{} — {}  {} / {}",
            player.title(),
            player.artist(),
            format_time(position),
            format_time(duration)
        ),
        content.width as usize,
    );
    frame.render_widget(
        Paragraph::new(Line::from(TuiSpan::styled(
            title,
            Style::default()
                .fg(app.palette.muted)
                .bg(app.palette.bg.unwrap_or(Color::Reset)),
        )))
        .alignment(Alignment::Right),
        Rect {
            x: content.x,
            y: content.y + visual_height,
            width: content.width,
            height: 1,
        },
    );

    let ratio = if duration == 0 {
        0.0
    } else {
        position as f64 / duration as f64
    };
    let bar_width = content.width as usize;
    let filled = ((bar_width as f64 * ratio).round() as usize).min(bar_width);
    let progress = format!(
        "{}{}",
        glyphs::PROGRESS_DONE.repeat(filled),
        glyphs::PROGRESS_TODO.repeat(bar_width.saturating_sub(filled))
    );
    frame.render_widget(
        Paragraph::new(progress).style(Style::default().fg(app.palette.accent)),
        Rect {
            x: content.x,
            y: content.bottom().saturating_sub(1),
            width: content.width,
            height: 1,
        },
    );
}

fn draw_player_frame(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    if width < 4 {
        return;
    }
    let title = " Player ";
    let mode = " Mode ↻ ";
    let title_w = UnicodeWidthStr::width(title);
    let mode_w = UnicodeWidthStr::width(mode);
    let top_fill = width.saturating_sub(2 + title_w);
    let bottom_fill = width.saturating_sub(2 + mode_w);
    let top = format!("╭{title}{}╮", "─".repeat(top_fill));
    let bottom = format!("╰{}{mode}╯", "─".repeat(bottom_fill));
    let line = Style::default().fg(app.palette.line_strong);
    frame.buffer_mut().set_string(area.x, area.y, &top, line);
    frame.buffer_mut().set_string(
        area.x.saturating_add(1),
        area.y,
        title,
        Style::default().fg(app.palette.accent),
    );
    frame
        .buffer_mut()
        .set_string(area.x, area.bottom().saturating_sub(1), &bottom, line);
    frame.buffer_mut().set_string(
        area.right().saturating_sub(mode_w as u16 + 1),
        area.bottom().saturating_sub(1),
        mode,
        Style::default().fg(app.palette.muted),
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
