use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, Focus};
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::*;
use crate::layout::layout_areas;
use crate::model::BrowseKind;
use crate::player_bar::draw_player_strip;

pub(crate) fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio) {
    let full = frame.area();
    frame.render_widget(Clear, full);
    if let Some(bg) = app.palette.bg {
        frame.buffer_mut().set_style(full, Style::default().bg(bg));
    }
    if full.width < 4 || full.height < 6 {
        return;
    }
    let outer = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(app.palette.muted))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let version = format!(" NOWHERE PLAYER v{} ", env!("CARGO_PKG_VERSION"));
    let theme = format!(" {} ", app.theme_name);
    let border_style = Style::default().fg(app.palette.line);
    frame.render_widget(outer.title(version.clone()), full);
    let theme_x = full
        .right()
        .saturating_sub(display_width(theme.as_str()) as u16 + 1);
    frame.buffer_mut().set_string(
        theme_x,
        full.y,
        &theme,
        Style::default().fg(app.palette.accent),
    );
    let _ = border_style;

    let [top, player_area, controller_area] = layout_areas(full);

    draw_browse_list(frame, top, app);
    draw_player_strip(frame, player_area, app, player);
    draw_controller(frame, controller_area, app);
    if app.completion.is_some() && controller_area.y > top.y + 3 {
        let height = 7.min(controller_area.y - top.y - 1);
        let width = 58.min(controller_area.width.saturating_sub(4));
        let popup = Rect {
            x: controller_area.x + 2,
            y: controller_area.y - height,
            width,
            height,
        };
        frame.render_widget(Clear, popup);
        draw_completion(frame, popup, app);
    }
    draw_status_border(frame, full, app);
}

fn draw_status_border(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height < 2 || area.width < 8 {
        return;
    }
    let y = area.bottom().saturating_sub(1);
    let max_x = area.right().saturating_sub(2);

    let hints: &[(&str, &str)] = match app.focus {
        Focus::Controller => &[
            ("←→", "光标"),
            ("Tab", "补全"),
            ("Enter", "执行"),
            ("Esc", "返回"),
        ],
        Focus::Browse => &[
            ("↑↓", "选择"),
            ("←→", "焦点"),
            ("Enter", "进入"),
            ("/", "命令"),
            ("q", "退出"),
        ],
        Focus::List => &[
            ("↑↓", "选择"),
            ("←→", "焦点"),
            ("Enter", "播放"),
            ("Space", "暂停"),
            ("n/p", "切歌"),
            ("/", "命令"),
            ("q", "退出"),
        ],
    };
    let mut x = area.x.saturating_add(2);
    for (index, (key, description)) in hints.iter().enumerate() {
        if index > 0 {
            let separator = " · ";
            let separator_width = display_width(separator) as u16;
            if x.saturating_add(separator_width) > max_x {
                break;
            }
            frame
                .buffer_mut()
                .set_string(x, y, separator, Style::default().fg(app.palette.faint));
            x = x.saturating_add(separator_width);
        }
        let key_width = display_width(key) as u16;
        let description_width = display_width(description) as u16;
        if x.saturating_add(key_width + description_width) > max_x {
            break;
        }
        frame.buffer_mut().set_string(
            x,
            y,
            key,
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        );
        x = x.saturating_add(key_width);
        frame
            .buffer_mut()
            .set_string(x, y, description, Style::default().fg(app.palette.muted));
        x = x.saturating_add(description_width);
    }
}

fn border_text(
    frame: &mut ratatui::Frame,
    area: Rect,
    y: u16,
    text: &str,
    right: bool,
    style: Style,
) {
    if area.width < 2 {
        return;
    }
    let width = display_width(text) as u16;
    if width + 2 > area.width {
        return;
    }
    let x = if right {
        area.right().saturating_sub(width + 2)
    } else {
        area.x + 2
    };
    frame.buffer_mut().set_string(x, y, text, style);
}

fn browse_width(inner_width: u16) -> u16 {
    let proportional = (u32::from(inner_width) * 25 / 100) as u16;
    proportional
        .clamp(18, 26)
        .min(inner_width.saturating_sub(10))
}

fn paint_browse_borders(
    frame: &mut ratatui::Frame,
    area: Rect,
    split: u16,
    focused: bool,
    app: &App,
) {
    let line = Style::default().fg(app.palette.line);
    let accent = Style::default().fg(app.palette.accent);
    let browse_style = if app.focus == Focus::Browse {
        accent
    } else {
        line
    };
    let list_style = if app.focus == Focus::List {
        accent
    } else {
        line
    };
    let divider_style = if focused { accent } else { line };
    let bottom = area.bottom().saturating_sub(1);
    for x in area.x..=split {
        frame.buffer_mut()[(x, area.y)].set_style(browse_style);
        frame.buffer_mut()[(x, bottom)].set_style(browse_style);
    }
    for x in split.saturating_add(1)..area.right() {
        frame.buffer_mut()[(x, area.y)].set_style(list_style);
        frame.buffer_mut()[(x, bottom)].set_style(list_style);
    }
    for y in area.y..=bottom {
        frame.buffer_mut()[(area.x, y)].set_style(browse_style);
        frame.buffer_mut()[(area.right().saturating_sub(1), y)].set_style(list_style);
        frame.buffer_mut()[(split, y)].set_style(divider_style);
    }
}

fn draw_browse_list(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.focus != Focus::Controller;
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(app.palette.line))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let split = inner
        .x
        .saturating_add(browse_width(inner.width))
        .min(inner.right().saturating_sub(1));
    paint_browse_borders(frame, area, split, focused, app);
    let browse_title_style = Style::default()
        .fg(if app.focus == Focus::Browse {
            app.palette.accent
        } else {
            app.palette.muted
        })
        .add_modifier(Modifier::BOLD);
    let list_title_style = Style::default()
        .fg(if app.focus == Focus::List {
            app.palette.accent
        } else {
            app.palette.muted
        })
        .add_modifier(Modifier::BOLD);
    border_text(frame, area, area.y, " Browse ", false, browse_title_style);
    frame
        .buffer_mut()
        .set_string(split.saturating_add(1), area.y, " List ", list_title_style);
    let view_number = if app.views.is_empty() {
        0
    } else {
        app.view_index + 1
    };
    let left_count = format!(" {view_number} of {} ", app.views.len());
    let right_count = format!(" total {} ", app.filtered_row_count());
    let count_style = Style::default()
        .fg(app.palette.faint)
        .add_modifier(Modifier::ITALIC);
    let left_count_x = split.saturating_sub(display_width(&left_count) as u16 + 1);
    frame.buffer_mut().set_string(
        left_count_x,
        area.bottom().saturating_sub(1),
        &left_count,
        count_style,
    );
    border_text(
        frame,
        area,
        area.bottom().saturating_sub(1),
        &right_count,
        true,
        count_style,
    );
    let left = Rect {
        x: inner.x,
        y: inner.y,
        width: split.saturating_sub(inner.x),
        height: inner.height,
    };
    let right = Rect {
        x: split.saturating_add(1),
        y: inner.y,
        width: inner.right().saturating_sub(split.saturating_add(1)),
        height: inner.height,
    };
    draw_library(frame, left, app);
    draw_list(frame, right, app);
    draw_list_scrollbar(frame, area, app);
    let line_style = Style::default().fg(if focused {
        app.palette.accent
    } else {
        app.palette.line
    });
    for y in inner.y..inner.bottom() {
        frame.buffer_mut().set_string(split, y, "┃", line_style);
    }
    frame
        .buffer_mut()
        .set_string(split, area.y, "┳", line_style);
    frame
        .buffer_mut()
        .set_string(split, area.bottom().saturating_sub(1), "┻", line_style);
}

fn draw_controller(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(if app.focus == Focus::Controller {
            app.palette.accent
        } else {
            app.palette.line
        }))
        .title(" Controller ")
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if area.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(input_line(app)).style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        ),
        inner,
    );
    if app.focus == Focus::Controller {
        frame.set_cursor_position((
            inner.x.saturating_add(input_width_before(app) as u16),
            inner.y,
        ));
    }
}

pub(crate) fn draw_library(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let inner = area;
    let lines: Vec<Line> = app
        .views
        .iter()
        .enumerate()
        .take(inner.height as usize)
        .map(|(index, view)| {
            let selected = index == app.view_index;
            let active = selected && app.focus == Focus::Browse;
            let depth = app.view_depth.get(index).copied().unwrap_or(0) as usize;
            let last_child = app
                .view_kind
                .get(index + 1)
                .is_none_or(|kind| !matches!(kind, BrowseKind::Playlist(_)));
            let marker = if selected { glyphs::SELECT } else { " " };
            let branch = if depth == 0 {
                ""
            } else if last_child {
                "└ "
            } else {
                "├ "
            };
            let indent = "  ".repeat(depth.saturating_sub(1));
            let mut text = format!("{marker} {indent}{branch}{}", view.title);
            if active {
                let width = display_width(&text);
                let pad = (inner.width as usize).saturating_sub(width);
                text.push_str(&" ".repeat(pad));
            }
            let style = if active {
                Style::default()
                    .bg(app.palette.fg)
                    .fg(app.palette.bg.unwrap_or(Color::Reset))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.palette.fg)
            };
            Line::from(TuiSpan::styled(
                truncate_width(&text, inner.width as usize),
                style,
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(crate) fn draw_list(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.width < 3 {
        return;
    }
    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    let Some(view) = app.selected_view() else {
        return;
    };
    if view.columns.is_empty() {
        frame.render_widget(
            Paragraph::new("暂无数据；: 命令执行 library list").style(
                Style::default()
                    .bg(app.palette.bg.unwrap_or(Color::Reset))
                    .fg(app.palette.muted),
            ),
            inner,
        );
        return;
    }
    let all_rows = app.filtered_rows();
    let keep: Vec<usize> = view
        .columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.as_str() != "来源")
        .map(|(index, _)| index)
        .collect();
    let columns: Vec<String> = keep
        .iter()
        .filter_map(|index| view.columns.get(*index).cloned())
        .collect();
    let rows: Vec<Vec<String>> = all_rows
        .iter()
        .map(|row| {
            keep.iter()
                .filter_map(|index| row.get(*index).cloned())
                .collect()
        })
        .collect();
    let widths = list_widths(&columns, &rows, inner.width as usize);
    let mut lines = Vec::new();
    lines.push(Line::styled(
        format_row(&columns, &widths),
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    ));
    let visible = inner.height.saturating_sub(1) as usize;
    let first = app.row_index.saturating_sub(visible.saturating_sub(1));
    for (index, row) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = app.focus == Focus::List && index == app.row_index;
        let style = if selected {
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
        } else {
            Style::default().fg(app.palette.fg)
        };
        lines.push(Line::styled(
            truncate_width(&format_row(row, &widths), inner.width as usize),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_list_scrollbar(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height < 4 || area.width < 2 {
        return;
    }
    let x = area.right().saturating_sub(1);
    let track_start = area.y.saturating_add(1);
    let track_end = area.bottom().saturating_sub(1);
    if track_end <= track_start {
        return;
    }
    let track_len = usize::from(track_end - track_start);
    let total = app.filtered_row_count();
    let visible = usize::from(area.height.saturating_sub(1));
    let track_style = Style::default().fg(app.palette.line);
    for y in track_start..track_end {
        frame.buffer_mut().set_string(x, y, "│", track_style);
    }
    if total <= visible {
        return;
    }
    let thumb_len = (track_len * visible / total).max(1).min(track_len);
    let first = app.row_index.saturating_sub(visible.saturating_sub(1));
    let max_first = total - visible;
    let offset = first * (track_len - thumb_len) / max_first.max(1);
    let thumb_start = track_start.saturating_add(offset as u16);
    let style = Style::default().fg(if app.focus == Focus::List {
        app.palette.accent
    } else {
        app.palette.muted
    });
    for offset in 0..thumb_len {
        frame
            .buffer_mut()
            .set_string(x, thumb_start.saturating_add(offset as u16), "█", style);
    }
}

pub(crate) fn draw_completion(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(app.palette.line))
        .title(" 候选 ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(completion) = &app.completion else {
        return;
    };
    let mut lines = Vec::new();
    for (index, item) in completion.items.iter().take(5).enumerate() {
        let selected = index == app.completion_index;
        let marker = if selected { glyphs::SELECT } else { " " };
        let text = format!("{marker} {}  {}", item.text, item.summary);
        let style = if selected {
            Style::default()
                .bg(app.palette.fg)
                .fg(app.palette.bg.unwrap_or(Color::Reset))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.palette.fg)
        };
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&text, inner.width as usize),
            style,
        )));
    }
    if let Some(item) = completion.items.get(app.completion_index) {
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&item.usage, inner.width as usize),
            Style::default().fg(app.palette.muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
