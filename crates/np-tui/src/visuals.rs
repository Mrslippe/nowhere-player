//! Player 视觉样式（T-0126 / T-0130）：Braille 栅格频谱与波形、柱状、镜像、环形、星野。
//!
//! 这里只负责在给定 `Rect` 内绘制，不读写 `App` 状态；颜色一律走
//! [`PaletteToken`]。静音（数据全零）时给 faint 基线 / 星点，不黑屏；
//! 每帧只做固定上限的循环，不分配大 `Vec`。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};

use crate::status::now_millis;
use crate::theme::{Palette, PaletteToken};

/// 柱状图每根柱固定 2 格宽、1 格间隔，拉伸窗口时增加柱数而不是加粗。
const BAR_WIDTH: u16 = 2;
const BAR_GAP: u16 = 1;
const BAR_PITCH: u16 = BAR_WIDTH + BAR_GAP;
/// 144×48 上限下最多 144/3 = 48 根；留一点余量给更宽的终端截图。
const MAX_BARS: usize = 64;
/// 环形图固定圈数。
const RING_COUNT: usize = 3;
/// 单圈最大分段数（基础圆与信号弧共用）。
const RING_SEGMENTS: usize = 24;
/// 星野固定星点数。
const STAR_COUNT: u64 = 48;

/// Braille 左列 4 个点行对应的位：上到下 `0x01, 0x02, 0x04, 0x40`。
const BRAILLE_LEFT: [u16; 4] = [0x01, 0x02, 0x04, 0x40];
/// Braille 右列 4 个点行对应的位：上到下 `0x08, 0x10, 0x20, 0x80`。
const BRAILLE_RIGHT: [u16; 4] = [0x08, 0x10, 0x20, 0x80];
/// 一个格子从底部往上填 0..=4 个点行时的点位；两列同时点，避免半列疏密带。
const BRAILLE_BOTTOM: [u16; 5] = [0x00, 0xC0, 0xE4, 0xF6, 0xFF];

/// 固定柱宽柱状图：每根 2 格、间隔 1 格；窗口变宽时增加柱数。
///
/// 16 段频谱插值到当前柱数；静音时只在底部画 faint 基线。
pub(crate) fn draw_bars(frame: &mut ratatui::Frame, area: Rect, values: &[f32], palette: &Palette) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if values.is_empty() || values.iter().all(|value| *value == 0.0) {
        draw_faint_baseline_at(frame, area, palette, area.bottom().saturating_sub(1));
        return;
    }
    let bars = (usize::from(area.width) / usize::from(BAR_PITCH)).clamp(1, MAX_BARS);
    let used = u16::try_from(bars)
        .unwrap_or(area.width)
        .saturating_mul(BAR_PITCH)
        .saturating_sub(BAR_GAP);
    let start_x = area.x + area.width.saturating_sub(used) / 2;
    let accent = Style::default().fg(palette.color(PaletteToken::Accent));
    let max_units = usize::from(area.height) * 8;
    for index in 0..bars {
        let value = spectrum_at(values, index, bars);
        let scaled = (value * max_units as f32).round() as usize;
        let units = if value > 0.0 {
            scaled.max(1).min(max_units)
        } else {
            0
        };
        if units == 0 {
            continue;
        }
        let x = start_x.saturating_add(u16::try_from(index).unwrap_or(0) * BAR_PITCH);
        let bottom = area.bottom().saturating_sub(1);
        let full_rows = units / 8;
        for row in 0..full_rows {
            let y = bottom.saturating_sub(row as u16);
            draw_block_row(frame, x, y, BAR_WIDTH.min(area.width), "█", accent);
        }
        let remainder = units % 8;
        if remainder > 0 {
            let y = bottom.saturating_sub(full_rows as u16);
            let glyph = ["", "▁", "▂", "▃", "▄", "▅", "▆", "▇"][remainder];
            draw_block_row(frame, x, y, BAR_WIDTH.min(area.width), glyph, accent);
        }
    }
}

/// 按终端格子栅格化的频谱：16 段线性插值到每一列，列内从底部填 Braille 点。
///
/// 每列的两个 Braille 子列同时填点，因此任意宽度下都不会出现旧的
/// “65 条浮点线只落在部分子列”造成的周期性疏密带。
pub(crate) fn draw_spectrum_braille(
    frame: &mut ratatui::Frame,
    area: Rect,
    values: &[f32],
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let background = palette.color(PaletteToken::Canvas);
    clear_cells(frame, area, background);
    if values.is_empty() {
        return;
    }
    let total_dots = usize::from(area.height) * 4;
    let style = Style::default()
        .fg(palette.color(PaletteToken::Accent))
        .bg(background);
    let width = usize::from(area.width);
    let heights: Vec<usize> = (0..width)
        .map(|column| {
            let value = spectrum_at(values, column, width);
            if value <= 0.0 {
                0
            } else {
                ((value * total_dots as f32).round() as usize).clamp(1, total_dots)
            }
        })
        .collect();

    let mut line = String::with_capacity(width * 3);
    for row in 0..area.height {
        line.clear();
        let below = usize::from(area.height - 1 - row) * 4;
        for &height in &heights {
            let filled = height.saturating_sub(below).min(4);
            line.push(braille_cell(BRAILLE_BOTTOM[filled]));
        }
        frame
            .buffer_mut()
            .set_string(area.x, area.y + row, &line, style);
    }
}

/// 按终端格子栅格化的波形：每列取一个 sample，相邻列纵向连成折线。
///
/// 每列两个 Braille 子列同时填点；全静音时用 faint 画中心基线。
pub(crate) fn draw_waveform_braille(
    frame: &mut ratatui::Frame,
    area: Rect,
    samples: &[f32],
    palette: &Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let background = palette.color(PaletteToken::Canvas);
    clear_cells(frame, area, background);
    let silent = samples.iter().all(|sample| finite(*sample) == 0.0);
    let color = if silent {
        palette.color(PaletteToken::Faint)
    } else {
        palette.color(PaletteToken::Accent)
    };
    let style = Style::default().fg(color).bg(background);
    let total_dots = usize::from(area.height) * 4;
    let width = usize::from(area.width);
    let mut previous: Option<usize> = None;
    // 每列的纵向区间覆盖“上一列 y → 当前列 y”，保证折线不断列。
    let spans: Vec<(usize, usize)> = (0..width)
        .map(|column| {
            let y = waveform_dot(sample_at(samples, column, width), total_dots);
            let top = previous.map_or(y, |previous| previous.min(y));
            let bottom = previous.map_or(y, |previous| previous.max(y));
            previous = Some(y);
            (top, bottom)
        })
        .collect();

    let mut line = String::with_capacity(width * 3);
    for row in 0..area.height {
        line.clear();
        let cell_top = usize::from(row) * 4;
        let cell_bottom = cell_top + 3;
        for &(top, bottom) in &spans {
            let from = top.max(cell_top);
            let to = bottom.min(cell_bottom);
            let bits = if from > to {
                0
            } else {
                braille_range_bits(from - cell_top, to - cell_top)
            };
            line.push(braille_cell(bits));
        }
        frame
            .buffer_mut()
            .set_string(area.x, area.y + row, &line, style);
    }
}

/// 16 段频谱在 `column` 处的线性插值，结果落在 `0.0..=1.0`。
fn spectrum_at(values: &[f32], column: usize, columns: usize) -> f32 {
    let Some(first) = values.first() else {
        return 0.0;
    };
    if columns <= 1 || values.len() == 1 {
        return finite(*first).clamp(0.0, 1.0);
    }
    let position = column as f32 * (values.len() - 1) as f32 / (columns - 1) as f32;
    let low = (position.floor() as usize).min(values.len() - 1);
    let high = (low + 1).min(values.len() - 1);
    let fraction = position - low as f32;
    let left = finite(values[low]).clamp(0.0, 1.0);
    let right = finite(values[high]).clamp(0.0, 1.0);
    left + (right - left) * fraction
}

/// 每个终端列在 `samples` 上线性取一个样本。
fn sample_at(samples: &[f32], column: usize, columns: usize) -> f32 {
    let Some(first) = samples.first() else {
        return 0.0;
    };
    if columns <= 1 || samples.len() == 1 {
        return finite(*first).clamp(-1.0, 1.0);
    }
    let index = column * (samples.len() - 1) / (columns - 1);
    finite(samples[index]).clamp(-1.0, 1.0)
}

/// sample 值映射到点行坐标（0 = 顶部，`total_dots - 1` = 底部）。
fn waveform_dot(value: f32, total_dots: usize) -> usize {
    let scaled = ((1.0 - value) * 0.5 * total_dots as f32).round() as usize;
    scaled.min(total_dots.saturating_sub(1))
}

/// 一个格子内 `top..=bottom` 点行的位，左右两个子列都点亮。
fn braille_range_bits(top: usize, bottom: usize) -> u16 {
    (top..=bottom).fold(0, |bits, row| bits | BRAILLE_LEFT[row] | BRAILLE_RIGHT[row])
}

/// 无点的格子用普通空格，避免 U+2800（Braille 空白）被当成可见内容。
fn braille_cell(bits: u16) -> char {
    if bits == 0 { ' ' } else { braille_char(bits) }
}

/// `0x2800 + bits` 即 Braille 点阵字符；点位按位或后不会越界。
fn braille_char(bits: u16) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or('\u{2800}')
}

/// 清空 area：必须写空格覆盖上一帧字符，只改样式会留下残影。
fn clear_cells(frame: &mut ratatui::Frame, area: Rect, background: Color) {
    let blank = " ".repeat(usize::from(area.width));
    let style = Style::default().bg(background);
    let buffer: &mut Buffer = frame.buffer_mut();
    for y in area.y..area.bottom() {
        buffer.set_string(area.x, y, &blank, style);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn bar_groups(width: u16) -> Vec<u16> {
        let area = Rect::new(0, 0, width, 19);
        let palette = Palette::default_dark();
        let values = vec![0.5f32; 16];
        let mut terminal = Terminal::new(TestBackend::new(width, 19)).unwrap();
        terminal
            .draw(|frame| draw_bars(frame, area, &values, &palette))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut groups = Vec::new();
        let mut current = 0u16;
        for x in 0..width {
            let filled = (0..19).any(|y| buffer[(x, y)].symbol() != " ");
            if filled {
                current += 1;
            } else if current > 0 {
                groups.push(current);
                current = 0;
            }
        }
        if current > 0 {
            groups.push(current);
        }
        groups
    }

    #[test]
    fn bars_keep_a_fixed_two_cell_width_as_the_window_grows() {
        for (width, expected_bars) in [(66u16, 22usize), (100, 33), (144, 48)] {
            let groups = bar_groups(width);
            assert_eq!(groups.len(), expected_bars, "width={width}");
            assert!(
                groups.iter().all(|group| *group == BAR_WIDTH),
                "width={width} groups={groups:?}"
            );
        }
    }
}
