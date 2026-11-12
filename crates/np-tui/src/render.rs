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
    draw_status_border(frame, full, app);
}

fn draw_status_border(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height < 2 || area.width < 8 {
        return;
    }
    let y = area.bottom().saturating_sub(1);
    let version = format!(" v{} ", env!("CARGO_PKG_VERSION"));
    let version_width = display_width(&version) as u16;
    let version_x = area.right().saturating_sub(version_width + 1);
    frame.buffer_mut().set_string(
        version_x,
        y,
        &version,
        Style::default().fg(app.palette.faint),
    );

    let hints: &[(&str, &str)] = match app.focus {
        Focus::Controller => &[("←→", " 光标"), ("Enter", " 执行"), ("Esc", " 返回")],
        Focus::Browse => &[
            ("↑↓", " 选择"),
            ("←→", " 焦点"),
            ("Enter", " 进入"),
            ("/", " 命令"),
            ("q", " 退出"),
        ],
        Focus::List => &[
            ("↑↓", " 选择"),
            ("←→", " 焦点"),
            ("Enter", " 播放"),
            ("Space", " 暂停"),
            ("n/p", " 切歌"),
            ("/", " 命令"),
            ("q", " 退出"),
        ],
    };
    let mut x = area.x.saturating_add(2);
    let max_x = version_x.saturating_sub(1);
    for (key, description) in hints {
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

fn draw_browse_list(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let focused = app.focus != Focus::Controller;
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(if focused {
            app.palette.accent
        } else {
            app.palette.line
        }))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
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
    let split = inner
        .x
        .saturating_add(22)
        .min(inner.right().saturating_sub(1));
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
    let line_style = Style::default().fg(app.palette.line);
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
    let inner = area;
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
    let rows = app.filtered_rows();
    let mut widths = table_widths(&view.columns, &rows);
    let used: usize = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
    if used < inner.width as usize {
        let flex = view
            .columns
            .iter()
            .position(|column| column == "标题")
            .unwrap_or(1.min(widths.len().saturating_sub(1)));
        if flex < widths.len() {
            widths[flex] += inner.width as usize - used;
        }
    }
    let mut lines = Vec::new();
    lines.push(Line::styled(
        format_row(&view.columns, &widths),
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
                .add_modifier(Modifier::REVERSED)
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
