//! 块状条：音量、Live 音高与播放进度共用的 `▰` 绘制。
//!
//! 只负责「在第 index 个槽位画 ▰，前 filled 个用 accent、其余用 faint」；
//! 各调用方保留自己的数值换算、时间文本与标签。

use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::glyphs;
use crate::theme::Palette;

/// 从 `area.x` 开始按 `spacing` 列间距画 `total` 个块，前 `filled` 个高亮。
///
/// `spacing == 1` 即连续音量条，`spacing == 2` 即播放进度 / 音高条。
pub(crate) fn draw_block_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    palette: &Palette,
    filled: usize,
    total: usize,
    spacing: u16,
) {
    if total == 0 || spacing == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let filled = filled.min(total);
    let bright = Style::default().fg(palette.accent);
    let dark = Style::default().fg(palette.faint);
    for index in 0..total {
        let offset = u16::try_from(index)
            .unwrap_or(u16::MAX)
            .saturating_mul(spacing);
        let x = area.x.saturating_add(offset);
        if x >= area.right() {
            break;
        }
        frame.buffer_mut().set_string(
            x,
            area.y,
            glyphs::PROGRESS_DONE,
            if index < filled { bright } else { dark },
        );
    }
}
