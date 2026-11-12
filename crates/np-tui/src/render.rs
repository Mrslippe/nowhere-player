use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Focus, UiMode};
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::*;
use crate::layout::vertical_heights;
use crate::player_bar::draw_player_strip;

pub(crate) fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio) {
    let full = frame.area();
    if let Some(bg) = app.palette.bg {
        frame.buffer_mut().set_style(full, Style::default().bg(bg));
    } else {
        frame.render_widget(Clear, full);
    }
    if full.width < 4 || full.height < 6 {
        return;
    }
    let outer = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.line))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    let inner = outer.inner(full);
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

    let (top_h, player_h, controller_h, _status_h) = vertical_heights(inner.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_h),
            Constraint::Length(player_h),
            Constraint::Length(controller_h),
            Constraint::Length(1),
        ])
        .split(inner);
    let (top, player_area, controller_area, status_area) = (rows[0], rows[1], rows[2], rows[3]);

    if app.immersive {
        draw_player_strip(frame, top.union(player_area), app, player);
    } else {
        draw_browse_list(frame, top, app);
        draw_player_strip(frame, player_area, app, player);
    }
    draw_controller(frame, controller_area, app);
    let hint = match (app.focus, app.mode) {
        (Focus::Controller, UiMode::Filter) => {
            "Filter: Enter 确认 · Esc 清除 · ↑/↓ 历史 · Tab 切焦点"
        }
        (Focus::Controller, _) => {
            "Enter 执行 · ↑/↓ 历史 · Esc 回列表 · Tab 切焦点 · ? 帮助 · q 退出"
        }
        (Focus::Browse, _) => {
            "j/k 切视图 · Enter 进入列表 · Tab 切焦点 · / 过滤 · : 命令 · ? 帮助 · q 退出"
        }
        (Focus::List, UiMode::Filter) => "Filter: Enter 确认 · Esc 清除 · / 输入过滤",
        (Focus::List, _) => {
            "j/k 移动 · Enter 播放 · a 入队 · d 删除(需 dd) · f 视觉 · Tab 切焦点 · / 过滤 · : 命令"
        }
    };
    let status = Paragraph::new(format!(" {} · {}", app.status, hint)).style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.muted),
    );
    frame.render_widget(status, status_area);
    if app.mode == UiMode::Help {
        draw_help(frame, inner, app);
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
    border_text(
        frame,
        area,
        area.y,
        " Browse ",
        false,
        Style::default().fg(app.palette.accent),
    );
    border_text(
        frame,
        area,
        area.y,
        " List ",
        true,
        Style::default().fg(app.palette.accent),
    );
    let view_number = if app.views.is_empty() {
        0
    } else {
        app.view_index + 1
    };
    let left_count = format!(" {view_number} of {} ", app.views.len());
    let right_count = format!(" total {} ", app.filtered_row_count());
    border_text(
        frame,
        area,
        area.bottom().saturating_sub(1),
        &left_count,
        false,
        Style::default().fg(app.palette.muted),
    );
    border_text(
        frame,
        area,
        area.bottom().saturating_sub(1),
        &right_count,
        true,
        Style::default().fg(app.palette.muted),
    );
    let split = inner
        .x
        .saturating_add(22)
        .min(inner.right().saturating_sub(1));
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
        frame.buffer_mut().set_string(split, y, "│", line_style);
    }
    frame
        .buffer_mut()
        .set_string(split, area.y, "┬", line_style);
    frame
        .buffer_mut()
        .set_string(split, area.bottom().saturating_sub(1), "┴", line_style);
}

fn draw_controller(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let block = TuiBlock::default()
        .borders(Borders::ALL)
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

pub(crate) fn draw_help(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(8).clamp(40, 64);
    let height = 18.min(area.height.saturating_sub(2));
    let overlay = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    if app.palette.bg.is_some() && area.width >= 60 {
        let shadow = Rect {
            x: overlay.x.saturating_add(2),
            y: overlay.y.saturating_add(1),
            width: overlay.width.min(area.width.saturating_sub(overlay.x + 2)),
            height: overlay
                .height
                .min(area.height.saturating_sub(overlay.y + 1)),
        };
        frame.render_widget(
            TuiBlock::default().style(Style::default().bg(Color::Black)),
            shadow,
        );
    }
    frame.render_widget(Clear, overlay);
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.accent))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .title(" 帮助 · 任意键关闭 ");
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);
    let lines = [
        "Space 播放/暂停   n/p 切歌   +/- 音量",
        "Tab 切焦点   j/k 移动   gg/G 首尾",
        "Enter 播放选中   a 入队   d 删除(需 dd)",
        "f 视觉模式   Ctrl+F 沉浸   / 过滤",
        ": 命令   ? 帮助   q 退出(播放中二次确认)",
        "",
        "Filter: Enter 确认 · Esc 清除",
        "Command: Enter 执行 · ↑/↓ 历史 · Esc 取消",
    ]
    .join("\n");
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(crate) fn panel_block(app: &App, title: &str, focused: bool) -> TuiBlock<'static> {
    TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            app.palette.accent
        } else {
            app.palette.faint
        }))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .title(title.to_owned())
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
            let marker = if selected { glyphs::SELECT } else { " " };
            let style = if selected {
                Style::default()
                    .bg(app.palette.fg)
                    .fg(app.palette.bg.unwrap_or(Color::Reset))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.palette.fg)
            };
            Line::from(TuiSpan::styled(
                truncate_width(&format!("{marker} {}", view.title), inner.width as usize),
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

pub(crate) fn draw_completion(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = TuiBlock::default()
        .borders(Borders::ALL)
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

pub(crate) fn draw_output(frame: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let lines = all_lines(app);
    let output_height = area.height.saturating_sub(2);
    let max_scroll = lines.len().saturating_sub(output_height as usize) as u16;
    if app.auto_scroll {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
    }
    let output = Paragraph::new(lines)
        .block(
            TuiBlock::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.palette.line))
                .title(" 输出 "),
        )
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(output, area);
}
