//! Player 视觉样式（T-0126）：柱状、镜像、环形、星野。
//!
//! 这里只负责在给定 `Rect` 内绘制，不读写 `App` 状态；颜色一律走
//! [`PaletteToken`]。静音（数据全零）时给 faint 基线 / 星点，不黑屏；
//! 每帧只做固定上限的循环，不分配大 `Vec`。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};

use crate::status::now_millis;
use crate::theme::{Palette, PaletteToken};

/// 柱状图频段数，与 `update_spectrum` 的 16 段一致。
const BAR_COUNT: usize = 16;
/// 环形图固定圈数。
const RING_COUNT: usize = 3;
/// 单圈最大分段数（基础圆与信号弧共用）。
const RING_SEGMENTS: usize = 24;
/// 星野固定星点数。
const STAR_COUNT: u64 = 48;

/// 16 段块状柱：宽度自适应，静音时只在底部画 faint 基线。
pub(crate) fn draw_bars(frame: &mut ratatui::Frame, area: Rect, values: &[f32], palette: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if values.iter().all(|value| *value == 0.0) {
        draw_faint_baseline_at(frame, area, palette, area.bottom().saturating_sub(1));
        return;
    }
    let accent = Style::default().fg(palette.color(PaletteToken::Accent));
    let bars = BAR_COUNT.min(usize::from(area.width)).max(1);
    let width = u32::from(area.width);
    let max_units = usize::from(area.height) * 8;
    for index in 0..bars {
        let band = index * BAR_COUNT / bars;
        let value = finite(values.get(band).copied().unwrap_or(0.0)).clamp(0.0, 1.0);
        let scaled = (value * max_units as f32).round() as usize;
        let units = if value > 0.0 {
            scaled.max(1).min(max_units)
        } else {
            0
        };
        if units == 0 {
            continue;
        }
        let start = (width * index as u32 / bars as u32) as u16;
        let end = (width * (index + 1) as u32 / bars as u32) as u16;
        let x = area.x.saturating_add(start);
        let slot = end.saturating_sub(start).max(1);
        let bar_width = if slot > 1 { slot - 1 } else { 1 };
        let bottom = area.bottom().saturating_sub(1);
        let full_rows = units / 8;
        for row in 0..full_rows {
            let y = bottom.saturating_sub(row as u16);
            draw_block_row(frame, x, y, bar_width, "█", accent);
        }
        let remainder = units % 8;
        if remainder > 0 {
            let y = bottom.saturating_sub(full_rows as u16);
            let glyph = ["", "▁", "▂", "▃", "▄", "▅", "▆", "▇"][remainder];
            draw_block_row(frame, x, y, bar_width, glyph, accent);
        }
    }
}

/// 中心镜像波形：上下对称，中心线 faint，波形 accent；静音只留中心线。
pub(crate) fn draw_mirror(
    frame: &mut ratatui::Frame,
    area: Rect,
    samples: &[f32],
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let faint = palette.color(PaletteToken::Faint);
    let accent = palette.color(PaletteToken::Accent);
    let background = palette.color(PaletteToken::Canvas);
    let silent = samples.iter().all(|sample| *sample == 0.0);
    let canvas = Canvas::default()
        .x_bounds([0.0, 64.0])
        .y_bounds([-1.0, 1.0])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            ctx.draw(&CanvasLine {
                x1: 0.0,
                y1: 0.0,
                x2: 64.0,
                y2: 0.0,
                color: faint,
            });
            if silent {
                return;
            }
            let count = samples.len().max(2);
            let mut previous_top: Option<(f64, f64)> = None;
            let mut previous_bottom: Option<(f64, f64)> = None;
            for index in 0..=64 {
                let sample_index = index * (count - 1) / 64;
                let value =
                    finite(samples.get(sample_index).copied().unwrap_or(0.0)).clamp(-1.0, 1.0);
                let x = index as f64;
                let y = f64::from(value);
                if let Some((x1, y1)) = previous_top {
                    ctx.draw(&CanvasLine {
                        x1,
                        y1,
                        x2: x,
                        y2: y,
                        color: accent,
                    });
                }
                if let Some((x1, y1)) = previous_bottom {
                    ctx.draw(&CanvasLine {
                        x1,
                        y1,
                        x2: x,
                        y2: -y,
                        color: accent,
                    });
                }
                previous_top = Some((x, y));
                previous_bottom = Some((x, -y));
            }
        });
    frame.render_widget(canvas, area);
}

/// 频段驱动的同心环：固定 3 圈，基础圆 faint，信号弧 accent。
pub(crate) fn draw_rings(
    frame: &mut ratatui::Frame,
    area: Rect,
    values: &[f32],
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let faint = palette.color(PaletteToken::Faint);
    let accent = palette.color(PaletteToken::Accent);
    let background = palette.color(PaletteToken::Canvas);
    // 终端字符约 2:1；按可视区宽高压缩 y 轴，圆看起来才是圆。
    let y_half = (2.0 * f64::from(area.height) / f64::from(area.width)).clamp(0.15, 1.0);
    let radii = [0.4, 0.7, 1.0];
    let energies = ring_energies(values);
    let canvas = Canvas::default()
        .x_bounds([-1.0, 1.0])
        .y_bounds([-y_half, y_half])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            let step = std::f64::consts::TAU / RING_SEGMENTS as f64;
            for (ring, fraction) in radii.iter().enumerate() {
                let radius = y_half * fraction;
                for segment in 0..RING_SEGMENTS {
                    let start = segment as f64 * step;
                    ctx.draw(&arc(radius, start, start + step, faint));
                }
                let energy = energies.get(ring).copied().unwrap_or(0.0);
                let lit = ((f64::from(energy) * RING_SEGMENTS as f64).round() as usize)
                    .min(RING_SEGMENTS);
                for segment in 0..lit {
                    let start = -std::f64::consts::FRAC_PI_2 + segment as f64 * step;
                    ctx.draw(&arc(radius, start, start + step, accent));
                }
            }
        });
    frame.render_widget(canvas, area);
}

/// 低频驱动的星野：确定性星点，相位从 `now_millis` 取；静音时静止。
pub(crate) fn draw_starfield(
    frame: &mut ratatui::Frame,
    area: Rect,
    values: &[f32],
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let faint = palette.color(PaletteToken::Faint);
    let accent = palette.color(PaletteToken::Accent);
    let background = palette.color(PaletteToken::Canvas);
    let energy = values
        .iter()
        .take(4)
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0f32, f32::max)
        .clamp(0.0, 1.0);
    // 用毫秒时间算相位；静音时 speed=0，星点完全静止，不产生额外闪烁。
    let speed = if energy <= 0.001 {
        0.0
    } else {
        0.05 + f64::from(energy) * 0.35
    };
    let phase = now_millis() as f64 / 1000.0 * speed;
    let canvas = Canvas::default()
        .x_bounds([0.0, 1.0])
        .y_bounds([0.0, 1.0])
        .background_color(background)
        .marker(Marker::Braille)
        .paint(move |ctx| {
            for index in 0..STAR_COUNT {
                let x = star_unit(index * 3 + 1);
                let base_y = star_unit(index * 3 + 2);
                let drift = 0.5 + base_y * 0.8;
                let y = (base_y - phase * drift).rem_euclid(1.0);
                let color = if index.is_multiple_of(4) {
                    accent
                } else {
                    faint
                };
                ctx.draw(&CanvasLine {
                    x1: x,
                    y1: y,
                    x2: x,
                    y2: y,
                    color,
                });
            }
        });
    frame.render_widget(canvas, area);
    draw_faint_baseline_at(frame, area, palette, area.bottom().saturating_sub(1));
}

/// 横向 faint 基线；没有信号的样式用它避免黑屏。
fn draw_faint_baseline_at(frame: &mut ratatui::Frame, area: Rect, palette: &Palette, y: u16) {
    if area.width == 0 || y < area.y || y >= area.bottom() {
        return;
    }
    let line = "░".repeat(usize::from(area.width));
    frame.buffer_mut().set_string(
        area.x,
        y,
        &line,
        Style::default().fg(palette.color(PaletteToken::Faint)),
    );
}

fn draw_block_row(
    frame: &mut ratatui::Frame,
    x: u16,
    y: u16,
    width: u16,
    glyph: &str,
    style: Style,
) {
    if width == 0 {
        return;
    }
    let row = glyph.repeat(usize::from(width));
    frame.buffer_mut().set_string(x, y, &row, style);
}

fn arc(radius: f64, start: f64, end: f64, color: Color) -> CanvasLine {
    CanvasLine {
        x1: radius * start.cos(),
        y1: radius * start.sin(),
        x2: radius * end.cos(),
        y2: radius * end.sin(),
        color,
    }
}

/// 把 16 段频谱按低 / 中 / 高分给 3 个同心环；空输入全零。
fn ring_energies(values: &[f32]) -> [f32; RING_COUNT] {
    let mut energies = [0.0f32; RING_COUNT];
    if values.is_empty() {
        return energies;
    }
    let bands = values.len();
    for (ring, energy) in energies.iter_mut().enumerate() {
        let start = ring * bands / RING_COUNT;
        let end = ((ring + 1) * bands / RING_COUNT).clamp(start + 1, bands);
        *energy = values[start..end]
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(0.0f32, f32::max)
            .clamp(0.0, 1.0);
    }
    energies
}

/// 确定性伪随机：同一个 index 每帧得到同一颗星，不需要存 Vec。
fn star_unit(seed: u64) -> f64 {
    let mut value = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    (value >> 11) as f64 / (1u64 << 53) as f64
}

fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}
