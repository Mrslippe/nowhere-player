use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Focus, UiMode};
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::*;
use crate::layout::player_height_for;
use crate::player_bar::draw_player_strip;

pub(crate) fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio) {
    let full = frame.area();
    if let Some(bg) = app.palette.bg {
        frame.buffer_mut().set_style(full, Style::default().bg(bg));
    } else {
        frame.render_widget(Clear, full);
    }
    // 主题模式自绘 1 格留白；跟随终端模式绝不覆盖用户终端背景。
    let area = if app.palette.bg.is_some() && full.width > 2 && full.height > 2 {
        Rect {
            x: full.x + 1,
            y: full.y + 1,
            width: full.width - 2,
            height: full.height - 2,
        }
    } else {
        full
    };

    let player_height = player_height_for(area.height);
    let input_height = u16::from(matches!(app.mode, UiMode::Command | UiMode::Filter));
    let body_height = area
        .height
        .saturating_sub(1 + player_height + input_height + 1)
        .max(1);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(body_height),
            Constraint::Length(player_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        TuiSpan::styled(
            " Nowhere Player ",
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        TuiSpan::raw(format!("  {}", app.theme_name)),
    ]))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.fg),
    );
    frame.render_widget(title, rows[0]);

    if app.immersive {
        draw_player_strip(frame, rows[1].union(rows[2]), app, player);
    } else {
        if area.width < 80 {
            draw_list(frame, rows[1], app);
        } else {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(22), Constraint::Min(20)])
                .split(rows[1]);
            draw_library(frame, columns[0], app);
            draw_list(frame, columns[1], app);
        }
        draw_player_strip(frame, rows[2], app, player);
    }

    if matches!(app.mode, UiMode::Command | UiMode::Filter) {
        let title = if app.mode == UiMode::Filter {
            " 过滤 "
        } else {
            " 命令 "
        };
        let _ = title;
        let input = Paragraph::new(input_line(app)).style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
        frame.render_widget(input, rows[3]);
        frame.set_cursor_position((
            rows[3].x.saturating_add(input_width_before(app) as u16),
            rows[3].y,
        ));
    }

    let hint = if app.immersive {
        "Ctrl+F 退出沉浸 · Space 播放/暂停 · n/p 切歌 · +/- 音量 · Esc 返回"
    } else {
        match (app.focus, app.mode) {
            (Focus::Library, _) => {
                "j/k 切视图 · Enter 进入列表 · Tab 切焦点 · / 过滤 · : 命令 · ? 帮助 · q 退出"
            }
            (Focus::List, _) => {
                "j/k 移动 · Enter 播放 · a 入队 · d 删除(需 dd) · f 视觉 · Tab 切焦点 · / 过滤 · : 命令 · ? 帮助"
            }
        }
    };
    let status = Paragraph::new(format!(
        " {} · {} · {} · NO_COLOR {}",
        app.selected_view()
            .map(|view| view.title.as_str())
            .unwrap_or("无视图"),
        app.status,
        hint,
        if app.no_color_ignored {
            "已忽略"
        } else {
            "未设置"
        }
    ))
    .style(
        Style::default()
            .bg(app.palette.bg.unwrap_or(Color::Reset))
            .fg(app.palette.muted),
    );
    frame.render_widget(status, rows[4]);
    if app.mode == UiMode::Help {
        draw_help(frame, area, app);
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
    let block = panel_block(app, " 视图 ", app.focus == Focus::Library);
    let inner = block.inner(area);
    frame.render_widget(block, area);
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
    let view = app.selected_view();
    let title = match view {
        Some(view) => format!(" {} · {} ", view.title, view.rows.len()),
        None => " 条目 ".to_owned(),
    };
    let block = panel_block(app, &title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(view) = view else {
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
