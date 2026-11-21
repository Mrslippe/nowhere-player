//! Browse「直播」页：房间号输入 + 历史选择。
//!
//! 状态与交互都在这里；`App` 只持有 [`LiveEntry`] 字段，不新增方法。
//! 输入框只接受数字，Enter 走 `player.start_live`（与 `/live` 命令同一个入口）。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, Borders, Paragraph};

use crate::app::App;
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::{display_width, input_window, truncate_width};
use crate::live_history::LiveHistory;
use crate::theme::PaletteToken;

/// 房间号最多 12 位；B 站房间号实际远小于这个上限。
pub(crate) const ROOM_ID_MAX_DIGITS: usize = 12;
/// 输入框下方最多显示 3 条历史。
pub(crate) const HISTORY_ROWS: usize = 3;
/// 输入框 38×3；内区不足 4 行时只画输入框。
/// （72×24 的内区是 7 行，装得下输入框 + 3 条历史。）
const INPUT_WIDTH: u16 = 38;
const INPUT_HEIGHT: u16 = 3;
/// 输入框 + 至少 1 行历史的最小高度；更矮时历史不参与绘制。
const MIN_WITH_HISTORY: u16 = INPUT_HEIGHT + 1;

#[derive(Debug, Default)]
pub(crate) struct LiveEntry {
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) selected: Option<usize>,
    pub(crate) history: LiveHistory,
    /// 已开播、等主播名回填的房间号。
    pub(crate) pending_anchor: Option<u64>,
    /// 输入态：List 再按一次 `Enter` 才置真；未激活时数字 / Esc 都交回全局键位。
    pub(crate) input_active: bool,
}

impl LiveEntry {
    /// 读历史文件；warning 由 `main` 写状态栏。
    pub(crate) fn load(path: Option<PathBuf>) -> (Self, Option<String>) {
        let (history, warning) = LiveHistory::load(path);
        (
            Self {
                history,
                ..Self::default()
            },
            warning,
        )
    }

    /// 记录一次开播并清掉输入；返回历史保存失败的提示（由调用方写状态栏）。
    pub(crate) fn record_start(&mut self, room: u64) -> Option<String> {
        self.input.clear();
        self.cursor = 0;
        self.selected = None;
        self.input_active = false;
        self.pending_anchor = Some(room);
        self.history.record(room, "");
        self.history.save().err()
    }

    /// `/live <room>` 命令入口：命令已经开播，这里只记录历史。
    pub(crate) fn record_command(&mut self, input: &str) -> Option<String> {
        let Some(Ok(room)) = crate::live_command::parse_live_command(input) else {
            return None;
        };
        self.record_start(room)
    }

    fn insert_digit(&mut self, digit: char) {
        if self.input.len() >= ROOM_ID_MAX_DIGITS {
            return;
        }
        // 输入只含 ASCII 数字，字节下标与光标下标一致。
        self.input.insert(self.cursor, digit);
        self.cursor += 1;
        self.selected = None;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.input.remove(self.cursor);
        self.selected = None;
    }

    fn move_cursor(&mut self, right: bool) {
        if right {
            if self.cursor < self.input.len() {
                self.cursor += 1;
            }
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    /// ↑↓：选中历史并把房间号填进输入框。
    fn move_selection(&mut self, down: bool) {
        let len = self.history.entries.len();
        if len == 0 {
            return;
        }
        let next = match (self.selected, down) {
            (None, _) => 0,
            (Some(index), false) => index.saturating_sub(1),
            (Some(index), true) => (index + 1).min(len - 1),
        };
        self.selected = Some(next);
        self.input = self.history.entries[next].room_id.to_string();
        self.cursor = self.input.len();
    }

    /// Delete：删掉选中的历史；返回是否真的删了。
    fn remove_selected(&mut self) -> bool {
        let Some(index) = self.selected else {
            return false;
        };
        let Some(room) = self.history.entries.get(index).map(|entry| entry.room_id) else {
            self.selected = None;
            return false;
        };
        self.history.remove(room);
        self.selected = if self.history.entries.is_empty() {
            None
        } else {
            Some(index.min(self.history.entries.len() - 1))
        };
        true
    }

    /// Enter 的目标：优先输入框，空时用历史选中项。
    fn target_room(&self) -> Result<u64, String> {
        let text = self.input.trim();
        if !text.is_empty() {
            return match text.parse::<u64>() {
                Ok(room) if room > 0 => Ok(room),
                _ => Err(format!("房间号无效：{text}")),
            };
        }
        let Some(index) = self.selected else {
            return Err("输入房间号再按 Enter".to_owned());
        };
        self.history
            .entries
            .get(index)
            .map(|entry| entry.room_id)
            .ok_or_else(|| "输入房间号再按 Enter".to_owned())
    }
}

/// Live 页的按键入口；返回 `true` 表示已消费。
///
/// 只有 `input_active` 时本页才接管键盘：数字、Backspace、←→、↑↓、Enter、Esc、
/// Delete；`Ctrl+*` 全部放行给全局键位（`Ctrl+F` 专注模式），其它普通字符吞掉，
/// 避免 `q` / `s` / `f` 误触。未激活时返回 `false`，让全局 `Enter` / `→` / `Esc`
/// 走搜索页同一条节奏。
pub(crate) fn handle_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    app: &mut App,
    player: &TuiAudio,
) -> bool {
    if !app.live_entry.input_active {
        return false;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    match code {
        KeyCode::Char(digit) if digit.is_ascii_digit() => {
            app.live_entry.insert_digit(digit);
            true
        }
        KeyCode::Char(_) => true,
        KeyCode::Backspace => {
            app.live_entry.backspace();
            true
        }
        KeyCode::Left => {
            app.live_entry.move_cursor(false);
            true
        }
        KeyCode::Right => {
            app.live_entry.move_cursor(true);
            true
        }
        KeyCode::Up => {
            app.live_entry.move_selection(false);
            true
        }
        KeyCode::Down => {
            app.live_entry.move_selection(true);
            true
        }
        KeyCode::Delete => {
            delete_selected(app);
            true
        }
        KeyCode::Enter => {
            enter(app, player);
            true
        }
        KeyCode::Esc => {
            // 输入态 Esc 只退出输入，焦点留在 List；再按一次才由全局键回 Browse。
            app.live_entry.input_active = false;
            app.set_transient_status("已退出直播输入");
            true
        }
        _ => false,
    }
}

fn delete_selected(app: &mut App) {
    if !app.live_entry.remove_selected() {
        return;
    }
    let saved = app.live_entry.history.save();
    match saved {
        Ok(()) => app.set_transient_status("已删除直播历史"),
        Err(error) => app.set_status(error),
    }
}

fn enter(app: &mut App, player: &TuiAudio) {
    match app.live_entry.target_room() {
        Ok(room) => {
            let status = crate::live_command::start_live(player, room);
            match app.live_entry.record_start(room) {
                Some(error) => app.set_status(format!("{status}（历史保存失败：{error}）")),
                None => app.set_status(status),
            }
        }
        Err(error) => app.set_status(error),
    }
}

/// 主播名回填：pending 房间的真实名字到了就写进历史并保存。
///
/// 非直播 / 已经换房时清掉 pending；返回历史是否变化（调用方据此重绘）。
pub(crate) fn poll_anchor(app: &mut App, player: &TuiAudio) -> bool {
    let Some(pending) = app.live_entry.pending_anchor else {
        return false;
    };
    if !player.is_live() || player.live_room() != pending {
        app.live_entry.pending_anchor = None;
        return false;
    }
    let Some(anchor) = player.live_anchor_name() else {
        return false;
    };
    app.live_entry.pending_anchor = None;
    let changed = app.live_entry.history.update_anchor(pending, &anchor);
    let saved = app.live_entry.history.save();
    if let Err(error) = saved {
        app.set_status(error);
    }
    changed
}

/// 居中 38×3 `Room ID` 输入框，下方最多 3 条历史。
pub(crate) fn draw(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let entry = &app.live_entry;
    let active = entry.input_active;
    let box_width = INPUT_WIDTH.min(area.width);
    let box_height = INPUT_HEIGHT.min(area.height);
    let rows = if area.height < MIN_WITH_HISTORY {
        // 内区太矮：只画输入框，历史留到窗口变大后出现。
        0
    } else {
        entry
            .history
            .entries
            .len()
            .min(HISTORY_ROWS)
            .min(usize::from(area.height - box_height))
    };
    let total_height = box_height.saturating_add(u16::try_from(rows).unwrap_or(0));
    let start_y = area.y + area.height.saturating_sub(total_height) / 2;
    let box_area = Rect {
        x: area.x + area.width.saturating_sub(box_width) / 2,
        y: start_y,
        width: box_width,
        height: box_height,
    };
    draw_input(frame, box_area, app, entry, active);
    for (index, item) in entry.history.entries.iter().take(rows).enumerate() {
        let y = box_area
            .bottom()
            .saturating_add(u16::try_from(index).unwrap_or(0));
        if y >= area.bottom() {
            break;
        }
        let selected = entry.selected == Some(index);
        let marker = if selected { glyphs::SELECT } else { " " };
        let text = truncate_width(
            &format!("{marker} {}", item.label()),
            usize::from(area.width),
        );
        let x = area.x + area.width.saturating_sub(display_width(&text) as u16) / 2;
        let style = if selected {
            app.palette.selection_style()
        } else {
            Style::default().fg(app.palette.color(PaletteToken::Muted))
        };
        frame.buffer_mut().set_string(x, y, &text, style);
    }
}

fn draw_input(frame: &mut ratatui::Frame, area: Rect, app: &App, entry: &LiveEntry, active: bool) {
    let bg = app.palette.color(PaletteToken::Canvas);
    let block = TuiBlock::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if active {
            app.palette.color(PaletteToken::Focus)
        } else {
            app.palette.color(PaletteToken::Border)
        }))
        .title_style(Style::default().fg(if active {
            app.palette.color(PaletteToken::Accent)
        } else {
            app.palette.color(PaletteToken::Muted)
        }))
        .title(" Room ID ")
        .style(Style::default().bg(bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let content = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };
    if content.width == 0 {
        return;
    }
    let chars: Vec<char> = entry.input.chars().collect();
    let cursor = entry.cursor.min(chars.len());
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
        let prefix: String = chars.iter().take(cursor).skip(start).collect();
        frame.set_cursor_position((
            content.x.saturating_add(display_width(&prefix) as u16),
            content.y,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Focus;

    fn entry_with_history(rooms: &[u64]) -> LiveEntry {
        let mut entry = LiveEntry::default();
        // 历史最新在前：倒序记录。
        for room in rooms.iter().rev() {
            entry.history.record(*room, "");
        }
        entry
    }

    #[test]
    fn digits_only_and_capped_at_twelve() {
        let mut entry = LiveEntry::default();
        for digit in "1234567890123".chars() {
            entry.insert_digit(digit);
        }
        assert_eq!(entry.input, "123456789012");
        assert_eq!(entry.cursor, ROOM_ID_MAX_DIGITS);
        entry.backspace();
        assert_eq!(entry.input, "12345678901");
        assert_eq!(entry.cursor, 11);
    }

    #[test]
    fn cursor_moves_between_digits_and_backspace_edits_middle() {
        let mut entry = LiveEntry::default();
        for digit in "123".chars() {
            entry.insert_digit(digit);
        }
        entry.move_cursor(false);
        entry.move_cursor(false);
        assert_eq!(entry.cursor, 1);
        entry.backspace();
        assert_eq!(entry.input, "23");
        assert_eq!(entry.cursor, 0);
        entry.move_cursor(true);
        assert_eq!(entry.cursor, 1);
        entry.move_cursor(true);
        entry.move_cursor(true);
        assert_eq!(entry.cursor, 2);
    }

    #[test]
    fn up_down_select_history_and_fill_the_input() {
        let mut entry = entry_with_history(&[100, 200, 300]);
        entry.move_selection(false);
        assert_eq!(entry.selected, Some(0));
        assert_eq!(entry.input, "100");
        entry.move_selection(true);
        assert_eq!(entry.selected, Some(1));
        assert_eq!(entry.input, "200");
        entry.move_selection(true);
        entry.move_selection(true);
        assert_eq!(entry.selected, Some(2), "down 停在最后一条");
        entry.move_selection(false);
        assert_eq!(entry.selected, Some(1));
        assert_eq!(entry.cursor, entry.input.len());
    }

    #[test]
    fn digits_after_selecting_history_append_at_the_cursor() {
        let mut entry = entry_with_history(&[100]);
        entry.move_selection(false);
        // ↑↓ 把房间号填进输入框，光标落在末尾；再按数字是追加。
        assert_eq!(entry.input, "100");
        assert_eq!(entry.cursor, 3);
        entry.insert_digit('9');
        assert_eq!(entry.selected, None);
        assert_eq!(entry.input, "1009");
    }

    #[test]
    fn delete_removes_selected_history_and_keeps_a_valid_index() {
        let mut entry = entry_with_history(&[100, 200, 300]);
        assert!(!entry.remove_selected(), "没有选中时不删");
        entry.selected = Some(1);
        assert!(entry.remove_selected());
        let rooms: Vec<u64> = entry
            .history
            .entries
            .iter()
            .map(|item| item.room_id)
            .collect();
        assert_eq!(rooms, vec![100, 300]);
        assert_eq!(entry.selected, Some(1));
        entry.selected = Some(0);
        entry.remove_selected();
        entry.selected = Some(0);
        entry.remove_selected();
        assert!(entry.history.entries.is_empty());
        assert_eq!(entry.selected, None);
    }

    #[test]
    fn enter_prefers_input_then_selected_history() {
        let mut entry = LiveEntry::default();
        assert_eq!(entry.target_room(), Err("输入房间号再按 Enter".to_owned()));
        for digit in "21622811".chars() {
            entry.insert_digit(digit);
        }
        assert_eq!(entry.target_room(), Ok(21622811));
        entry.input.clear();
        entry.cursor = 0;
        entry.history.record(4321, "");
        entry.move_selection(false);
        assert_eq!(entry.target_room(), Ok(4321));
        entry.input = "0".to_owned();
        entry.cursor = 1;
        assert_eq!(entry.target_room(), Err("房间号无效：0".to_owned()));
    }

    #[test]
    fn record_start_clears_input_and_exits_input_mode() {
        let mut entry = LiveEntry {
            input_active: true,
            ..LiveEntry::default()
        };
        for digit in "123".chars() {
            entry.insert_digit(digit);
        }
        assert!(entry.record_start(21622811).is_none(), "path None 不写盘");
        assert!(entry.input.is_empty());
        assert!(!entry.input_active, "开播成功后必须退出输入态");
        assert_eq!(entry.cursor, 0);
        assert_eq!(entry.pending_anchor, Some(21622811));
        assert_eq!(entry.history.entries.len(), 1);
        assert_eq!(entry.history.entries[0].room_id, 21622811);
    }

    #[test]
    fn poll_anchor_clears_pending_when_not_live() {
        let mut app = App::new();
        app.live_entry.pending_anchor = Some(21622811);
        let audio = TuiAudio::empty();
        assert!(!poll_anchor(&mut app, &audio));
        assert_eq!(app.live_entry.pending_anchor, None);
    }

    #[test]
    fn poll_anchor_fills_the_real_anchor_name() {
        let mut app = App::new();
        app.live_entry.history.record(21622811, "");
        app.live_entry.pending_anchor = Some(21622811);
        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST");
        audio.live_meta_for_test("测试主播", "游戏赛事");
        assert!(poll_anchor(&mut app, &audio));
        assert_eq!(app.live_entry.pending_anchor, None);
        assert_eq!(app.live_entry.history.entries[0].anchor, "测试主播");
        // 主播名已到位后再 poll 不会重复变化。
        app.live_entry.pending_anchor = Some(21622811);
        assert!(!poll_anchor(&mut app, &audio));
    }

    #[test]
    fn record_command_records_only_live_commands() {
        let mut entry = LiveEntry::default();
        assert!(entry.record_command("search 晴天").is_none());
        assert!(entry.history.entries.is_empty());
        assert!(entry.record_command("live 0").is_none());
        assert!(entry.record_command("live abc").is_none());
        assert!(entry.history.entries.is_empty(), "无效命令不能写历史");
        assert!(entry.record_command("live 21622811").is_none());
        assert_eq!(entry.history.entries.len(), 1);
        assert_eq!(entry.history.entries[0].room_id, 21622811);
        assert_eq!(entry.pending_anchor, Some(21622811));
    }

    #[test]
    fn handle_key_is_inert_until_input_active() {
        let mut app = App::new();
        app.focus = Focus::List;
        let audio = TuiAudio::empty();
        // 未激活：数字 / Enter / Esc 都交回全局键位，页面状态不变。
        for code in [KeyCode::Char('7'), KeyCode::Enter, KeyCode::Esc] {
            assert!(!handle_key(code, KeyModifiers::NONE, &mut app, &audio));
        }
        assert!(app.live_entry.input.is_empty());
        assert_eq!(app.focus, Focus::List);
        assert_eq!(app.status, "就绪");

        // 激活后数字才写入。
        app.live_entry.input_active = true;
        assert!(handle_key(
            KeyCode::Char('7'),
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        assert_eq!(app.live_entry.input, "7");
    }

    #[test]
    fn handle_key_owns_digits_delete_esc_and_lets_ctrl_through() {
        let mut app = App::new();
        app.focus = Focus::List;
        app.live_entry.input_active = true;
        app.live_entry.history.record(1, "主播A");
        app.live_entry.history.record(2, "");
        app.live_entry.selected = Some(1);
        let audio = TuiAudio::empty();
        assert!(handle_key(
            KeyCode::Delete,
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        assert_eq!(app.live_entry.history.entries.len(), 1);
        assert_eq!(app.live_entry.history.entries[0].room_id, 2);
        assert!(handle_key(
            KeyCode::Char('7'),
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        assert_eq!(app.live_entry.input, "7");
        // 普通字符吞掉，避免 q / s / f 误触全局键。
        assert!(handle_key(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        // Ctrl+F 放行给全局键位。
        assert!(!handle_key(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            &mut app,
            &audio
        ));
        // 输入态 Esc 只退出输入：焦点留在 List，状态给出提示。
        assert!(handle_key(
            KeyCode::Esc,
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        assert!(!app.live_entry.input_active);
        assert_eq!(app.focus, Focus::List);
        assert_eq!(app.status, "已退出直播输入");
        // 退出输入后本页不再接管，Esc 回到全局键位（controller 负责回 Browse）。
        assert!(!handle_key(
            KeyCode::Esc,
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
    }

    #[test]
    fn enter_with_empty_input_keeps_input_active_and_shows_error() {
        let mut app = App::new();
        app.focus = Focus::List;
        app.live_entry.input_active = true;
        let audio = TuiAudio::empty();
        assert!(handle_key(
            KeyCode::Enter,
            KeyModifiers::NONE,
            &mut app,
            &audio
        ));
        assert!(app.live_entry.input_active, "开播失败保持输入态，方便重试");
        assert_eq!(app.status, "输入房间号再按 Enter");
        assert!(!audio.is_live());
    }

    fn draw_area(area: Rect, app: &App) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8))
            .expect("test terminal");
        terminal.draw(|frame| draw(frame, area, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn draw_centers_three_history_rows_and_degrades_on_short_areas() {
        let mut app = App::new();
        app.live_entry.history.record(1, "主播A");
        app.live_entry.history.record(2, "主播B");
        app.live_entry.history.record(3, "主播C");
        let tall = draw_area(Rect::new(0, 0, 60, 7), &app);
        // 宽字符的续格在缓冲区里是空格，断言前先去掉空白。
        let compact = |text: String| -> String {
            text.chars()
                .filter(|character| !character.is_whitespace())
                .collect()
        };
        let tall = compact(tall);
        assert!(tall.contains("RoomID"), "{tall:?}");
        assert!(tall.contains("主播C·3"), "{tall:?}");
        assert!(tall.contains("主播A·1"), "{tall:?}");
        // 高度 3 只够输入框：历史不画，也不会越界。
        let short = compact(draw_area(Rect::new(0, 0, 60, 3), &app));
        assert!(short.contains("RoomID"), "{short:?}");
        assert!(!short.contains("主播"), "高度不足时只画输入框：{short:?}");
    }
}
