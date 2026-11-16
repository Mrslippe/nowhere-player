use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::Paragraph;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, VisualMode};
use crate::audio::TuiAudio;
use crate::fft::spectrum;
use crate::glyphs;
use crate::keymap::{self, Action};
use crate::transport::TransportAvailability;
use crate::truncate_width;
use np_core::{PlaybackMode, RepeatMode};

pub(crate) fn draw_player_strip(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    player: &TuiAudio,
) {
    if area.width < 8 || area.height < 3 {
        return;
    }
    draw_player_frame(
        frame,
        area,
        app,
        player.volume(),
        player.mode(),
        TransportAvailability::from_player(player),
    );
    let content = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    if content.width == 0 || content.height < 2 {
        return;
    }

    let visual_height = content.height.saturating_sub(1);
    let visual_area = Rect {
        x: content.x,
        y: content.y,
        width: content.width,
        height: visual_height,
    };
    let idle =
        app.visual_mode == VisualMode::Spectrum && app.spectrum.iter().all(|value| *value == 0.0);
    if visual_height > 0 {
        if idle {
            let line = "░".repeat(content.width as usize);
            let style = Style::default().fg(app.palette.faint);
            for y in visual_area.y..visual_area.bottom() {
                frame
                    .buffer_mut()
                    .set_string(visual_area.x, y, &line, style);
            }
        } else {
            draw_visual(frame, visual_area, app, app.palette.accent, app.palette.bg);
        }
    }

    let live = player.is_live();
    let duration = if live {
        0
    } else {
        player.duration_ms().unwrap_or(0)
    };
    let position = if live {
        0
    } else {
        player.position_ms().min(duration)
    };
    let overlay_y = content.bottom().saturating_sub(2);
    let time = if live {
        live_time_label(player.live_buffer_ms())
    } else {
        format!("{} / {}", format_time(position), format_time(duration))
    };
    let time_width = UnicodeWidthStr::width(time.as_str()) as u16;
    let bar_width = content
        .width
        .saturating_sub(time_width)
        .saturating_sub(2)
        .max(4) as usize;
    let max_text = content.width.saturating_sub(2) as usize;
    let title = format_info_title(&player.title(), &player.artist(), max_text);
    let title_width = UnicodeWidthStr::width(title.as_str()) as u16;
    if title_width > 0 {
        let patch_width = title_width.saturating_add(2).min(content.width);
        let patch_x = content.right().saturating_sub(patch_width);
        let bg = app.palette.bg.unwrap_or(Color::Reset);
        frame.buffer_mut().set_string(
            patch_x,
            overlay_y,
            " ".repeat(patch_width as usize),
            Style::default().bg(bg),
        );
        frame.render_widget(
            Paragraph::new(Line::from(TuiSpan::styled(
                title,
                Style::default().fg(app.palette.muted).bg(bg),
            )))
            .alignment(Alignment::Right),
            Rect {
                x: patch_x,
                y: overlay_y,
                width: patch_width,
                height: 1,
            },
        );
    }

    let ratio = if duration == 0 {
        0.0
    } else {
        position as f64 / duration as f64
    };
    let filled = ((bar_width as f64 * ratio).round() as usize).min(bar_width);
    let mut spans = Vec::with_capacity(bar_width);
    for column in 0..bar_width {
        if column % 2 == 1 {
            spans.push(TuiSpan::raw(" "));
        } else if column < filled {
            spans.push(TuiSpan::styled(
                glyphs::PROGRESS_DONE,
                Style::default().fg(app.palette.accent),
            ));
        } else {
            spans.push(TuiSpan::styled(
                glyphs::PROGRESS_DONE,
                Style::default().fg(app.palette.faint),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: content.x,
            y: content.bottom().saturating_sub(1),
            width: bar_width as u16,
            height: 1,
        },
    );
    frame.render_widget(
        Paragraph::new(time)
            .alignment(Alignment::Right)
            .style(Style::default().fg(app.palette.muted)),
        Rect {
            x: content.right().saturating_sub(time_width),
            y: content.bottom().saturating_sub(1),
            width: time_width,
            height: 1,
        },
    );
}

/// 直播时间位：`LIVE`，缓冲可读时带上队列毫秒数（B1.2 的 900ms 目标）。
fn live_time_label(buffer_ms: Option<u64>) -> String {
    match buffer_ms {
        Some(ms) if ms > 0 => format!("LIVE {ms}ms"),
        _ => "LIVE".to_owned(),
    }
}

fn format_info_title(title: &str, artist: &str, max_width: usize) -> String {
    let suffix = if artist.trim().is_empty() {
        String::new()
    } else {
        format!(" — {artist}")
    };
    let suffix_width = UnicodeWidthStr::width(suffix.as_str());
    if suffix_width >= max_width {
        return truncate_width(&suffix, max_width);
    }
    let title = truncate_width(title, max_width - suffix_width);
    format!("{title}{suffix}")
}

fn draw_player_frame(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    volume: f32,
    mode: PlaybackMode,
    transport: TransportAvailability,
) {
    let width = area.width as usize;
    if width < 4 {
        return;
    }
    let title = " Player ";
    let mode = match (mode.repeat, mode.shuffle) {
        (_, true) => " Mode ⇄ ",
        (RepeatMode::One, false) => " Mode ↺ ",
        (RepeatMode::All, false) => " Mode ↻ ",
        (RepeatMode::Off, false) => " Mode ⇉ ",
    };
    let title_w = UnicodeWidthStr::width(title);
    let mode_w = UnicodeWidthStr::width(mode);
    let top_fill = width.saturating_sub(2 + title_w);
    let bottom_fill = width.saturating_sub(2 + mode_w);
    let top = format!("┏{title}{}┓", "━".repeat(top_fill));
    let bottom = format!("┗{}{mode}┛", "━".repeat(bottom_fill));
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
    let volume_width = 20u16;
    if area.width > volume_width + 8 {
        let filled =
            ((volume.clamp(0.0, 1.0) * f32::from(volume_width)).round() as u16).min(volume_width);
        let gap_x = area.right().saturating_sub(2);
        let start = gap_x.saturating_sub(volume_width);
        let bright = Style::default().fg(app.palette.accent);
        let dark = Style::default().fg(app.palette.faint);
        let bg = app.palette.bg.unwrap_or(Color::Reset);
        for index in 0..volume_width {
            frame.buffer_mut().set_string(
                start + index,
                area.y,
                "▰",
                if index < filled { bright } else { dark },
            );
        }
        frame
            .buffer_mut()
            .set_string(gap_x, area.y, " ", Style::default().bg(bg));
        let label = " Vol ";
        let label_width = UnicodeWidthStr::width(label) as u16;
        frame.buffer_mut().set_string(
            start.saturating_sub(label_width),
            area.y,
            label,
            Style::default().fg(app.palette.muted),
        );
    }
    draw_transport(frame, area, app, transport);
}

fn draw_transport(
    frame: &mut ratatui::Frame,
    area: Rect,
    app: &App,
    transport: TransportAvailability,
) {
    if area.width < 48 {
        return;
    }
    let bottom = area.bottom().saturating_sub(1);
    let start_x = area.x.saturating_add(2);
    let edge_style = Style::default().fg(app.palette.line);
    // 左端留一格空白，按钮从下一格开始。
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), bottom, " ", edge_style);

    let hints = app.show_key_hints;
    let gap = if hints { "  " } else { " " };
    let key_style = Style::default()
        .fg(app.palette.faint)
        .add_modifier(Modifier::ITALIC);
    let disabled = Style::default()
        .fg(app.palette.faint)
        .add_modifier(Modifier::DIM);
    let button_style = Style::default().fg(app.palette.accent);
    let paused_style = Style::default()
        .fg(app.palette.fg)
        .add_modifier(Modifier::BOLD);
    let playing_style = Style::default()
        .fg(app.palette.accent)
        .add_modifier(Modifier::BOLD);
    let flash_style = Style::default()
        .bg(app.palette.accent)
        .fg(app.palette.bg.unwrap_or(Color::Reset))
        .add_modifier(Modifier::BOLD);
    let feedback = |button: Action| {
        app.transport_feedback.is_some_and(|(current, at)| {
            current == button && at.elapsed() < std::time::Duration::from_millis(180)
        })
    };
    let mut spans = Vec::new();
    let mut width = 0u16;
    for (index, action) in keymap::TRANSPORT_ACTIONS.into_iter().enumerate() {
        let label = transport_glyph(action, transport.playing);
        if index > 0 {
            spans.push(TuiSpan::raw(gap));
            width = width.saturating_add(UnicodeWidthStr::width(gap) as u16);
        }
        let enabled = transport.enabled(action);
        let style = if feedback(action) {
            flash_style
        } else if !enabled {
            disabled
        } else if action == Action::TogglePlay {
            if transport.playing {
                playing_style
            } else {
                paused_style
            }
        } else {
            button_style
        };
        spans.push(TuiSpan::styled(label, style));
        width = width.saturating_add(UnicodeWidthStr::width(label) as u16);
        if hints {
            let key = keymap::label(action);
            spans.push(TuiSpan::raw(" "));
            width = width.saturating_add(1);
            spans.push(TuiSpan::styled(key, key_style));
            width = width.saturating_add(UnicodeWidthStr::width(key) as u16);
        }
    }
    if start_x.saturating_add(width) >= area.right().saturating_sub(1) {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: start_x,
            y: bottom,
            width,
            height: 1,
        },
    );
    // 右端留一格空白，再接后面的边框线。
    frame
        .buffer_mut()
        .set_string(start_x.saturating_add(width), bottom, " ", edge_style);
}

fn transport_glyph(action: Action, playing: bool) -> &'static str {
    match action {
        Action::PrevTrack => "◀◀",
        Action::SeekBack => "<<",
        Action::TogglePlay => {
            if playing {
                "❚❚"
            } else {
                "▶"
            }
        }
        Action::SeekForward => ">>",
        Action::NextTrack => "▶▶",
        _ => "",
    }
}

pub(crate) fn draw_spectrum(
    frame: &mut ratatui::Frame,
    area: Rect,
    values: &[f32],
    accent: Color,
    background: Color,
) {
    let canvas = Canvas::default()
        .x_bounds([0.0, 15.0])
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

#[cfg(test)]
mod tests {
    use super::{format_info_title, live_time_label};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn info_title_folds_title_but_keeps_artist() {
        let text = format_info_title(&"Luv(Sic)-1".repeat(8), "Nujabes", 30);
        assert!(text.contains("Nujabes"));
        assert!(UnicodeWidthStr::width(text.as_str()) <= 30);
    }

    #[test]
    fn live_label_shows_the_queue_buffer_when_known() {
        assert_eq!(live_time_label(None), "LIVE");
        assert_eq!(live_time_label(Some(0)), "LIVE");
        assert_eq!(live_time_label(Some(900)), "LIVE 900ms");
    }
}
