use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, Focus};
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::*;
use crate::keymap::{self, Action};
use crate::layout::{layout_areas, layout_inner};
use crate::model::{BrowseKind, SearchDisplay};
use crate::player_bar::draw_player_strip;
use crate::settings_page::SettingsKind;
use crate::theme::PaletteToken;
use crate::ui::chrome::{self, StatusHints, StatusState};
use crate::ui::scroll::ScrollState;

pub(crate) fn draw(frame: &mut ratatui::Frame, app: &mut App, player: &TuiAudio) {
    let full = frame.area();
    frame.render_widget(Clear, full);
    if let Some(bg) = app.palette.canvas() {
        frame.buffer_mut().set_style(full, Style::default().bg(bg));
    }
    if full.width < 4 || full.height < 6 {
        return;
    }
    // 直播独占整屏：不再画 Browse/List/Player 三区。
    if player.is_live() {
        crate::live_page::draw(frame, app, player);
        draw_controller_overlay(frame, full, app);
        player.draw_login(frame, &app.palette);
        return;
    }
    // UI 专注模式：隐藏 Browse/List，Player 占满内区。小终端回退普通布局，
    // 但状态栏仍给出 Ctrl+F 退出提示。
    if app.focus_mode && full.width >= 50 && full.height >= 12 {
        draw_focus(frame, full, app, player);
        return;
    }
    let outer_title = outer_title();
    chrome::draw_outer_chrome(frame, full, &app.palette, &outer_title, &app.theme_name);

    let [top, player_area, _] = layout_areas(full);

    draw_browse_list(frame, top, app);
    draw_player_strip(frame, player_area, app, player);
    draw_controller_overlay(frame, full, app);
    draw_status_border(frame, full, app);
    player.draw_login(frame, &app.palette);
}

fn outer_title() -> String {
    format!(" NOWHERE PLAYER v{} ", env!("CARGO_PKG_VERSION"))
}

/// 专注模式只留外框 + Player：Player 直接占用 `layout_inner` 的整个内区。
fn draw_focus(frame: &mut ratatui::Frame, full: Rect, app: &mut App, player: &TuiAudio) {
    let outer_title = outer_title();
    chrome::draw_outer_chrome(frame, full, &app.palette, &outer_title, &app.theme_name);
    draw_player_strip(frame, layout_inner(full), app, player);
    draw_controller_overlay(frame, full, app);
    draw_status_border(frame, full, app);
    player.draw_login(frame, &app.palette);
}

/// `/` 命令浮层：主界面和 Live 页共用。
fn draw_controller_overlay(frame: &mut ratatui::Frame, full: Rect, app: &App) {
    if app.focus != Focus::Controller {
        return;
    }
    let inner = layout_inner(full);
    let height = 3.min(inner.height);
    if height == 0 {
        return;
    }
    let overlay = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(height),
        width: inner.width,
        height,
    };
    frame.render_widget(Clear, overlay);
    draw_controller(frame, overlay, app);
    if app.completion.is_some() && overlay.y > inner.y + 3 {
        let popup_height = 7.min(overlay.y.saturating_sub(inner.y).saturating_sub(1));
        let width = 58.min(overlay.width.saturating_sub(4));
        let popup = Rect {
            x: overlay.x + 2,
            y: overlay.y.saturating_sub(popup_height),
            width,
            height: popup_height,
        };
        frame.render_widget(Clear, popup);
        draw_completion(frame, popup, app);
    }
}

fn draw_status_border(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.height < 2 || area.width < 8 {
        return;
    }
    let status = app.status.trim();
    let status = truncate_width(status, 24);
    let state = if !status.is_empty() && status != "就绪" {
        Some(StatusState {
            text: &status,
            warn: is_pending_status(app),
            require_space: true,
        })
    } else {
        None
    };
    let hints = status_hints(app);
    chrome::draw_status_bar(
        frame,
        area,
        &app.palette,
        state.as_ref(),
        StatusHints::Keyed(&hints),
    );
}

fn is_pending_status(app: &App) -> bool {
    app.pending_add.is_some()
        || app.pending_remove.is_some()
        || app.pending_playlist_delete.is_some()
        || app.quit_armed
        || app.playlist_input.is_some()
}

fn status_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let mut hints = base_status_hints(app);
    if app.focus_mode {
        // 专注模式（含小终端回退）第一项固定是退出提示。
        hints.insert(0, (keymap::label(Action::ToggleFocusMode), "退出专注"));
    }
    hints
}

fn base_status_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let enter = keymap::label(Action::Enter);
    let esc = keymap::label(Action::Escape);
    let tab = keymap::label(Action::NextSearchPlatform);
    let create = keymap::label(Action::CreatePlaylist);
    let delete = keymap::label(Action::DeletePlaylist);
    let add = keymap::label(Action::Add);
    let remove = keymap::label(Action::Remove);
    let reset = keymap::label(Action::ResetSetting);
    let clear = keymap::label(Action::ClearSearch);
    let search = keymap::label(Action::OpenSearch);
    let command = keymap::label(Action::OpenCommand);

    if app.focus == Focus::Controller {
        return vec![
            ("←→", "光标"),
            (tab, "补全"),
            ("↑↓", "历史"),
            (enter, "执行"),
            (esc, "返回"),
        ];
    }
    if app.settings_editor.is_editing() {
        return vec![("←→", "调整"), (enter, "保存"), (esc, "取消")];
    }
    if app.playlist_input.is_some() {
        return vec![(enter, "创建"), (esc, "取消")];
    }
    if app.pending_add.is_some() {
        return vec![(enter, "确认添加"), ("↑↓", "选择歌单"), (esc, "取消")];
    }
    if app.pending_remove.is_some() {
        return vec![(enter, "确认移除"), (esc, "取消")];
    }
    if app.pending_playlist_delete.is_some() {
        return vec![(enter, "确认删除"), (esc, "取消")];
    }
    if app.quit_armed {
        return vec![(enter, "确认退出"), (esc, "取消")];
    }
    if app.search_editing_input() {
        return vec![
            ("←→", "光标"),
            // `⌫` 实际占两格；字符串写两格空白，视觉上只留一格。
            ("⌫  ", "删除"),
            (tab, "平台"),
            (enter, "搜索"),
            (esc, "退出输入"),
        ];
    }

    let kind = app.view_kind.get(app.view_index);
    let list = app.focus == Focus::List;
    let mut hints = match kind {
        Some(BrowseKind::PlaylistRoot) if list => {
            vec![(create, "新建"), (delete, "删除"), (enter, "打开")]
        }
        Some(BrowseKind::PlaylistRoot) => {
            vec![(create, "新建"), (delete, "删除"), (enter, "进入")]
        }
        Some(BrowseKind::Playlist(_)) if list => {
            vec![
                (add, "添加"),
                (remove, "移除"),
                (enter, "播放"),
                (esc, "返回"),
            ]
        }
        Some(BrowseKind::Playlist(_)) => vec![(enter, "进入"), ("↑↓", "选择")],
        Some(BrowseKind::Queue) if list => {
            vec![(remove, "移除"), (enter, "播放"), (esc, "返回")]
        }
        Some(BrowseKind::Queue) => vec![(enter, "进入"), ("↑↓", "选择")],
        Some(BrowseKind::Library) if list => {
            vec![
                (add, "添加"),
                (remove, "移除"),
                (enter, "播放"),
                (esc, "返回"),
            ]
        }
        Some(BrowseKind::Library) => vec![(enter, "进入"), ("↑↓", "选择")],
        Some(BrowseKind::Search) if list => match app.search_display() {
            Some(SearchDisplay::Empty { .. }) => {
                vec![(enter, "输入"), (tab, "平台"), (esc, "返回")]
            }
            Some(SearchDisplay::Searching) => vec![(esc, "停止")],
            Some(SearchDisplay::NoResults) => vec![(clear, "清空"), (esc, "返回")],
            None => vec![
                (add, "添加"),
                (clear, "清空"),
                (enter, "播放"),
                (esc, "返回"),
            ],
        },
        Some(BrowseKind::Search) => vec![(enter, "进入"), (tab, "平台")],
        // 直播页和搜索页同一条节奏：List 未激活先提示 Enter 输入，激活后才显示
        // 房间号 / 历史 / Delete 的操作提示。
        Some(BrowseKind::Live) if list && app.live_entry.input_active => vec![
            ("", "输入房间号"),
            (enter, "开始"),
            ("↑↓", "历史"),
            ("Delete", "删除"),
            (esc, "返回"),
        ],
        Some(BrowseKind::Live) if list => vec![(enter, "输入"), ("↑↓", "选择"), (esc, "返回")],
        Some(BrowseKind::Live) => vec![(enter, "进入"), ("↑↓", "选择")],
        Some(BrowseKind::Settings) if list => vec![(enter, "编辑"), (reset, "重置"), (esc, "返回")],
        Some(BrowseKind::Settings) => vec![(enter, "进入"), ("↑↓", "选择")],
        None if list => vec![("↑↓", "选择"), (esc, "返回")],
        None => vec![("↑↓", "选择"), ("←→", "焦点")],
    };
    if app.focus == Focus::Browse && !matches!(kind, Some(BrowseKind::Search)) {
        hints.push((search, "搜索"));
    }
    hints.push((command, "命令"));
    hints
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
    let proportional = (u32::from(inner_width) * 23 / 100) as u16;
    proportional
        .clamp(15, 28)
        .min(inner_width.saturating_sub(20))
}

fn paint_browse_borders(
    frame: &mut ratatui::Frame,
    area: Rect,
    split: u16,
    focused: bool,
    app: &App,
) {
    let line = Style::default().fg(app.palette.color(PaletteToken::Border));
    let accent = Style::default().fg(app.palette.color(PaletteToken::Focus));
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
        .border_style(Style::default().fg(app.palette.color(PaletteToken::Border)))
        .style(
            Style::default()
                .bg(app.palette.color(PaletteToken::Canvas))
                .fg(app.palette.color(PaletteToken::Text)),
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
            app.palette.color(PaletteToken::Focus)
        } else {
            app.palette.color(PaletteToken::Muted)
        })
        .add_modifier(Modifier::BOLD);
    let list_title_style = Style::default()
        .fg(if app.focus == Focus::List {
            app.palette.color(PaletteToken::Focus)
        } else {
            app.palette.color(PaletteToken::Muted)
        })
        .add_modifier(Modifier::BOLD);
    border_text(frame, area, area.y, " Browse ", false, browse_title_style);
    let title_width = usize::from(
        area.right()
            .saturating_sub(1)
            .saturating_sub(split.saturating_add(1)),
    );
    let list_title = truncate_width(&format!(" {} ", list_title(app)), title_width);
    frame.buffer_mut().set_string(
        split.saturating_add(1),
        area.y,
        &list_title,
        list_title_style,
    );
    let view_number = if app.views.is_empty() {
        0
    } else {
        app.view_index + 1
    };
    let left_count = format!(" {view_number} of {} ", app.views.len());
    let count_style = Style::default()
        .fg(app.palette.color(PaletteToken::Faint))
        .add_modifier(Modifier::ITALIC);
    let left_count_x = split.saturating_sub(display_width(&left_count) as u16 + 1);
    frame.buffer_mut().set_string(
        left_count_x,
        area.bottom().saturating_sub(1),
        &left_count,
        count_style,
    );
    let bottom_y = area.bottom().saturating_sub(1);
    let total = format!(" total {} ", app.filtered_row_count());
    border_text(frame, area, bottom_y, &total, true, count_style);
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
        app.palette.color(PaletteToken::Focus)
    } else {
        app.palette.color(PaletteToken::Border)
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
            app.palette.color(PaletteToken::Focus)
        } else {
            app.palette.color(PaletteToken::Border)
        }))
        .title(" Controller ")
        .style(
            Style::default()
                .bg(app.palette.color(PaletteToken::Canvas))
                .fg(app.palette.color(PaletteToken::Text)),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if area.height < 2 {
        return;
    }
    let (line, cursor_x) = input_line(app, usize::from(inner.width));
    frame.render_widget(
        Paragraph::new(line).style(
            Style::default()
                .bg(app.palette.color(PaletteToken::Canvas))
                .fg(app.palette.color(PaletteToken::Text)),
        ),
        inner,
    );
    if app.focus == Focus::Controller {
        frame.set_cursor_position((inner.x.saturating_add(cursor_x as u16), inner.y));
    }
}

/// Browse 树：72×24 下内区 7 行，选中项始终落在窗口内。
pub(crate) fn draw_library(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let inner = area;
    let mut scroll = ScrollState::new(usize::from(inner.height));
    if scroll.visible == 0 {
        return;
    }
    scroll.follow(app.view_index, app.views.len());
    let lines: Vec<Line> = scroll
        .range(app.views.len())
        .map(|index| {
            let view = &app.views[index];
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
            let text_width = inner.width as usize;
            if active {
                let width = display_width(&text);
                let pad = text_width.saturating_sub(width);
                text.push_str(&" ".repeat(pad));
            }
            let style = if active {
                app.palette.selection_style().add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.palette.color(PaletteToken::Text))
            };
            Line::from(TuiSpan::styled(truncate_width(&text, text_width), style))
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
    if app.playlist_input.is_some() {
        draw_playlist_input(frame, inner, app);
        return;
    }
    // 直播页在搜索空态之前：它有自己的输入框 + 历史，不走 SearchDisplay。
    if matches!(app.view_kind.get(app.view_index), Some(BrowseKind::Live)) {
        crate::live_entry::draw(frame, inner, app);
        return;
    }
    if let Some(display) = app.search_display() {
        draw_search_state(frame, inner, app, display);
        return;
    }
    let Some(view) = app.selected_view() else {
        return;
    };
    if view.columns.is_empty() {
        frame.render_widget(
            Paragraph::new("暂无数据；: 命令执行 library list").style(
                Style::default()
                    .bg(app.palette.color(PaletteToken::Canvas))
                    .fg(app.palette.color(PaletteToken::Muted)),
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
    let mut widths = list_widths(&columns, &rows, inner.width as usize);
    let used: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    if used < inner.width as usize && widths.len() > 1 {
        widths[1] += inner.width as usize - used;
    }
    let mut lines = Vec::new();
    lines.push(Line::styled(
        format_row(&columns, &widths),
        Style::default()
            .fg(app.palette.color(PaletteToken::Accent))
            .add_modifier(Modifier::BOLD),
    ));
    // 列表扣掉表头：72×24 下 Settings 等列表可见 6 行。
    let mut scroll = ScrollState::new(usize::from(inner.height.saturating_sub(1)));
    scroll.follow(app.row_index, rows.len());
    let settings = matches!(
        app.view_kind.get(app.view_index),
        Some(BrowseKind::Settings)
    );
    for index in scroll.range(rows.len()) {
        let row = &rows[index];
        let selected = app.focus == Focus::List && index == app.row_index;
        let category = settings
            && app
                .settings_fields
                .get(index)
                .is_some_and(|field| field.kind == SettingsKind::Category);
        let style = if selected {
            app.palette.selection_style()
        } else if category {
            Style::default().fg(app.palette.color(PaletteToken::Accent))
        } else {
            Style::default().fg(app.palette.color(PaletteToken::Text))
        };
        let style = if category {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        lines.push(Line::styled(
            truncate_width(&format_row(row, &widths), inner.width as usize),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
    if matches!(app.view_kind.get(app.view_index), Some(BrowseKind::Queue))
        && let Some(marker) = app.queue_marker
        && marker >= scroll.first
        && marker < scroll.first.saturating_add(scroll.visible)
    {
        let y = inner.y.saturating_add(1 + (marker - scroll.first) as u16);
        frame.buffer_mut().set_string(
            area.x,
            y,
            glyphs::SELECT,
            Style::default().fg(app.palette.color(PaletteToken::Accent)),
        );
    }
}

fn list_title(app: &App) -> String {
    match app.view_kind.get(app.view_index) {
        Some(BrowseKind::PlaylistRoot) => "PlayList".to_owned(),
        Some(BrowseKind::Playlist(name)) => name.clone(),
        Some(BrowseKind::Library) => "Library".to_owned(),
        Some(BrowseKind::Search) => "Search".to_owned(),
        Some(BrowseKind::Live) => "Live".to_owned(),
        Some(BrowseKind::Queue) => "Queue".to_owned(),
        Some(BrowseKind::Settings) => "Settings".to_owned(),
        None => "List".to_owned(),
    }
}

fn draw_playlist_input(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    let Some(input) = app.playlist_input.as_ref() else {
        return;
    };
    let width = 38.min(area.width);
    let height = 3.min(area.height);
    let box_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.palette.color(PaletteToken::Accent)))
        .title_style(Style::default().fg(app.palette.color(PaletteToken::Accent)))
        .title(" New Playlist ")
        .style(Style::default().bg(app.palette.color(PaletteToken::Canvas)));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let content = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };
    let chars: Vec<char> = input.value.chars().collect();
    let cursor = input.cursor.min(chars.len());
    let width = usize::from(content.width);
    let (start, end) = input_window(&chars, cursor, width);
    let mut spans = Vec::new();
    for (index, character) in chars.iter().enumerate().take(end).skip(start) {
        if index == cursor {
            spans.push(TuiSpan::styled(
                character.to_string(),
                app.palette.selection_style(),
            ));
        } else {
            spans.push(TuiSpan::raw(character.to_string()));
        }
    }
    if cursor == chars.len() && cursor - start < width {
        spans.push(TuiSpan::styled(" ", app.palette.selection_style()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(
            Style::default()
                .fg(app.palette.color(PaletteToken::Text))
                .bg(app.palette.color(PaletteToken::Canvas)),
        ),
        content,
    );
    let prefix: String = chars.iter().take(cursor).skip(start).collect();
    frame.set_cursor_position((
        content.x.saturating_add(display_width(&prefix) as u16),
        content.y,
    ));
}

fn draw_search_state(frame: &mut ratatui::Frame, area: Rect, app: &App, display: SearchDisplay) {
    let bg = app.palette.color(PaletteToken::Canvas);
    match display {
        SearchDisplay::Empty { query } => {
            let width = 38.min(area.width);
            let height = 3.min(area.height);
            let box_area = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            };
            let active = app.search_editing_input();
            let block = TuiBlock::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if active {
                    app.palette.color(PaletteToken::Focus)
                } else {
                    app.palette.color(PaletteToken::Border)
                }))
                .title_style(Style::default().fg(if active {
                    app.palette.color(PaletteToken::Focus)
                } else {
                    app.palette.color(PaletteToken::Muted)
                }))
                .title(" Key Words ")
                .style(Style::default().bg(bg));
            let inner = block.inner(box_area);
            frame.render_widget(block, box_area);
            let content = Rect {
                x: inner.x.saturating_add(1),
                y: inner.y,
                width: inner.width.saturating_sub(2),
                height: inner.height,
            };
            let chars: Vec<char> = query.chars().collect();
            let cursor = app.search_cursor.min(chars.len());
            let width = usize::from(content.width);
            let (start, end) = input_window(&chars, cursor, width);
            let mut spans = Vec::new();
            for (index, character) in chars.iter().enumerate().take(end).skip(start) {
                if index == cursor && active {
                    spans.push(TuiSpan::styled(
                        character.to_string(),
                        app.palette.selection_style(),
                    ));
                } else {
                    spans.push(TuiSpan::raw(character.to_string()));
                }
            }
            if cursor == chars.len() && cursor - start < width && active {
                spans.push(TuiSpan::styled(" ", app.palette.selection_style()));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(
                    Style::default()
                        .fg(app.palette.color(PaletteToken::Text))
                        .bg(bg),
                ),
                content,
            );
            if active {
                let prefix: String = chars[start..cursor].iter().collect();
                let cursor_x = content.x.saturating_add(display_width(&prefix) as u16);
                frame.set_cursor_position((cursor_x, content.y));
            }
            // 平台名嵌在 Key Words 输入框的下边框，右对齐，前后各留一格。
            let platform = format!(" {}", app.search_platform.id_label());
            let platform_style = Style::default()
                .fg(app.palette.color(PaletteToken::Faint))
                .add_modifier(Modifier::ITALIC);
            border_text(
                frame,
                box_area,
                box_area.bottom().saturating_sub(1),
                &platform,
                true,
                platform_style,
            );
            // 标签后保留一格空白，再接右下角；不能把边框线直接贴在平台名后面。
            frame.buffer_mut().set_string(
                box_area.right().saturating_sub(2),
                box_area.bottom().saturating_sub(1),
                " ",
                Style::default().fg(app.palette.color(PaletteToken::Border)),
            );
        }
        SearchDisplay::Searching => {
            let frames = ["◰", "◳", "◲", "◱"];
            let frame_index = (crate::app::now_millis() / 120 % 4) as usize;
            let text = format!("{} searching...", frames[frame_index]);
            let y = area.y + area.height / 2;
            frame.render_widget(
                Paragraph::new(text)
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(app.palette.color(PaletteToken::Muted))
                            .bg(bg),
                    ),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
        }
        SearchDisplay::NoResults => {
            let y = area.y + area.height / 2;
            frame.render_widget(
                Paragraph::new("✘ no results")
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(app.palette.color(PaletteToken::Muted))
                            .bg(bg),
                    ),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
        }
    }
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
    let mut scroll = ScrollState::new(usize::from(area.height.saturating_sub(1)));
    let track_style = Style::default().fg(app.palette.color(PaletteToken::Border));
    for y in track_start..track_end {
        frame.buffer_mut().set_string(x, y, "│", track_style);
    }
    if total <= scroll.visible {
        return;
    }
    scroll.follow(app.row_index, total);
    let thumb_len = (track_len * scroll.visible / total).max(1).min(track_len);
    let first = scroll.first;
    let max_first = total - scroll.visible;
    let offset = first * (track_len - thumb_len) / max_first.max(1);
    let thumb_start = track_start.saturating_add(offset as u16);
    let style = Style::default().fg(if app.focus == Focus::List {
        app.palette.color(PaletteToken::Focus)
    } else {
        app.palette.color(PaletteToken::Muted)
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
        .border_style(Style::default().fg(app.palette.color(PaletteToken::Border)))
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
            app.palette.selection_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.palette.color(PaletteToken::Text))
        };
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&text, inner.width as usize),
            style,
        )));
    }
    if let Some(item) = completion.items.get(app.completion_index) {
        lines.push(Line::from(TuiSpan::styled(
            truncate_width(&item.usage, inner.width as usize),
            Style::default().fg(app.palette.color(PaletteToken::Muted)),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
