//! Live 页右侧弹幕浮层（T-0100b）。
//!
//! 消息源是 [`TuiAudio`] 的 500 条环形缓冲；本模块只负责窗口选择、控制字符
//! 清理与绘制，不建立连接，也不改变 `c` 输入框的状态机。

use std::cell::Cell;
use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::app::App;
use crate::audio::TuiAudio;
use crate::input::{display_width, truncate_width};
use crate::live_page::LiveLayout;
use crate::model::Focus;
use crate::theme::Palette;

/// 浮层宽度：72×24 下覆盖视觉区右侧 30 列，左侧留 36 列频谱。
pub(crate) const OVERLAY_WIDTH: u16 = 30;
/// 浮层最大高度；72×24 下正好等于视觉区高度 19。
pub(crate) const OVERLAY_MAX_HEIGHT: u16 = 19;
/// 标题栏占 1 行，其余给消息。
const TITLE_HEIGHT: u16 = 1;
/// 标题文字；前导空格与外框留白一致。
const TITLE_TEXT: &str = " 弹幕";
/// 空态文案。
pub(crate) const EMPTY_TEXT: &str = "暂无弹幕";
/// 未读数最多显示到 99+，避免标题行被撑破。
const MAX_UNREAD_LABEL: u64 = 99;

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

    /// 单调递增的收包计数（含本地回显）；浮层用它算未读与滚动窗口。
    pub(crate) fn danmaku_received(&self) -> u64 {
        self.danmaku_received.get()
    }

    /// 发送成功后的本地回显：写进同一个环形缓冲，不依赖服务器回包。
    pub(crate) fn push_local_danmaku(&self, user: &str, text: &str) {
        let message = np_live::DanmakuMessage {
            user: user.to_owned(),
            text: text.to_owned(),
            color: 0x00FF_FFFF,
            timestamp: crate::status::now_millis(),
        };
        push_recent(&mut self.danmaku_recent.borrow_mut(), message);
        self.danmaku_received
            .set(self.danmaku_received.get().saturating_add(1));
    }
}

/// 弹幕浮层的 UI 状态；不持有消息本体（消息留在 [`TuiAudio`]）。
#[derive(Debug, Default)]
pub(crate) struct DanmakuOverlay {
    visible: bool,
    /// 距最新消息的偏移；0 表示跟随底部。
    offset_from_bottom: usize,
    /// TuiAudio 的收包计数（单调递增，含本地回显）。
    total: u64,
    /// 已经看到的消息计数；`total - seen_total` 即未读。
    seen_total: u64,
    /// 上一次绘制时的正文行数，滚动上限按它计算。
    body_rows: Cell<usize>,
}

impl DanmakuOverlay {
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset_from_bottom
    }

    pub(crate) fn unread(&self) -> u64 {
        self.total.saturating_sub(self.seen_total)
    }

    /// 显示 / 隐藏；打开时回到跟随并清空未读。
    pub(crate) fn toggle(&mut self, total: u64) -> bool {
        self.total = total;
        if self.visible {
            self.visible = false;
            self.offset_from_bottom = 0;
        } else {
            self.visible = true;
            self.offset_from_bottom = 0;
            self.seen_total = total;
        }
        self.visible
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

    /// 向上滚动一条；已经在顶部时返回 `false`。
    pub(crate) fn scroll_up(&mut self) -> bool {
        let rows = self.body_rows.get().max(1);
        let rows = u64::try_from(rows).unwrap_or(u64::MAX);
        let max_offset = usize::try_from(self.total.saturating_sub(rows)).unwrap_or(usize::MAX);
        if self.offset_from_bottom >= max_offset {
            return false;
        }
        self.offset_from_bottom += 1;
        true
    }

    /// 向下滚动一条；回到底部即恢复跟随并清零未读。
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

    /// `f`：回到跟随最新，并清零未读。
    pub(crate) fn follow(&mut self) -> bool {
        let changed = self.offset_from_bottom != 0 || self.unread() != 0;
        self.offset_from_bottom = 0;
        self.seen_total = self.total;
        changed
    }

    /// 停播 / 换房时清空浮层状态。
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn set_body_rows(&self, rows: usize) {
        self.body_rows.set(rows);
    }
}

/// 当前窗口 `(start, end, 渲染行数)`：始终贴底，向上偏移不超过历史条数。
pub(crate) fn message_window(total: usize, rows: usize, offset: usize) -> (usize, usize, usize) {
    let rows = rows.min(total);
    let max_offset = total.saturating_sub(rows);
    let offset = offset.min(max_offset);
    let start = total - rows - offset;
    (start, start + rows, rows)
}

/// 浮层区域：72×24 下为 x39,y2,30×19；小尺寸按视觉区收缩，不 panic。
pub(crate) fn area(layout: &LiveLayout) -> Option<Rect> {
    let visual = layout.visual;
    if visual.width < 16 || visual.height < 2 {
        return None;
    }
    let width = OVERLAY_WIDTH.min(visual.width.saturating_sub(8));
    if width < 8 {
        return None;
    }
    let height = visual.height.min(OVERLAY_MAX_HEIGHT);
    Some(Rect::new(
        visual.right().saturating_sub(width),
        visual.y,
        width,
        height,
    ))
}

/// 浮层可见时左侧频谱区域；72×24 下为 x3,y2,36×19。
pub(crate) fn left_area(layout: &LiveLayout, overlay: Rect) -> Rect {
    Rect {
        x: layout.visual.x,
        y: layout.visual.y,
        width: layout.visual.width.saturating_sub(overlay.width),
        height: layout.visual.height,
    }
}

/// 浮层键只在 Live 普通态接管。
///
/// 命令输入（`Focus::Controller`）与歌单命名输入必须优先：否则 `f` 会被当成
/// 跟随而不是输入字符，`↑/↓` 会滚弹幕而不是翻历史 / 补全。聊天输入框、
/// 搜索与设置编辑在事件循环里排得更前，不在这里重复判断。
pub(crate) fn keys_owned(app: &App) -> bool {
    app.danmaku_overlay_visible() && app.focus != Focus::Controller && !app.playlist_input_active()
}

/// 浮层可见时的按键：↑/↓ 滚动、`f` 跟随；`d` 由 keymap 处理。
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
        KeyCode::Char('f') => {
            app.follow_danmaku();
            true
        }
        _ => false,
    }
}

/// 绘制浮层：整块铺底色 → 左边框 → 标题/未读 → 底部对齐的消息。
pub(crate) fn draw(frame: &mut ratatui::Frame, area: Rect, app: &App, player: &TuiAudio) {
    if area.width < 6 || area.height < 2 {
        return;
    }
    let palette = &app.palette;
    let bg = palette.bg.unwrap_or(Color::Reset);
    let fill = " ".repeat(usize::from(area.width));
    for y in area.y..area.bottom() {
        frame
            .buffer_mut()
            .set_string(area.x, y, &fill, Style::default().bg(bg));
        frame.buffer_mut().set_string(
            area.x,
            y,
            "│",
            Style::default().fg(palette.line_strong).bg(bg),
        );
    }

    let inner = Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    let body_rows = usize::from(area.height.saturating_sub(TITLE_HEIGHT));
    app.danmaku_overlay.set_body_rows(body_rows);
    draw_title(frame, inner, app);
    draw_messages(
        frame,
        Rect {
            x: inner.x,
            y: area.y.saturating_add(TITLE_HEIGHT),
            width: inner.width,
            height: area.height.saturating_sub(TITLE_HEIGHT),
        },
        app,
        player,
    );
}

fn draw_title(frame: &mut ratatui::Frame, inner: Rect, app: &App) {
    if inner.width == 0 {
        return;
    }
    let palette = &app.palette;
    let bg = palette.bg.unwrap_or(Color::Reset);
    frame.buffer_mut().set_string(
        inner.x,
        inner.y,
        TITLE_TEXT,
        Style::default()
            .fg(palette.accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    );
    let unread = app.danmaku_overlay.unread();
    let label = title_label(app.danmaku_overlay.offset(), unread);
    if label.is_empty() {
        return;
    }
    let label_width = display_width(&label) as u16;
    let title_width = display_width(TITLE_TEXT) as u16;
    if title_width.saturating_add(label_width).saturating_add(2) > inner.width {
        return;
    }
    let x = inner.right().saturating_sub(label_width).saturating_sub(1);
    let style = if unread > 0 {
        Style::default().fg(palette.warn).bg(bg)
    } else {
        Style::default().fg(palette.muted).bg(bg)
    };
    frame.buffer_mut().set_string(x, inner.y, &label, style);
}

/// 标题行右侧：滚动时显示 `↑/↓`，有未读时显示计数（最多 99+）。
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

fn draw_messages(frame: &mut ratatui::Frame, area: Rect, app: &App, player: &TuiAudio) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let palette = &app.palette;
    let bg = palette.bg.unwrap_or(Color::Reset);
    let messages = player.danmaku_messages();
    if messages.is_empty() {
        let text = truncate_width(EMPTY_TEXT, usize::from(area.width));
        let x = area
            .x
            .saturating_add(area.width.saturating_sub(display_width(&text) as u16) / 2);
        let y = area.y.saturating_add(area.height / 2);
        frame
            .buffer_mut()
            .set_string(x, y, &text, Style::default().fg(palette.faint).bg(bg));
        return;
    }

    // 右侧留 1 格空列，宽字符截断后也不会压到浮层右缘。
    let line_width = usize::from(area.width.saturating_sub(1));
    let (start, end, rows) = message_window(
        messages.len(),
        usize::from(area.height),
        app.danmaku_overlay.offset(),
    );
    let first_y = area.y.saturating_add(
        area.height
            .saturating_sub(u16::try_from(rows).unwrap_or(u16::MAX)),
    );
    for (row, message) in messages[start..end].iter().enumerate() {
        let y = first_y.saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
        draw_message(frame, area.x, y, line_width, message, palette, bg);
    }
}

fn draw_message(
    frame: &mut ratatui::Frame,
    x: u16,
    y: u16,
    width: usize,
    message: &np_live::DanmakuMessage,
    palette: &Palette,
    bg: Color,
) {
    if width == 0 {
        return;
    }
    let user = sanitize(&message.user);
    let user = if user.is_empty() {
        "匿名".to_owned()
    } else {
        user
    };
    let prefix = format!("{user}: ");
    let prefix_width = display_width(&prefix);
    if prefix_width >= width {
        let prefix = truncate_width(&prefix, width);
        frame
            .buffer_mut()
            .set_string(x, y, &prefix, Style::default().fg(palette.muted).bg(bg));
        return;
    }
    frame
        .buffer_mut()
        .set_string(x, y, &prefix, Style::default().fg(palette.accent).bg(bg));
    let text = truncate_width(&sanitize(&message.text), width - prefix_width);
    frame.buffer_mut().set_string(
        x.saturating_add(u16::try_from(prefix_width).unwrap_or(u16::MAX)),
        y,
        &text,
        Style::default().fg(palette.fg).bg(bg),
    );
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
    // ---- Live 页弹幕浮层（T-0100b） ----

    pub(crate) fn danmaku_overlay_visible(&self) -> bool {
        self.danmaku_overlay.visible()
    }

    /// 同步收包计数；返回需要重绘（未读或总数变化）。
    pub(crate) fn sync_danmaku(&mut self, total: u64) -> bool {
        self.danmaku_overlay.sync(total)
    }

    pub(crate) fn toggle_danmaku_overlay(&mut self, total: u64) {
        self.danmaku_overlay.toggle(total);
    }

    pub(crate) fn scroll_danmaku(&mut self, up: bool) -> bool {
        if up {
            self.danmaku_overlay.scroll_up()
        } else {
            self.danmaku_overlay.scroll_down()
        }
    }

    pub(crate) fn follow_danmaku(&mut self) -> bool {
        self.danmaku_overlay.follow()
    }

    pub(crate) fn reset_danmaku_overlay(&mut self) {
        self.danmaku_overlay.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay_with(total: u64) -> DanmakuOverlay {
        let mut overlay = DanmakuOverlay::default();
        overlay.sync(total);
        overlay.toggle(total);
        overlay
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
        let (start, end, rows) = message_window(500, 18, 0);
        assert_eq!((start, end, rows), (482, 500, 18));
        let (start, end, _) = message_window(500, 18, 5);
        assert_eq!((start, end), (477, 495));
        let (start, end, rows) = message_window(3, 18, 0);
        assert_eq!((start, end, rows), (0, 3, 3));
        // 偏移再大也只能停在最上面一条，不会越界。
        let (start, end, _) = message_window(20, 18, 99);
        assert_eq!((start, end), (0, 18));
    }

    #[test]
    fn scroll_and_follow_track_unread_messages() {
        let mut overlay = overlay_with(40);
        overlay.set_body_rows(18);
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
        let mut overlay = overlay_with(20);
        overlay.set_body_rows(18);
        assert!(overlay.scroll_up());
        assert!(overlay.scroll_up());
        assert!(!overlay.scroll_up(), "already at the oldest row");
        assert_eq!(overlay.offset(), 2);
    }

    #[test]
    fn toggle_resets_scroll_and_clears_the_backlog() {
        let mut overlay = overlay_with(10);
        overlay.set_body_rows(3);
        assert!(overlay.scroll_up());
        overlay.sync(12);
        assert_eq!(overlay.unread(), 2);
        assert!(!overlay.toggle(12), "closes");
        assert!(!overlay.visible());
        assert!(overlay.toggle(12), "opens");
        assert!(overlay.visible());
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
    fn overlay_geometry_matches_the_card() {
        let layout = crate::live_page::layout(Rect::new(0, 0, 72, 24));
        let overlay = area(&layout).expect("overlay");
        assert_eq!(overlay, Rect::new(39, 2, 30, 19));
        assert_eq!(left_area(&layout, overlay), Rect::new(3, 2, 36, 19));

        // 小尺寸不 panic，也不会吃掉整个视觉区。
        for (width, height) in [(60u16, 20u16), (40, 10), (24, 8), (16, 6)] {
            let small = crate::live_page::layout(Rect::new(0, 0, width, height));
            if let Some(overlay) = area(&small) {
                assert!(overlay.right() <= small.visual.right());
                assert!(left_area(&small, overlay).width >= 8);
            }
        }
    }

    #[test]
    fn overlay_keys_yield_to_command_and_playlist_input() {
        let mut app = App::new();
        app.toggle_danmaku_overlay(0);
        assert!(keys_owned(&app), "普通态由浮层接管 ↑/↓/f");

        // 命令模式：f 应作为字符输入，↑/↓ 应翻历史 / 补全。
        app.enter_command();
        assert!(!keys_owned(&app));
        app.leave_command();
        assert!(keys_owned(&app));

        // 歌单命名输入同样优先。
        app.playlist_input = Some(crate::app::PlaylistInput {
            value: String::new(),
            cursor: 0,
        });
        assert!(!keys_owned(&app));
        app.playlist_input = None;
        assert!(keys_owned(&app));

        // 浮层隐藏时任何状态都不接管。
        app.toggle_danmaku_overlay(0);
        assert!(!keys_owned(&app));
    }
}
