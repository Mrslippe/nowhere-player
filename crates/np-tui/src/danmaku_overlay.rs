//! Live 页右侧弹幕浮层（T-0100b / T-0100b2）。
//!
//! 消息源是 [`TuiAudio`] 的 500 条环形缓冲；本模块只负责窗口选择、控制字符
//! 清理、按显示宽度换行与绘制，不建立连接，也不改变 `c` 输入框的状态机。
//!
//! T-0100b2 起浮层改为覆盖式：视觉区仍按 66×19 完整绘制，浮层叠在右侧；
//! 左侧频谱不再被压缩。T-0100b3 起浮层为 34×18、落在 x36,y2：左缘 `┃`、
//! 上下横线与 `┣ 输入 ━` 分隔都用粗线，右侧只在右上 `┛`、右下 `┓` 收口；
//! 消息行与 Input 行右侧均不画竖框。

use std::cell::Cell;
use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::audio::TuiAudio;
use crate::input::{display_width, truncate_width};
use crate::live_page::LiveLayout;
use crate::model::Focus;
use crate::theme::{Palette, PaletteToken};

/// 浮层宽度：34 = 左框 1 + 内容 32 + 右上 / 右下角 1；72×24 下占 x36..x69。
pub(crate) const OVERLAY_WIDTH: u16 = 34;
/// 浮层最大高度：72×24 下 y2..y19 共 18 行，信息行 y20 不被覆盖。
pub(crate) const OVERLAY_MAX_HEIGHT: u16 = 18;
/// 标题文字；`弹幕` 左右各 1 空格（T-0101）。
pub(crate) const TITLE_TEXT: &str = " 弹幕 ";
/// Input 分隔行标签；`输入` 左右各 1 空格（T-0101）。
pub(crate) const INPUT_LABEL: &str = " 输入 ";
/// 空态文案。
pub(crate) const EMPTY_TEXT: &str = "暂无弹幕";
/// 未读数最多显示到 99+。
const MAX_UNREAD_LABEL: u64 = 99;
/// 同接数（≤9999）固定 4 格；两侧各 1 空格由 [`online_text`] 补。
const ONLINE_CELL_WIDTH: usize = 4;
/// 浮层固定占用的行数：顶边 1 + 分隔 1 + 输入 1 + 底边 1。
const CHROME_ROWS: u16 = 4;
/// 浮层最小高度：至少保留 1 行消息 + 三行 Input。
const OVERLAY_MIN_HEIGHT: u16 = CHROME_ROWS + 1;
/// 终端高度低于 10 行时完全不绘制弹幕栏，回退到普通 Live 页。
pub(crate) const DANMAKU_MIN_SCREEN_HEIGHT: u16 = 10;

/// 往环形缓冲追加一条；超过容量先丢最旧的一条。
pub(crate) fn push_recent(
    recent: &mut VecDeque<np_live::DanmakuMessage>,
    message: np_live::DanmakuMessage,
) {
    if recent.len() >= crate::audio::DANMAKU_BUFFER_CAPACITY {
        recent.pop_front();
    }
    recent.push_back(message);
}

impl TuiAudio {
    /// 最近收到的弹幕（旧 → 新，最多 500 条）；浮层只读它。
    pub(crate) fn danmaku_messages(&self) -> Vec<np_live::DanmakuMessage> {
        self.danmaku_recent.borrow().iter().cloned().collect()
    }

    /// 单调递增的收包计数；浮层用它算未读与滚动窗口。
    pub(crate) fn danmaku_received(&self) -> u64 {
        self.danmaku_received.get()
    }
}

/// 弹幕浮层的 UI 状态；不持有消息本体（消息留在 [`TuiAudio`]）。
#[derive(Debug)]
pub(crate) struct DanmakuOverlay {
    visible: bool,
    /// 输入行是否聚焦；`d` / `c` 打开时自动置位，`Esc` 清除。
    input_focused: bool,
    /// 距最新消息的偏移（单位：换行后的显示行）；0 表示跟随底部。
    offset_from_bottom: usize,
    /// TuiAudio 的收包计数（单调递增）。
    total: u64,
    /// 已经看到的消息计数；`total - seen_total` 即未读。
    seen_total: u64,
    /// 上一次绘制时的正文行数，滚动上限按它计算。
    body_rows: Cell<usize>,
    /// 上一次绘制时换行后的总行数。
    total_lines: Cell<usize>,
    /// 终端高度（行）；<10 时整个弹幕栏禁用。
    screen_height: u16,
    /// Live 页退出确认态：`Esc` 置位，`Enter` 停播或确认态 `Esc` 清除。
    /// 停播走 [`Self::reset`] 时一并归零。
    exit_armed: bool,
}

impl Default for DanmakuOverlay {
    fn default() -> Self {
        Self {
            visible: false,
            input_focused: false,
            offset_from_bottom: 0,
            total: 0,
            seen_total: 0,
            body_rows: Cell::new(0),
            total_lines: Cell::new(0),
            screen_height: 24,
            exit_armed: false,
        }
    }
}

impl DanmakuOverlay {
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn input_focused(&self) -> bool {
        self.input_focused
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset_from_bottom
    }

    pub(crate) fn unread(&self) -> u64 {
        self.total.saturating_sub(self.seen_total)
    }

    pub(crate) fn screen_height(&self) -> u16 {
        self.screen_height
    }

    pub(crate) fn set_screen_height(&mut self, height: u16) {
        self.screen_height = height;
    }

    /// 打开浮层并清空未读，但不聚焦输入（`d`）；已打开时保持不动。
    pub(crate) fn show(&mut self) {
        if !self.visible {
            self.visible = true;
        }
        self.offset_from_bottom = 0;
        self.seen_total = self.total;
    }

    /// 关闭浮层并释放焦点。
    pub(crate) fn hide(&mut self) {
        self.visible = false;
        self.input_focused = false;
        self.offset_from_bottom = 0;
    }

    pub(crate) fn focus_input(&mut self) {
        self.visible = true;
        self.input_focused = true;
    }

    pub(crate) fn blur_input(&mut self) {
        self.input_focused = false;
    }

    /// Live 页退出确认态。
    pub(crate) fn exit_armed(&self) -> bool {
        self.exit_armed
    }

    pub(crate) fn arm_exit(&mut self) {
        self.exit_armed = true;
    }

    pub(crate) fn clear_exit(&mut self) -> bool {
        let armed = self.exit_armed;
        self.exit_armed = false;
        armed
    }

    /// 同步消息计数；返回未读或总数是否变化（需要重绘）。
    pub(crate) fn sync(&mut self, total: u64) -> bool {
        let before = (self.total, self.unread());
        self.total = total;
        // 只有浮层可见且停在底部时，新消息才算“已经看到”。
        if self.visible && self.offset_from_bottom == 0 {
            self.seen_total = total;
        }
        (self.total, self.unread()) != before
    }

    /// 向上滚动一行；已经在顶部时返回 `false`。
    pub(crate) fn scroll_up(&mut self) -> bool {
        let rows = self.body_rows.get().max(1);
        let total = self.total_lines.get();
        let max_offset = total.saturating_sub(rows);
        if self.offset_from_bottom >= max_offset {
            return false;
        }
        self.offset_from_bottom += 1;
        true
    }

    /// 向下滚动一行；回到底部即恢复跟随并清零未读。
    pub(crate) fn scroll_down(&mut self) -> bool {
        if self.offset_from_bottom == 0 {
            return false;
        }
        self.offset_from_bottom -= 1;
        if self.offset_from_bottom == 0 {
            self.seen_total = self.total;
        }
        true
    }

    /// PgUp / PgDn：按整页滚动；已在顶部 / 底部时返回 `false`。
    pub(crate) fn scroll_page(&mut self, up: bool) -> bool {
        let rows = self.body_rows.get().max(1);
        let total = self.total_lines.get();
        let max_offset = total.saturating_sub(rows);
        let target = if up {
            self.offset_from_bottom.saturating_add(rows).min(max_offset)
        } else {
            self.offset_from_bottom.saturating_sub(rows)
        };
        if target == self.offset_from_bottom {
            return false;
        }
        self.offset_from_bottom = target;
        if target == 0 {
            self.seen_total = self.total;
        }
        true
    }

    /// `f`：回到跟随最新，并清零未读。
    pub(crate) fn follow(&mut self) -> bool {
        let changed = self.offset_from_bottom != 0 || self.unread() != 0;
        self.offset_from_bottom = 0;
        self.seen_total = self.total;
        changed
    }

    /// 停播 / 换房时清空浮层状态（保留终端高度）。
    pub(crate) fn reset(&mut self) {
        let screen_height = self.screen_height;
        *self = Self::default();
        self.screen_height = screen_height;
    }

    /// 记录上次绘制出的换行总行数与正文行数，供滚动上限计算。
    pub(crate) fn set_scroll_metrics(&self, total_lines: usize, rows: usize) {
        self.total_lines.set(total_lines);
        self.body_rows.set(rows);
    }
}

/// 当前窗口 `(start, end, 渲染行数)`：始终贴底，向上偏移不超过历史行数。
pub(crate) fn message_window(total: usize, rows: usize, offset: usize) -> (usize, usize, usize) {
    let rows = rows.min(total);
    let max_offset = total.saturating_sub(rows);
    let offset = offset.min(max_offset);
    let start = total - rows - offset;
    (start, start + rows, rows)
}

/// 浮层区域：72×24 下为 x36,y2,34×18；右缘与 Player 框右缘同列（x69），
/// 内容区 x37..x68 共 32 列。小尺寸只收缩宽度 / 高度，不 panic。
///
/// 常规情况高度比视觉区少 1，避免盖住最下面的 Information 行；当视觉区只有
/// 5 行（40×10）时用满 5 行，保证 1 行消息 + 三行 Input 仍然存在。
pub(crate) fn area(layout: &LiveLayout) -> Option<Rect> {
    if layout.outer.height < DANMAKU_MIN_SCREEN_HEIGHT {
        return None;
    }
    let visual = layout.visual;
    if visual.width < 16 || visual.height < OVERLAY_MIN_HEIGHT {
        return None;
    }
    let width = OVERLAY_WIDTH.min(visual.width.saturating_sub(8));
    if width < 16 {
        return None;
    }
    let preferred = OVERLAY_MAX_HEIGHT.min(visual.height.saturating_sub(1));
    let height = if preferred >= OVERLAY_MIN_HEIGHT {
        preferred
    } else {
        OVERLAY_MAX_HEIGHT.min(visual.height)
    };
    if height < OVERLAY_MIN_HEIGHT {
        return None;
    }
    Some(Rect::new(
        layout.player.right().saturating_sub(width),
        visual.y,
        width,
        height,
    ))
}

/// 标题行右侧的同接数：≤9999 固定 4 格；>9999 用 `w`（如 `1.2w`）。
///
/// 只截断到 0.1w（不四舍五入），保证 10000..=99999 恰好 4 格；更大的数自然
/// 变成 `10.0w` / `123.4w`，由调用方按实际显示宽度右对齐，不覆盖右上角。
pub(crate) fn online_label(online: Option<u64>) -> String {
    match online {
        Some(value) if value > 9999 => {
            let label = format!("{}.{}w", value / 10_000, (value % 10_000) / 1_000);
            format!(
                "{label:>width$}",
                width = ONLINE_CELL_WIDTH.max(label.len())
            )
        }
        Some(value) => format!("{value:>width$}", width = ONLINE_CELL_WIDTH),
        None => " -- ".to_owned(),
    }
}

/// 标题行右侧完整块：两侧各 1 空格。
pub(crate) fn online_text(online: Option<u64>) -> String {
    format!(" {} ", online_label(online))
}

/// 浮层键只在 Live 普通态接管。
///
/// 命令输入（`Focus::Controller`）与歌单命名输入必须优先：否则 `f` 会被当成
/// 跟随而不是输入字符，`↑/↓` 会滚弹幕而不是翻历史 / 补全。聊天输入框、
/// 搜索与设置编辑在事件循环里排得更前，不在这里重复判断。
pub(crate) fn keys_owned(app: &App) -> bool {
    app.danmaku_overlay_visible()
        && !app.danmaku_overlay.input_focused()
        && app.focus != Focus::Controller
        && !app.playlist_input_active()
}

/// 浮层可见时的按键：↑/↓ 滚动、PgUp/PgDn 翻页、`f` 跟随；`d` / `c` 由
/// keymap 处理。
///
/// 返回 `true` 表示按键已被浮层消费，即使已经滚到边界也不再落到全局键位。
pub(crate) fn handle_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Up => {
            app.scroll_danmaku(true);
            true
        }
        KeyCode::Down => {
            app.scroll_danmaku(false);
            true
        }
        KeyCode::PageUp => {
            app.scroll_danmaku_page(true);
            true
        }
        KeyCode::PageDown => {
            app.scroll_danmaku_page(false);
            true
        }
        KeyCode::Char('f') => {
            app.follow_danmaku();
            true
        }
        _ => false,
    }
}

/// 绘制浮层：铺底 → 边框 / 分隔线 → 标题与同接 → 换行消息 → 输入行。
///
/// 边框按卡面 T-0100b3 §0：顶行 `┏ ━ ┛`、底行 `┗ ━ ┓`、Input 分隔 `┣ 输入 ━`
/// 全部用粗线；消息行与 Input 行右侧完全开口。
pub(crate) fn draw(frame: &mut ratatui::Frame, area: Rect, app: &App, player: &TuiAudio) {
    if area.width < 8 || area.height < CHROME_ROWS {
        return;
    }
    let palette = &app.palette;
    let bg = palette.color(PaletteToken::Canvas);
    let fill = " ".repeat(usize::from(area.width));
    for y in area.y..area.bottom() {
        frame
            .buffer_mut()
            .set_string(area.x, y, &fill, Style::default().bg(bg));
    }

    let border = Style::default()
        .fg(palette.color(PaletteToken::BorderStrong))
        .bg(bg);
    let right = area.right().saturating_sub(1);
    // 只画左竖边；消息行与 Input 行右侧保持空白。
    for y in area.y.saturating_add(1)..area.bottom() {
        frame.buffer_mut().set_string(area.x, y, "┃", border);
    }

    // 横向粗线；上下各 1 个转角，分隔线没有右端。
    let horizontal = "━".repeat(usize::from(area.width.saturating_sub(2)));

    // 顶边：`┏` + `━` + `┛`。
    frame.buffer_mut().set_string(area.x, area.y, "┏", border);
    frame.buffer_mut().set_string(right, area.y, "┛", border);
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), area.y, &horizontal, border);

    // 底边：`┗` + `━` + `┓`。
    let bottom = area.bottom().saturating_sub(1);
    frame.buffer_mut().set_string(area.x, bottom, "┗", border);
    frame.buffer_mut().set_string(right, bottom, "┓", border);
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), bottom, &horizontal, border);

    // Input 分隔线：`┣ 输入 ━`，右端不画 `┫`。
    let separator_y = area.y.saturating_add(area.height).saturating_sub(3);
    frame
        .buffer_mut()
        .set_string(area.x, separator_y, "┣", border);
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), separator_y, &horizontal, border);
    // 标签与标题同风格：`┣` 后 1 空格 + accent + BOLD，横线在标签处让位。
    frame.buffer_mut().set_string(
        area.x.saturating_add(1),
        separator_y,
        INPUT_LABEL,
        Style::default()
            .fg(palette.color(PaletteToken::Accent))
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    draw_title(frame, area, app, player);

    let message_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(CHROME_ROWS),
    };
    draw_messages(frame, message_area, app, player);

    let input_area = Rect {
        x: area.x.saturating_add(1),
        y: bottom.saturating_sub(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    crate::chat_input::draw_input(frame, input_area, app);
    // Input 行提示符是 ASCII `>`（x37），x38 留空、x39 起是输入内容；
    // 右侧完全开口，不画竖线。
    frame.buffer_mut().set_string(
        input_area.x,
        input_area.y,
        ">",
        Style::default()
            .fg(palette.color(PaletteToken::Accent))
            .bg(bg),
    );
}

fn draw_title(frame: &mut ratatui::Frame, area: Rect, app: &App, player: &TuiAudio) {
    let palette = &app.palette;
    let bg = palette.color(PaletteToken::Canvas);
    let right = area.right().saturating_sub(1);
    let title_x = area.x.saturating_add(1);
    frame.buffer_mut().set_string(
        title_x,
        area.y,
        TITLE_TEXT,
        Style::default()
            .fg(palette.color(PaletteToken::Accent))
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );

    let online = online_text(player.live_online());
    let online_x = right.saturating_sub(display_width(&online) as u16);
    frame.buffer_mut().set_string(
        online_x,
        area.y,
        &online,
        Style::default()
            .fg(palette.color(PaletteToken::Muted))
            .bg(bg),
    );

    let label = title_label(app.danmaku_overlay.offset(), app.danmaku_overlay.unread());
    if label.is_empty() {
        return;
    }
    let label_x = title_x
        .saturating_add(display_width(TITLE_TEXT) as u16)
        .saturating_add(1);
    let available = online_x.saturating_sub(label_x).saturating_sub(1) as usize;
    if available < 2 {
        return;
    }
    let label = truncate_width(&label, available);
    if label.is_empty() {
        return;
    }
    let style = if app.danmaku_overlay.unread() > 0 {
        Style::default()
            .fg(palette.color(PaletteToken::Warn))
            .bg(bg)
    } else {
        Style::default()
            .fg(palette.color(PaletteToken::Muted))
            .bg(bg)
    };
    frame
        .buffer_mut()
        .set_string(label_x, area.y, &label, style);
}

/// 标题行中间：滚动时显示 `↑/↓`，有未读时显示计数（最多 99+）。
fn title_label(offset: usize, unread: u64) -> String {
    let mut parts = Vec::new();
    if offset > 0 {
        parts.push("↑/↓".to_owned());
    }
    if unread > 0 {
        let unread = if unread > MAX_UNREAD_LABEL {
            format!("{MAX_UNREAD_LABEL}+")
        } else {
            unread.to_string()
        };
        parts.push(unread);
    }
    parts.join(" ")
}

/// 一行已经换好行的弹幕；`segments` 里 `true` 表示用户名（accent），
/// `own` 表示发送者 uid 与当前登录账号一致（整行 accent + bold）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WrappedLine {
    pub(crate) own: bool,
    pub(crate) segments: Vec<(bool, String)>,
}

impl WrappedLine {
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        self.segments
            .iter()
            .map(|(_, text)| text.as_str())
            .collect()
    }

    #[cfg(test)]
    fn width(&self) -> usize {
        display_width(&self.text())
    }
}

/// 把一条弹幕按显示宽度换行；用户名只出现在第一行，续行不缩进、不省略。
pub(crate) fn wrap_message_lines(
    message: &np_live::DanmakuMessage,
    width: usize,
    own_uid: Option<u64>,
) -> Vec<WrappedLine> {
    let width = width.max(1);
    let user = sanitize(&message.user);
    let user = if user.is_empty() {
        "匿名".to_owned()
    } else {
        user
    };
    let prefix = format!("{user}: ");
    let text = sanitize(&message.text);
    // 只有双方都有 uid 且相等才算自己；缺失 uid 时保持普通样式。
    let own = message
        .uid
        .zip(own_uid)
        .is_some_and(|(message_uid, own_uid)| message_uid == own_uid);

    let mut lines = vec![WrappedLine {
        own,
        ..WrappedLine::default()
    }];
    let mut used = 0usize;
    for (accent, value) in [(true, prefix.as_str()), (false, text.as_str())] {
        for character in value.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if character_width == 0 || character_width > width {
                continue;
            }
            if used > 0 && used + character_width > width {
                lines.push(WrappedLine {
                    own,
                    ..WrappedLine::default()
                });
                used = 0;
            }
            let line = lines.last_mut().expect("at least one line");
            match line.segments.last_mut() {
                Some((last_accent, text)) if *last_accent == accent => {
                    text.push(character);
                }
                _ => line.segments.push((accent, character.to_string())),
            }
            used += character_width;
        }
    }
    lines
}

/// 把全部消息展开成显示行，供窗口选择与滚动使用。
pub(crate) fn wrap_messages(
    messages: &[np_live::DanmakuMessage],
    width: usize,
    own_uid: Option<u64>,
) -> Vec<WrappedLine> {
    let mut lines = Vec::new();
    for message in messages {
        lines.extend(wrap_message_lines(message, width, own_uid));
    }
    lines
}

fn draw_messages(frame: &mut ratatui::Frame, area: Rect, app: &App, player: &TuiAudio) {
    let rows = usize::from(area.height);
    if area.width < 2 {
        app.danmaku_overlay.set_scroll_metrics(0, rows);
        return;
    }
    let palette = &app.palette;
    let bg = palette.color(PaletteToken::Canvas);
    let messages = player.danmaku_messages();
    let width = usize::from(area.width);
    let lines = wrap_messages(&messages, width, player.own_bili_uid());
    app.danmaku_overlay.set_scroll_metrics(lines.len(), rows);
    if lines.is_empty() {
        if area.height == 0 {
            return;
        }
        let text = truncate_width(EMPTY_TEXT, width);
        let x = area
            .x
            .saturating_add(area.width.saturating_sub(display_width(&text) as u16) / 2);
        let y = area.y.saturating_add(area.height / 2);
        frame.buffer_mut().set_string(
            x,
            y,
            &text,
            Style::default()
                .fg(palette.color(PaletteToken::Faint))
                .bg(bg),
        );
        return;
    }

    let (start, end, rows) = message_window(lines.len(), rows, app.danmaku_overlay.offset());
    let first_y = area.y.saturating_add(
        area.height
            .saturating_sub(u16::try_from(rows).unwrap_or(u16::MAX)),
    );
    for (row, line) in lines[start..end].iter().enumerate() {
        let y = first_y.saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
        draw_line(frame, area.x, y, line, palette, bg);
    }
}

fn draw_line(
    frame: &mut ratatui::Frame,
    x: u16,
    y: u16,
    line: &WrappedLine,
    palette: &Palette,
    bg: Color,
) {
    let mut cursor = x;
    for (accent, text) in &line.segments {
        let style = if line.own {
            Style::default()
                .fg(palette.color(PaletteToken::Accent))
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if *accent {
            Style::default()
                .fg(palette.color(PaletteToken::Accent))
                .bg(bg)
        } else {
            Style::default()
                .fg(palette.color(PaletteToken::Text))
                .bg(bg)
        };
        frame.buffer_mut().set_string(cursor, y, text, style);
        cursor = cursor.saturating_add(display_width(text) as u16);
    }
}

/// 清理弹幕文本：控制字符 / 换行变单个空格，去掉零宽与双向控制符。
pub(crate) fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut space_pending = false;
    for character in value.chars() {
        if is_format_char(character) {
            continue;
        }
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            space_pending = true;
            continue;
        }
        if space_pending {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            space_pending = false;
        }
        out.push(character);
    }
    out.trim().to_owned()
}

/// 零宽、双向与变体选择符：宽度为 0 或会改变相邻字形宽度，统一剥掉。
fn is_format_char(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{20D0}'..='\u{20F0}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

impl App {
    // ---- Live 页弹幕浮层（T-0100b / T-0100b2） ----

    pub(crate) fn danmaku_overlay_visible(&self) -> bool {
        self.danmaku_overlay.visible()
    }

    /// 同步收包计数；返回需要重绘（未读或总数变化）。
    pub(crate) fn sync_danmaku(&mut self, total: u64) -> bool {
        self.danmaku_overlay.sync(total)
    }

    /// 同步终端高度；低于弹幕栏下限时关闭浮层与输入焦点并请求重绘。
    pub(crate) fn set_viewport_height(&mut self, height: u16) -> bool {
        let changed = self.danmaku_overlay.screen_height() != height;
        self.danmaku_overlay.set_screen_height(height);
        if height < DANMAKU_MIN_SCREEN_HEIGHT
            && (self.danmaku_overlay_visible() || self.chat_input.is_some())
        {
            self.reset_danmaku_overlay();
            return true;
        }
        changed
    }

    /// `d`：打开浮层并聚焦 Input（T-0101 起 `d` 只负责打开，关闭交给 `Esc`）。
    ///
    /// 输入焦点存在时 `d` 是普通字符（事件循环先走 chat 分支）。
    pub(crate) fn toggle_danmaku_overlay(&mut self, total: u64) {
        if self.danmaku_overlay.screen_height() < DANMAKU_MIN_SCREEN_HEIGHT {
            return;
        }
        self.danmaku_overlay.sync(total);
        self.begin_live_chat();
    }

    /// `Esc`：收起浮层并清空输入焦点与草稿。
    pub(crate) fn hide_danmaku_overlay(&mut self) {
        self.danmaku_overlay.hide();
        self.chat_input = None;
    }

    pub(crate) fn scroll_danmaku(&mut self, up: bool) -> bool {
        if up {
            self.danmaku_overlay.scroll_up()
        } else {
            self.danmaku_overlay.scroll_down()
        }
    }

    pub(crate) fn scroll_danmaku_page(&mut self, up: bool) -> bool {
        self.danmaku_overlay.scroll_page(up)
    }

    pub(crate) fn follow_danmaku(&mut self) -> bool {
        self.danmaku_overlay.follow()
    }

    pub(crate) fn reset_danmaku_overlay(&mut self) {
        self.danmaku_overlay.reset();
        self.chat_input = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(user: &str, text: &str) -> np_live::DanmakuMessage {
        np_live::DanmakuMessage {
            user: user.to_owned(),
            uid: None,
            text: text.to_owned(),
            color: 0,
            timestamp: 0,
        }
    }

    #[test]
    fn sanitize_strips_controls_newlines_and_bidi_marks() {
        let cleaned = sanitize("\u{1b}[31m红\n色\r\t弹幕\u{202E}\u{200B}\u{FE0F}");
        assert_eq!(cleaned, "[31m红 色 弹幕");
        assert!(!cleaned.chars().any(char::is_control));
    }

    #[test]
    fn sanitize_keeps_ordinary_text_and_collapses_blank_runs() {
        assert_eq!(sanitize("  晚上好  "), "晚上好");
        assert_eq!(sanitize("a\n\n\nb"), "a b");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn wrapping_keeps_display_width_and_never_uses_an_ellipsis() {
        let text = "这是一条很长的中文弹幕内容，用来验证按显示宽度换行时不会被省略";
        let lines = wrap_message_lines(&message("观众", text), 30, None);
        assert!(lines.len() > 1, "长消息必须换行");
        for line in &lines {
            assert!(line.width() <= 30, "{:?}", line.text());
            assert!(!line.text().contains('…'));
        }
        // 续行不缩进、不重复用户名。
        let joined: String = lines.iter().map(WrappedLine::text).collect();
        assert_eq!(joined, format!("观众: {text}"));
        assert!(!lines[1].text().starts_with(' '));
        assert!(!lines[1].text().contains("观众:"));
    }

    #[test]
    fn own_uid_marks_all_wrapped_lines_and_missing_uid_never_matches() {
        let mut own = message("我", "这是一条自己的回显");
        own.uid = Some(42);
        let lines = wrap_message_lines(&own, 12, Some(42));
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.own));

        assert!(!wrap_message_lines(&own, 12, Some(7))[0].own);
        assert!(!wrap_message_lines(&own, 12, None)[0].own);

        let no_uid = message("我", "没有 uid");
        assert!(!wrap_message_lines(&no_uid, 12, Some(42))[0].own);
        assert!(!wrap_message_lines(&no_uid, 12, None)[0].own);
    }

    #[test]
    fn wrapping_keeps_wide_characters_inside_the_cell_budget() {
        let text = "弹".repeat(20);
        let lines = wrap_message_lines(&message("我", &text), 30, None);
        for line in &lines {
            assert!(line.width() <= 30);
        }
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn wrapping_a_long_username_never_drops_text() {
        let user = "很长的用户名".repeat(10);
        let lines = wrap_message_lines(&message(&user, "正文"), 30, None);
        let joined: String = lines.iter().map(WrappedLine::text).collect();
        assert_eq!(joined, format!("{user}: 正文"));
        for line in &lines {
            assert!(line.width() <= 30);
        }
    }

    #[test]
    fn truncation_keeps_display_width_for_wide_characters() {
        let text = "这是一条很长的中文弹幕内容";
        for width in 3..=16 {
            let cut = truncate_width(text, width);
            assert!(
                display_width(&cut) <= width,
                "width {width}: {:?} = {}",
                cut,
                display_width(&cut)
            );
        }
        assert!(truncate_width(text, 10).ends_with('…'));
    }

    #[test]
    fn window_keeps_only_the_visible_rows_and_follows_the_bottom() {
        let (start, end, rows) = message_window(500, 14, 0);
        assert_eq!((start, end, rows), (486, 500, 14));
        let (start, end, _) = message_window(500, 14, 5);
        assert_eq!((start, end), (481, 495));
        let (start, end, rows) = message_window(3, 14, 0);
        assert_eq!((start, end, rows), (0, 3, 3));
        let (start, end, _) = message_window(20, 14, 99);
        assert_eq!((start, end), (0, 14));
    }

    #[test]
    fn scroll_and_follow_track_unread_messages() {
        let mut overlay = DanmakuOverlay::default();
        overlay.sync(40);
        overlay.show();
        overlay.set_scroll_metrics(40, 14);
        // 跟随态：新消息直接算已读。
        assert!(overlay.sync(41));
        assert_eq!(overlay.unread(), 0);
        // 向上滚动后，新消息计未读。
        assert!(overlay.scroll_up());
        assert!(overlay.sync(44));
        assert_eq!(overlay.unread(), 3);
        assert_eq!(overlay.offset(), 1);
        // f 回到跟随并清零未读。
        assert!(overlay.follow());
        assert_eq!(overlay.unread(), 0);
        assert_eq!(overlay.offset(), 0);
        // 滚到最底部也恢复已读。
        assert!(overlay.scroll_up());
        assert!(overlay.sync(45));
        assert_eq!(overlay.unread(), 1);
        assert!(overlay.scroll_down());
        assert_eq!(overlay.unread(), 0);
    }

    #[test]
    fn scroll_stops_at_the_oldest_message() {
        let mut overlay = DanmakuOverlay::default();
        overlay.sync(20);
        overlay.show();
        overlay.set_scroll_metrics(20, 14);
        assert_eq!(overlay.offset(), 0);
        assert!(overlay.scroll_up());
        assert!(overlay.scroll_up());
        for _ in 0..40 {
            overlay.scroll_up();
        }
        // 20 行、14 行窗口：最多向上 6 行。
        assert_eq!(overlay.offset(), 6);
        assert!(!overlay.scroll_up(), "already at the oldest row");
    }

    #[test]
    fn page_keys_move_by_one_window_and_clamp() {
        let mut overlay = DanmakuOverlay::default();
        overlay.sync(40);
        overlay.show();
        overlay.set_scroll_metrics(40, 14);
        assert!(overlay.scroll_page(true));
        assert_eq!(overlay.offset(), 14);
        assert!(overlay.scroll_page(true));
        assert_eq!(overlay.offset(), 26, "40 行、14 行窗口最多上移 26 行");
        assert!(!overlay.scroll_page(true));
        assert!(overlay.scroll_page(false));
        assert_eq!(overlay.offset(), 12);
        overlay.follow();
        assert!(!overlay.scroll_page(false));
        assert_eq!(overlay.offset(), 0);
    }

    #[test]
    fn toggle_resets_scroll_and_clears_the_backlog() {
        let mut overlay = DanmakuOverlay::default();
        overlay.sync(10);
        overlay.show();
        overlay.set_scroll_metrics(4, 3);
        assert!(overlay.scroll_up());
        overlay.sync(12);
        assert_eq!(overlay.unread(), 2);
        overlay.hide();
        assert!(!overlay.visible());
        overlay.show();
        assert!(overlay.visible());
        assert!(!overlay.input_focused());
        assert_eq!(overlay.unread(), 0);
        assert_eq!(overlay.offset(), 0);
    }

    #[test]
    fn title_label_shows_scroll_and_unread_caps() {
        assert_eq!(title_label(0, 0), "");
        assert_eq!(title_label(2, 0), "↑/↓");
        assert_eq!(title_label(0, 3), "3");
        assert_eq!(title_label(2, 3), "↑/↓ 3");
        assert_eq!(title_label(0, 1234), "99+");
    }

    #[test]
    fn online_is_four_digits_up_to_9999_and_uses_w_above_it() {
        assert_eq!(online_text(Some(2000)), " 2000 ");
        assert_eq!(online_text(Some(0)), "    0 ");
        assert_eq!(online_text(Some(9999)), " 9999 ");
        assert_eq!(online_text(Some(12345)), " 1.2w ");
        assert_eq!(online_text(Some(10000)), " 1.0w ");
        assert_eq!(online_text(Some(99999)), " 9.9w ");
        assert_eq!(online_text(Some(123456)), " 12.3w ");
        assert_eq!(online_text(None), "  --  ");
        // ≤9999 与 ` -- ` 都保持 4 格数字域 + 两侧空格。
        for value in [0, 9, 99, 999, 2000, 9999, 12345, 99999] {
            assert_eq!(
                display_width(&online_text(Some(value))),
                ONLINE_CELL_WIDTH + 2,
                "{value}"
            );
        }
        assert_eq!(display_width(&online_text(None)), ONLINE_CELL_WIDTH + 2);
    }

    #[test]
    fn overlay_geometry_matches_the_card() {
        let layout = crate::live_page::layout(Rect::new(0, 0, 72, 24));
        let overlay = area(&layout).expect("overlay");
        assert_eq!(overlay, Rect::new(36, 2, 34, 18));
        // 内容区 32 列，消息 14 行（18 - 4 行框架）。
        assert_eq!(overlay.width - 2, 32);
        assert_eq!(overlay.height - CHROME_ROWS, 14);
        // 信息行 y20 不在浮层内。
        assert_eq!(overlay.bottom(), 20);
        assert_eq!(layout.info_y, 20);

        // 60×20 仍让出 Information 行；40×10 用满 5 行换取 1 行消息。
        let medium = crate::live_page::layout(Rect::new(0, 0, 60, 20));
        let medium_overlay = area(&medium).expect("60×20 overlay");
        assert_eq!(medium_overlay, Rect::new(24, 2, 34, 14));
        assert!(medium_overlay.bottom() <= medium.info_y);

        let small = crate::live_page::layout(Rect::new(0, 0, 40, 10));
        let small_overlay = area(&small).expect("40×10 overlay");
        assert_eq!(small_overlay, Rect::new(12, 2, 26, 5));
        assert_eq!(small_overlay.bottom(), small.info_y + 1);
        assert_eq!(small_overlay.height - CHROME_ROWS, 1, "至少 1 行消息");

        for (width, height) in [(24u16, 8u16), (16, 6)] {
            let tiny = crate::live_page::layout(Rect::new(0, 0, width, height));
            assert!(area(&tiny).is_none());
        }
    }

    #[test]
    fn overlay_is_disabled_below_ten_screen_rows() {
        // 布局层：高度 9 永远没有浮层区域。
        assert!(area(&crate::live_page::layout(Rect::new(0, 0, 72, 9))).is_none());
        assert!(area(&crate::live_page::layout(Rect::new(0, 0, 72, 10))).is_some());

        let mut app = App::new();
        app.set_viewport_height(9);
        app.toggle_danmaku_overlay(0);
        assert!(!app.danmaku_overlay_visible());
        assert!(!app.live_chat_active());
        assert!(!app.begin_live_chat(), "c 在 <10 行时也不能打开");
        assert!(!app.danmaku_overlay_visible());

        app.set_viewport_height(10);
        app.toggle_danmaku_overlay(0);
        assert!(app.danmaku_overlay_visible());
        assert!(app.live_chat_active());

        // 运行中缩到 <10：浮层与焦点一起关闭，回退普通 Live 页。
        assert!(app.set_viewport_height(9));
        assert!(!app.danmaku_overlay_visible());
        assert!(!app.live_chat_active());
    }

    #[test]
    fn overlay_keys_yield_to_command_input_and_focus() {
        let mut app = App::new();
        // d 打开浮层时同步聚焦 Input，字符键先给输入框。
        app.toggle_danmaku_overlay(0);
        assert!(app.live_chat_active());
        assert!(!keys_owned(&app));

        // Esc 退出焦点后，↑/↓/f 才归浮层。
        app.cancel_live_chat();
        assert!(keys_owned(&app), "退出焦点后由浮层接管 ↑/↓/f");

        // 命令模式：f 应作为字符输入，↑/↓ 应翻历史 / 补全。
        app.enter_command();
        assert!(!keys_owned(&app));
        app.leave_command();
        assert!(keys_owned(&app));

        // c 重新聚焦时字符键优先，浮层不接管。
        app.begin_live_chat();
        assert!(!keys_owned(&app));
        app.cancel_live_chat();
        assert!(keys_owned(&app));

        // 歌单命名输入同样优先。
        app.playlist_input = Some(crate::app::PlaylistInput {
            value: String::new(),
            cursor: 0,
        });
        assert!(!keys_owned(&app));
        app.playlist_input = None;

        // Esc 收起浮层；d 只负责打开 / 聚焦。
        app.hide_danmaku_overlay();
        assert!(!app.danmaku_overlay_visible());
        assert!(!keys_owned(&app));
        app.toggle_danmaku_overlay(0);
        assert!(app.danmaku_overlay_visible());
        assert!(app.live_chat_active());
    }
}
