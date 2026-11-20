//! 外框与状态栏的共享绘制。
//!
//! 只共享形状与坐标，文案、状态判定和提示列表仍由调用方提供：
//! - 主界面外框标题带版本号，Live 页只写产品名；
//! - 主界面状态栏是「状态 + 键/说明」，Live 页是一段整色提示。

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block as TuiBlock, BorderType, Borders};

use crate::input::{display_width, truncate_width};
use crate::theme::{Palette, PaletteToken};

/// 外框：粗边框、左下角主题名；标题由调用方给全（含两侧空格）。
pub(crate) fn draw_outer_chrome(
    frame: &mut ratatui::Frame,
    area: Rect,
    palette: &Palette,
    title: &str,
    theme_name: &str,
) {
    let outer = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(palette.color(PaletteToken::Muted)))
        .style(
            Style::default()
                .bg(palette.color(PaletteToken::Canvas))
                .fg(palette.color(PaletteToken::Text)),
        );
    frame.render_widget(outer.title(title), area);
    let theme = format!(" {theme_name} ");
    let theme_x = area
        .right()
        .saturating_sub(display_width(theme.as_str()) as u16 + 1);
    frame.buffer_mut().set_string(
        theme_x,
        area.y,
        &theme,
        Style::default()
            .fg(palette.color(PaletteToken::Accent))
            .add_modifier(Modifier::ITALIC),
    );
}

/// 状态栏左侧状态；`require_space` 为真时状态必须整段放得下才绘制。
pub(crate) struct StatusState<'a> {
    pub(crate) text: &'a str,
    pub(crate) warn: bool,
    pub(crate) require_space: bool,
}

/// 状态栏提示的两种既有形态。
pub(crate) enum StatusHints<'a> {
    /// 主界面：按键 + 说明，逐项用 ` · ` 分隔。
    Keyed(&'a [(&'a str, &'a str)]),
    /// Live 页：调用方拼好的一整段提示，统一用 muted 色。
    Plain(&'a str),
}

/// 状态栏基础绘制：覆盖外框下边线、左右各留 1 格，再交给调用方的文案。
pub(crate) fn draw_status_bar(
    frame: &mut ratatui::Frame,
    area: Rect,
    palette: &Palette,
    state: Option<&StatusState<'_>>,
    hints: StatusHints<'_>,
) {
    if area.height < 2 || area.width < 8 {
        return;
    }
    let y = area.bottom().saturating_sub(1);
    let max_x = area.right().saturating_sub(2);
    let mut x = area.x.saturating_add(2);
    // 状态栏覆盖在外框下边线上，延伸线沿用外框的深色，而不是内容区的浅色 line。
    let edge_style = Style::default().fg(palette.color(PaletteToken::Muted));
    // 先把这一行恢复成连续的边框线，避免宽字符跳过相邻格时把上一帧的
    // `·` / 键名残留在缓冲里。
    let border_line = "━".repeat(usize::from(area.width.saturating_sub(2)));
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), y, &border_line, edge_style);
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), y, " ", edge_style);

    let mut drew_text = false;
    if let Some(state) = state {
        let width = display_width(state.text) as u16;
        if !state.require_space || x.saturating_add(width) <= max_x {
            frame.buffer_mut().set_string(
                x,
                y,
                state.text,
                Style::default()
                    .fg(if state.warn {
                        palette.color(PaletteToken::Warn)
                    } else {
                        palette.color(PaletteToken::Accent)
                    })
                    .add_modifier(Modifier::BOLD),
            );
            x = x.saturating_add(width);
            drew_text = true;
            if matches!(hints, StatusHints::Keyed(_)) {
                let separator = " · ";
                let sep_width = display_width(separator) as u16;
                if x.saturating_add(sep_width) <= max_x {
                    frame.buffer_mut().set_string(
                        x,
                        y,
                        separator,
                        Style::default().fg(palette.color(PaletteToken::Faint)),
                    );
                    x = x.saturating_add(sep_width);
                }
            }
        }
    }

    match hints {
        StatusHints::Keyed(items) => {
            for (index, (key, description)) in items.iter().enumerate() {
                let key_width = display_width(key) as u16;
                let description_width = display_width(description) as u16;
                let item_width = key_width + description_width;
                let separator = " · ";
                let separator_width = display_width(separator) as u16;
                let reserve = if index > 0 { separator_width } else { 0 };
                if x.saturating_add(reserve + item_width) > max_x {
                    break;
                }
                if index > 0 {
                    frame.buffer_mut().set_string(
                        x,
                        y,
                        separator,
                        Style::default().fg(palette.color(PaletteToken::Faint)),
                    );
                    x = x.saturating_add(separator_width);
                }
                frame.buffer_mut().set_string(
                    x,
                    y,
                    key,
                    Style::default()
                        .fg(palette.color(PaletteToken::Accent))
                        .add_modifier(Modifier::BOLD),
                );
                x = x.saturating_add(key_width);
                frame.buffer_mut().set_string(
                    x,
                    y,
                    description,
                    Style::default().fg(palette.color(PaletteToken::Muted)),
                );
                x = x.saturating_add(description_width);
                drew_text = true;
            }
        }
        StatusHints::Plain(text) => {
            let available = usize::from(max_x.saturating_sub(x));
            let hints = truncate_width(text, available);
            if hints.is_empty() {
                return;
            }
            frame.buffer_mut().set_string(
                x,
                y,
                &hints,
                Style::default().fg(palette.color(PaletteToken::Muted)),
            );
            x = x.saturating_add(display_width(&hints) as u16);
            drew_text = true;
        }
    }

    // 右侧留白必须紧跟最后一段文本，而不是固定在倒数第二格：
    // 文本结束后先空一格，再让外框线继续。
    if drew_text && x <= max_x {
        frame.buffer_mut().set_string(x, y, " ", edge_style);
    }
}
