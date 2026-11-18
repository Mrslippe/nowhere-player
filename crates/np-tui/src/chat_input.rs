//! Live 页弹幕输入浮层：状态机、按键与绘制。
//!
//! `ChatInput` 只保存文本、光标、pending 与错误；网络发送与结果回填仍由
//! [`App`] 通过 [`TuiAudio::send_live_danmaku`] 完成。

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::Paragraph;
#[cfg(test)]
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::audio::TuiAudio;
use crate::input::{display_width, input_window, truncate_width};

/// 弹幕输入上限：B 站单条弹幕最多 20 个字符（CJK 按 1 个字符计）。
pub(crate) const CHAT_MAX_CHARS: usize = 20;
/// 输入框内容行：`› ` + 输入区 + 右边框。
const CHAT_PROMPT: &str = "› ";

/// Live 页底部弹幕输入框的状态机。
///
/// 只保存文本、光标、pending 与错误；网络发送由 [`App`] 通过
/// [`TuiAudio::send_live_danmaku`] 发起，本结构不接触登录凭证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatInput {
    value: Vec<char>,
    cursor: usize,
    pending: Option<u64>,
    /// 正在发送的文本；成功后用于本地回显 `我:`。
    pending_text: Option<String>,
    error: Option<String>,
}

/// `handle_key` 的结果；发送与结果回填仍由调用方接线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChatOutcome {
    Handled,
    Submit(String),
    Cancel,
}

impl ChatInput {
    pub(crate) fn new() -> Self {
        Self {
            value: Vec::new(),
            cursor: 0,
            pending: None,
            pending_text: None,
            error: None,
        }
    }

    pub(crate) fn text(&self) -> String {
        self.value.iter().collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.value.iter().all(|character| character.is_whitespace())
    }

    pub(crate) fn pending(&self) -> Option<u64> {
        self.pending
    }

    pub(crate) fn pending_text(&self) -> Option<&str> {
        self.pending_text.as_deref()
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn set_pending(&mut self, generation: u64, message: String) {
        self.pending = Some(generation);
        self.pending_text = Some(message);
        self.error = None;
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.pending = None;
        self.pending_text = None;
        self.error = Some(error.into());
    }

    /// 插入字符；超过 20 个字符、控制字符或正在发送时忽略。
    pub(crate) fn insert(&mut self, character: char) {
        if self.pending.is_some() || character.is_control() || self.value.len() >= CHAT_MAX_CHARS {
            return;
        }
        self.value.insert(self.cursor, character);
        self.cursor += 1;
        self.error = None;
    }

    pub(crate) fn backspace(&mut self) {
        if self.pending.is_some() || self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        self.value.remove(self.cursor);
        self.error = None;
    }

    pub(crate) fn move_cursor(&mut self, right: bool) {
        if self.pending.is_some() {
            return;
        }
        if right {
            if self.cursor < self.value.len() {
                self.cursor += 1;
            }
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    /// 输入框打开时的按键；返回前不会进入全局 / Live 键位解析。
    pub(crate) fn handle_key(&mut self, code: KeyCode) -> ChatOutcome {
        match code {
            KeyCode::Esc => ChatOutcome::Cancel,
            // pending 期间重复 Enter 直接忽略，由发送方保持幂等。
            KeyCode::Enter if self.pending.is_some() => ChatOutcome::Handled,
            KeyCode::Enter => ChatOutcome::Submit(self.text()),
            KeyCode::Backspace => {
                self.backspace();
                ChatOutcome::Handled
            }
            KeyCode::Left => {
                self.move_cursor(false);
                ChatOutcome::Handled
            }
            KeyCode::Right => {
                self.move_cursor(true);
                ChatOutcome::Handled
            }
            KeyCode::Char(character) if !character.is_control() => {
                self.insert(character);
                ChatOutcome::Handled
            }
            _ => ChatOutcome::Handled,
        }
    }

    /// 光标前的显示宽度；CJK 占两列，用于测试与渲染定位。
    #[cfg(test)]
    pub(crate) fn cursor_column(&self) -> usize {
        self.value[..self.cursor]
            .iter()
            .map(|character| UnicodeWidthChar::width(*character).unwrap_or(0))
            .sum()
    }
}

/// 处理输入框打开时的按键；返回前不会进入全局 / Live 键位解析。
pub(crate) fn handle_chat_key(code: KeyCode, app: &mut App, player: &TuiAudio) {
    let outcome = match app.chat_input.as_mut() {
        Some(chat) => chat.handle_key(code),
        None => return,
    };
    match outcome {
        ChatOutcome::Handled => {}
        ChatOutcome::Cancel => {
            app.cancel_live_chat();
        }
        ChatOutcome::Submit(message) => app.submit_live_chat_message(message, player),
    }
}

/// 弹幕浮层内嵌的 Input 行（T-0100b2）：宽 30，高 1，位于分隔线与底边之间。
///
/// 有焦点时绘制光标；未聚焦或无输入状态时显示 `c` 提示，不再弹出底部
/// 3 行输入框。`area` 已经是边框内的可用内容区。
pub(crate) fn draw_input(frame: &mut ratatui::Frame, area: Rect, app: &App) {
    if area.width < 4 || area.height == 0 {
        return;
    }
    let bg = app.palette.bg.unwrap_or(Color::Reset);
    let Some(chat) = app.chat_input.as_ref() else {
        frame.buffer_mut().set_string(
            area.x,
            area.y,
            CHAT_PROMPT,
            Style::default().fg(app.palette.accent).bg(bg),
        );
        let hint_width = usize::from(area.width.saturating_sub(2));
        let hint = truncate_width("按 c 输入弹幕", hint_width);
        frame.buffer_mut().set_string(
            area.x.saturating_add(2),
            area.y,
            &hint,
            Style::default().fg(app.palette.faint).bg(bg),
        );
        return;
    };

    let focused = app.danmaku_overlay.input_focused();
    let input_width = usize::from(area.width.saturating_sub(2));
    let (start, end) = input_window(&chat.value, chat.cursor, input_width);
    let cursor_style = Style::default()
        .bg(app.palette.fg)
        .fg(app.palette.bg.unwrap_or(Color::Reset))
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![TuiSpan::styled(
        CHAT_PROMPT,
        Style::default().fg(app.palette.accent).bg(bg),
    )];
    for (index, character) in chat.value.iter().enumerate().take(end).skip(start) {
        let style = if focused && index == chat.cursor {
            cursor_style
        } else {
            Style::default().fg(app.palette.fg).bg(bg)
        };
        spans.push(TuiSpan::styled(character.to_string(), style));
    }
    if focused && chat.cursor == chat.value.len() {
        spans.push(TuiSpan::styled(" ", cursor_style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg).fg(app.palette.fg)),
        area,
    );

    if chat.pending().is_some() {
        let label = " 发送中… ";
        let label_width = display_width(label) as u16;
        let x = area.right().saturating_sub(label_width);
        if x > area.x.saturating_add(2) {
            frame.buffer_mut().set_string(
                x,
                area.y,
                label,
                Style::default()
                    .bg(bg)
                    .fg(app.palette.accent)
                    .add_modifier(Modifier::BOLD),
            );
        }
    }
}

/// Live 页弹幕输入的状态操作（T-0097 / T-0100b2）；发送结果回填留在 App。
impl App {
    // ---- Live 页弹幕输入（T-0097） ----

    /// 输入焦点是否在浮层内嵌 Input 上；事件循环据此优先路由字符键。
    pub(crate) fn live_chat_active(&self) -> bool {
        self.chat_input.is_some() && self.danmaku_overlay.input_focused()
    }

    /// `c`：打开浮层并聚焦 Input（T-0100b2）；已打开时只重新聚焦。
    pub(crate) fn begin_live_chat(&mut self) -> bool {
        self.danmaku_overlay.show();
        self.danmaku_overlay.focus_input();
        if self.chat_input.is_none() {
            self.chat_input = Some(ChatInput::new());
        }
        self.chat_notice = None;
        self.chat_notice_expires_at = None;
        true
    }

    /// `Esc`：退出输入焦点，浮层保持打开；草稿一并丢弃。
    pub(crate) fn cancel_live_chat(&mut self) -> bool {
        let had = self.chat_input.take().is_some();
        self.danmaku_overlay.blur_input();
        had
    }

    pub(crate) fn insert_live_chat_char(&mut self, character: char) {
        if let Some(chat) = self.chat_input.as_mut() {
            chat.insert(character);
        }
    }

    #[cfg(test)]
    pub(crate) fn move_live_chat_cursor(&mut self, right: bool) {
        if let Some(chat) = self.chat_input.as_mut() {
            chat.move_cursor(right);
        }
    }

    /// 提交输入框：空内容、未登录都不发起请求；pending 期间重复 Enter 直接忽略。
    #[cfg(test)]
    pub(crate) fn submit_live_chat(&mut self, player: &TuiAudio) {
        let Some(message) = self.chat_input.as_ref().map(ChatInput::text) else {
            return;
        };
        self.submit_live_chat_message(message, player);
    }

    fn submit_live_chat_message(&mut self, message: String, player: &TuiAudio) {
        let Some(chat) = self.chat_input.as_ref() else {
            return;
        };
        if chat.pending().is_some() {
            return;
        }
        if chat.is_empty() {
            self.set_live_chat_error("弹幕内容不能为空");
            return;
        }
        if !player.has_bili_auth() {
            self.set_live_chat_error("未登录，请先 /login bili");
            return;
        }
        match player.send_live_danmaku(&message) {
            Ok(generation) => {
                if let Some(chat) = self.chat_input.as_mut() {
                    chat.set_pending(generation, message);
                }
                self.chat_notice = None;
            }
            Err(error) => self.set_live_chat_error(error),
        }
    }

    /// 收下后台发送结果；只有当前 pending 的 generation 才能改变输入框状态。
    ///
    /// 发送成功时把文本回显进 TuiAudio 的同一个环形缓冲（`我:`），
    /// 不等待服务器把这条弹幕再推回来。
    pub(crate) fn apply_live_danmaku_result(
        &mut self,
        generation: u64,
        result: Result<(), String>,
        player: &TuiAudio,
    ) -> bool {
        let Some(chat) = self.chat_input.as_mut() else {
            return false;
        };
        if chat.pending() != Some(generation) {
            return false;
        }
        match result {
            Ok(()) => {
                let echoed = chat.pending_text().map(str::to_owned);
                // 浮层内联 Input：发送成功后清空文本但保持焦点，方便连发。
                if self.danmaku_overlay.input_focused() {
                    self.chat_input = Some(ChatInput::new());
                } else {
                    self.chat_input = None;
                }
                if let Some(echoed) = echoed.filter(|text| !text.trim().is_empty()) {
                    player.push_local_danmaku("我", &echoed);
                }
                self.show_chat_notice("弹幕已发送");
            }
            Err(error) => chat.set_error(format!("发送失败：{error}")),
        }
        true
    }

    fn set_live_chat_error(&mut self, error: impl Into<String>) {
        if let Some(chat) = self.chat_input.as_mut() {
            chat.set_error(error);
        }
    }

    pub(crate) fn clear_chat_notice(&mut self) {
        self.chat_notice = None;
        self.chat_notice_expires_at = None;
    }
}
