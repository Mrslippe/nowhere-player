//! Live 页（T-0095）：直播独占屏幕的布局与绘制。
//!
//! 72×24 基线：外框 72×24 = 上框 1 + Player 68×22 + 下框 1；
//! Player = 上框 1 + 视觉区 66×19 + 音高 66×1 + 下框 1。
//! 信息行叠加在视觉区最后一行，不额外占高度；小尺寸只压缩视觉区，
//! 不退回底部 LIVE 条。

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::audio::TuiAudio;
use crate::danmaku_overlay;
use crate::input::{display_width, truncate_width};
use crate::player_bar::draw_visual;
use crate::ui::bars;
use crate::ui::chrome::{self, StatusHints, StatusState};

/// 音高条左右各留 1 格（与 Player 页 bar 算法一致）。
pub(crate) const PITCH_SIDE_PADDING: u16 = 2;
/// 开播时长固定宽度 `HH:MM:SS`。
pub(crate) const ELAPSED_WIDTH: u16 = 8;
/// 音量条格数，与 Player 顶边一致。
pub(crate) const VOLUME_CELLS: u16 = 20;
/// `live.buffer_ms` 默认值；与 T-0093 在 `np-core::settings` 定义的默认保持一致。
pub(crate) const DEFAULT_LIVE_BUFFER_MS: u64 = np_core::settings::LIVE_BUFFER_MS_DEFAULT;
/// 右下角缓冲块固定 11 宽：`{current:>4}/{target:>4}ms`（左右各 1 空格已删，T-0101）。
pub(crate) const BUFFER_STATUS_WIDTH: u16 = 11;
/// 状态栏提示；`直播` / `暂停` 与 `空格` 后的动作由当前状态单独拼。
///
/// `d 弹幕` 在隐藏态也常驻，用户不必先知道快捷键才能发现浮层（T-0100c）。
pub(crate) const STATUS_HINTS_PLAYING: &str = " · r 刷新 · d 弹幕 · 空格 暂停 · c 弹幕 · / 命令";
pub(crate) const STATUS_HINTS_PAUSED: &str = " · r 刷新 · d 弹幕 · 空格 继续 · c 弹幕 · / 命令";
/// 终端高度 <10 时弹幕栏禁用，状态栏不再提示 d/c。
pub(crate) const STATUS_HINTS_PLAYING_NO_DANMAKU: &str = " · r 刷新 · 空格 暂停 · / 命令";
pub(crate) const STATUS_HINTS_PAUSED_NO_DANMAKU: &str = " · r 刷新 · 空格 继续 · / 命令";

/// 浮层可见时的状态栏提示：在常驻的 `d 弹幕` 之外补滚动 / 翻页 / 跟随键。
fn danmaku_status_hints(paused: bool) -> String {
    let space = if paused { "继续" } else { "暂停" };
    format!(" · d 弹幕 · ↑/↓ 滚动 · PgUp/PgDn 翻页 · f 跟随 · 空格 {space} · c 弹幕 · / 命令")
}

/// Live 页各区域坐标；72×24 下即卡 §5 的数值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveLayout {
    pub(crate) outer: Rect,
    pub(crate) player: Rect,
    pub(crate) content: Rect,
    pub(crate) visual: Rect,
    pub(crate) pitch: Rect,
    pub(crate) info_y: u16,
    pub(crate) control_y: u16,
    pub(crate) status_y: u16,
}

/// 视觉区高度：`max(1, H-5)`，72×24 下为 19。
pub(crate) fn visual_height(total_height: u16) -> u16 {
    total_height.saturating_sub(5).max(1)
}

/// 计算 Live 页布局；所有尺寸都用饱和减法，极小终端不 panic。
pub(crate) fn layout(area: Rect) -> LiveLayout {
    let player = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let content = Rect::new(
        player.x.saturating_add(1),
        player.y.saturating_add(1),
        player.width.saturating_sub(2),
        player.height.saturating_sub(2),
    );
    let height = visual_height(area.height)
        .min(content.height.saturating_sub(1))
        .max(1);
    let visual = Rect::new(content.x, content.y, content.width, height);
    LiveLayout {
        outer: area,
        player,
        content,
        visual,
        pitch: Rect::new(content.x, visual.bottom(), content.width, 1),
        info_y: visual.bottom().saturating_sub(1),
        control_y: player.bottom().saturating_sub(1),
        status_y: area.bottom().saturating_sub(1),
    }
}

/// 信息行文本：`直播标题 - 分区`；只有一段时不带分隔符。
pub(crate) fn info_text(title: &str, area_name: &str) -> String {
    let title = title.trim();
    let area_name = area_name.trim();
    match (title.is_empty(), area_name.is_empty()) {
        (true, _) => area_name.to_owned(),
        (_, true) => title.to_owned(),
        _ => format!("{title} - {area_name}"),
    }
}

/// 信息行按显示宽度裁剪：标题过长从左侧截断，保留右侧分区。
pub(crate) fn info_line(title: &str, area_name: &str, max_width: usize) -> String {
    let text = info_text(title, area_name);
    if max_width == 0 {
        return String::new();
    }
    if display_width(&text) <= max_width {
        return text;
    }
    truncate_left(&text, max_width)
}

fn truncate_left(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for character in value.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        used += character_width;
        kept.push(character);
    }
    kept.reverse();
    format!("…{}", kept.into_iter().collect::<String>())
}

/// 音高条格数：照搬 Player 页 bar 算法，
/// `blocks = (row.width - 左空 1 - 右空 1 - 时长 8) / 2`。
pub(crate) fn pitch_blocks(row_width: u16) -> u16 {
    row_width.saturating_sub(PITCH_SIDE_PADDING + ELAPSED_WIDTH) / 2
}

/// 右下角缓冲文本：固定 11 宽，current 在前、target 在后。
///
/// T-0101 删掉左右各 1 格空格；数字域仍是固定 4 位，右对齐到控制行末，
/// 数值变化时不会左右跳动。
pub(crate) fn buffer_status_text(current: u64, target: u64) -> String {
    format!("{current:>4}/{target:>4}ms")
}

/// 音高条的填充格数：取能量最强的频段，把频段位置映射到条宽。
///
/// 静音（全部为 0 或低于阈值）返回 0，整条只用未填充色。
pub(crate) fn pitch_fill(spectrum: &[f32], cells: usize) -> usize {
    if cells == 0 || spectrum.is_empty() {
        return 0;
    }
    let mut peak_index = 0usize;
    let mut peak = 0.0f32;
    for (index, value) in spectrum.iter().enumerate() {
        if *value > peak {
            peak = *value;
            peak_index = index;
        }
    }
    if peak < 0.02 {
        return 0;
    }
    let position = (peak_index as f32 + 1.0) / spectrum.len() as f32;
    ((position * cells as f32).round() as usize).clamp(1, cells)
}

/// 绘制 Live 页：独占屏幕，不渲染 Browse/List。
///
/// 视觉区始终按完整宽度绘制（72×24 下 66×19）；弹幕浮层只是叠在右侧
/// 34×18 的一块覆盖层，不再压缩左侧频谱（T-0100b2 / T-0100b3）。
pub(crate) fn draw(frame: &mut ratatui::Frame, app: &App, player: &TuiAudio) {
    let layout = layout(frame.area());
    if layout.outer.width < 8 || layout.outer.height < 4 {
        return;
    }
    let overlay = if app.danmaku_overlay_visible() {
        danmaku_overlay::area(&layout)
    } else {
        None
    };
    draw_outer(frame, &layout, app);
    draw_player_top(frame, &layout, app, player);
    draw_visual_area(frame, layout.visual, app);
    // 常规尺寸让出 Info 行；40×10 浮层用满视觉区时 Info 会被遮住，
    // 宁可不画半截文本（隐藏态仍照常绘制）。
    let info_covered =
        overlay.is_some_and(|panel| panel.y <= layout.info_y && panel.bottom() > layout.info_y);
    if !info_covered {
        draw_info(frame, layout.visual, app, player);
    }
    draw_pitch(frame, &layout, app, player);
    draw_control(frame, &layout, app, player);
    draw_status(frame, &layout, app, player);
    if let Some(overlay) = overlay {
        danmaku_overlay::draw(frame, overlay, app, player);
    }
}

fn draw_outer(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App) {
    chrome::draw_outer_chrome(
        frame,
        layout.outer,
        &app.palette,
        " NOWHERE PLAYER ",
        &app.theme_name,
    );
}

fn draw_player_top(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App, player: &TuiAudio) {
    let area = layout.player;
    if area.width < 6 {
        return;
    }
    let y = area.y;
    let right = area.right();
    let line_style = Style::default().fg(app.palette.line_strong);
    frame.buffer_mut().set_string(area.x, y, "┏", line_style);
    frame
        .buffer_mut()
        .set_string(right.saturating_sub(1), y, "┓", line_style);

    let anchor_x = area.x.saturating_add(2);
    // 右侧结构：末空 1 + Vol 20 格 + ` Vol ` 标签（5 格）。
    let volume = area.width >= VOLUME_CELLS + 16;
    let label_x = if volume {
        right.saturating_sub(2 + VOLUME_CELLS + 5)
    } else {
        right.saturating_sub(1)
    };
    // 主播名左右各 1 格空白。
    let anchor_room = label_x.saturating_sub(anchor_x).saturating_sub(2) as usize;
    let anchor = if anchor_room == 0 {
        String::new()
    } else {
        truncate_width(&player.live_anchor(), anchor_room)
    };
    if !anchor.is_empty() {
        frame.buffer_mut().set_string(
            anchor_x,
            y,
            &anchor,
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        );
    }
    let fill_start = anchor_x
        .saturating_add(display_width(&anchor) as u16)
        .saturating_add(1);
    if label_x > fill_start {
        let fill = "━".repeat(usize::from(label_x - fill_start));
        frame
            .buffer_mut()
            .set_string(fill_start, y, &fill, line_style);
    }
    if !volume {
        return;
    }
    frame
        .buffer_mut()
        .set_string(label_x, y, " Vol ", Style::default().fg(app.palette.muted));
    let filled = ((player.volume().clamp(0.0, 1.0) * f32::from(VOLUME_CELLS)).round() as u16)
        .min(VOLUME_CELLS);
    let gap_x = right.saturating_sub(2);
    let start = gap_x.saturating_sub(VOLUME_CELLS);
    bars::draw_block_bar(
        frame,
        Rect {
            x: start,
            y,
            width: VOLUME_CELLS,
            height: 1,
        },
        &app.palette,
        usize::from(filled),
        usize::from(VOLUME_CELLS),
        1,
    );
    frame.buffer_mut().set_string(
        gap_x,
        y,
        " ",
        Style::default().bg(app.palette.bg.unwrap_or(Color::Reset)),
    );
}

fn draw_visual_area(frame: &mut ratatui::Frame, visual: Rect, app: &App) {
    if visual.width == 0 || visual.height == 0 {
        return;
    }
    let idle = app.spectrum.iter().all(|value| *value == 0.0);
    if idle {
        let line = "░".repeat(usize::from(visual.width));
        let style = Style::default().fg(app.palette.faint);
        for y in visual.y..visual.bottom() {
            frame.buffer_mut().set_string(visual.x, y, &line, style);
        }
        return;
    }
    draw_visual(frame, visual, app, app.palette.accent, app.palette.bg);
}

fn draw_info(frame: &mut ratatui::Frame, visual: Rect, app: &App, player: &TuiAudio) {
    if visual.width < 3 {
        return;
    }
    let max_width = usize::from(visual.width.saturating_sub(2));
    let text = info_line(&player.title(), &player.live_area(), max_width);
    if text.is_empty() {
        return;
    }
    let text_width = display_width(&text) as u16;
    let patch_width = text_width.saturating_add(2).min(visual.width);
    let patch_x = visual.right().saturating_sub(patch_width);
    let bg = app.palette.bg.unwrap_or(Color::Reset);
    let info_y = visual.bottom().saturating_sub(1);
    frame.buffer_mut().set_string(
        patch_x,
        info_y,
        " ".repeat(usize::from(patch_width)),
        Style::default().bg(bg),
    );
    frame.render_widget(
        Paragraph::new(Line::from(TuiSpan::styled(
            text,
            Style::default().fg(app.palette.muted).bg(bg),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        Rect {
            x: patch_x,
            y: info_y,
            width: patch_width,
            height: 1,
        },
    );
}

fn draw_pitch(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App, player: &TuiAudio) {
    let row = layout.pitch;
    if row.width < 12 {
        return;
    }
    let available_blocks = pitch_blocks(row.width);
    let cells = available_blocks.max(1);
    let filled = pitch_fill(&app.spectrum, usize::from(cells));
    // 进度条整体左移一格；开播时长块保持原位。
    bars::draw_block_bar(frame, row, &app.palette, filled, usize::from(cells), 2);
    // 开播时长右对齐到内容区最后一格（整体比音高条再右移一格）。
    let time_x = row.right().saturating_sub(ELAPSED_WIDTH);
    frame.buffer_mut().set_string(
        time_x,
        row.y,
        player.live_elapsed_label(),
        Style::default().fg(app.palette.muted),
    );
}

fn draw_control(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App, player: &TuiAudio) {
    let area = layout.player;
    if area.width < 6 {
        return;
    }
    let y = layout.control_y;
    let line_style = Style::default().fg(app.palette.line_strong);
    let bg = Style::default().bg(app.palette.bg.unwrap_or(Color::Reset));
    frame.buffer_mut().set_string(area.x, y, "┗", line_style);
    frame
        .buffer_mut()
        .set_string(area.right().saturating_sub(1), y, "┛", line_style);
    if area.width < 20 {
        // 极窄终端只保留角与底线，避免按钮压到右角。
        return;
    }
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), y, " ", bg);

    let mut x = area.x.saturating_add(2);
    frame
        .buffer_mut()
        .set_string(x, y, "↻", Style::default().fg(app.palette.accent));
    x = x.saturating_add(1);
    frame.buffer_mut().set_string(x, y, " ", bg);
    x = x.saturating_add(1);
    let playing = player.is_playing() || player.live_state_is_playing();
    let glyph = if playing { "❚❚" } else { "▶" };
    frame.buffer_mut().set_string(
        x,
        y,
        glyph,
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    x = x.saturating_add(display_width(glyph) as u16);
    // 控制钮右侧留 1 格空白再接填充线。
    frame.buffer_mut().set_string(x, y, " ", bg);
    x = x.saturating_add(1);

    let target = player.live_buffer_target_ms();
    let current = player.live_buffer_ms().unwrap_or(0);
    let buffer_text = buffer_status_text(current, target);
    let text_x = area.right().saturating_sub(2 + BUFFER_STATUS_WIDTH);
    let fill_end = text_x.saturating_sub(1);
    if x < fill_end {
        let fill = "━".repeat(usize::from(fill_end - x));
        frame.buffer_mut().set_string(x, y, &fill, line_style);
    }
    if text_x > x {
        frame.buffer_mut().set_string(
            text_x,
            y,
            &buffer_text,
            Style::default().fg(app.palette.muted),
        );
        frame
            .buffer_mut()
            .set_string(area.right().saturating_sub(2), y, " ", bg);
    }
}

fn draw_status(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App, player: &TuiAudio) {
    let state = player.live_state_text();
    let paused = state == "暂停";
    let hints_text = chat_hints(app, paused, player);
    let state = StatusState {
        text: state,
        warn: false,
        require_space: false,
    };
    chrome::draw_status_bar(
        frame,
        layout.outer,
        &app.palette,
        Some(&state),
        StatusHints::Plain(&hints_text),
    );
}

/// 状态栏提示：输入框打开时切换成发送/取消提示，发送结果短暂覆盖快捷键。
///
/// 弹幕连接状态放在最前面做轻提示；`Idle` 不显示，现有 golden 不受影响。
fn chat_hints(app: &App, paused: bool, player: &TuiAudio) -> String {
    let base = if app.live_exit_armed() {
        " · Enter 退出直播 · Esc 取消".to_owned()
    } else if let Some(chat) = app.chat_input.as_ref() {
        match chat.error() {
            Some(error) => format!(" · {error}"),
            None if chat.pending().is_some() => " · 发送中…".to_owned(),
            None => " · Enter 发送 · PgUp/PgDn 翻页 · Esc 收起".to_owned(),
        }
    } else {
        let danmaku_disabled =
            app.danmaku_overlay.screen_height() < danmaku_overlay::DANMAKU_MIN_SCREEN_HEIGHT;
        match app.chat_notice.as_deref() {
            Some(notice) => format!(" · {notice}"),
            None if danmaku_disabled && paused => STATUS_HINTS_PAUSED_NO_DANMAKU.to_owned(),
            None if danmaku_disabled => STATUS_HINTS_PLAYING_NO_DANMAKU.to_owned(),
            None if app.danmaku_overlay_visible() => danmaku_status_hints(paused),
            None if paused => STATUS_HINTS_PAUSED.to_owned(),
            None => STATUS_HINTS_PLAYING.to_owned(),
        }
    };
    match player.danmaku_link().hint() {
        Some(hint) => format!(" · {hint}{base}"),
        None => base,
    }
}

/// Live 页退出确认状态（T-0101）：`Esc` 进入，`Enter` 停止直播，`Esc` 取消。
impl App {
    /// Live 页是否处于退出确认态。
    pub(crate) fn live_exit_armed(&self) -> bool {
        self.danmaku_overlay.exit_armed()
    }

    /// 第一次 `Esc`：只进入确认态，不立即停播。
    pub(crate) fn arm_live_exit(&mut self) -> bool {
        if self.live_exit_armed() {
            return false;
        }
        self.danmaku_overlay.arm_exit();
        self.quit_armed = false;
        self.set_status("Enter确认退出直播 · Esc取消".to_owned());
        true
    }

    /// 确认态再次 `Esc`：取消退出，继续播放。
    pub(crate) fn cancel_live_exit(&mut self) -> bool {
        if !self.danmaku_overlay.clear_exit() {
            return false;
        }
        self.set_transient_status("已取消退出");
        true
    }

    /// 非 `Enter` 路径清除确认态（如 `q` 接管确认、停播后复位）。
    pub(crate) fn clear_live_exit(&mut self) {
        self.danmaku_overlay.clear_exit();
    }

    /// 确认态 `Enter`：停止直播并清空浮层 / 输入状态，队列保留。
    pub(crate) fn confirm_live_exit(&mut self, player: &TuiAudio) -> bool {
        let armed = self.danmaku_overlay.clear_exit();
        self.reset_danmaku_overlay();
        let stopped = player.stop_live();
        if armed && stopped {
            self.set_status("已停止直播".to_owned());
        }
        stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::TuiThemeApi;
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn themed_app() -> App {
        let mut app = App::new();
        let api = TuiThemeApi::new(std::env::temp_dir().join("np-tui-live-theme"));
        app.load_theme(&api, "milkgreen");
        app
    }

    fn live_audio() -> TuiAudio {
        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST香港对抗赛2026");
        audio.live_meta_for_test("测试主播", "游戏赛事");
        audio.set_live_buffer_target(900);
        audio
    }

    fn draw_live(width: u16, height: u16, app: &App, audio: &TuiAudio) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| draw(frame, app, audio))
            .expect("draw");
        terminal
    }

    fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let area = terminal.backend().buffer().area;
        (0..area.width)
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect()
    }

    /// 去掉宽字符留下的空列，便于按文字内容断言。
    fn compact(value: &str) -> String {
        value.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn layout_matches_the_72x24_baseline() {
        let layout = layout(Rect::new(0, 0, 72, 24));
        assert_eq!(layout.outer, Rect::new(0, 0, 72, 24));
        assert_eq!(layout.player, Rect::new(2, 1, 68, 22));
        assert_eq!(layout.content, Rect::new(3, 2, 66, 20));
        assert_eq!(layout.visual, Rect::new(3, 2, 66, 19));
        assert_eq!(layout.pitch, Rect::new(3, 21, 66, 1));
        assert_eq!(layout.info_y, 20);
        assert_eq!(layout.control_y, 22);
        assert_eq!(layout.status_y, 23);
        assert_eq!(visual_height(24), 19);
    }

    #[test]
    fn layout_compresses_the_visual_area_on_small_termals() {
        let medium = layout(Rect::new(0, 0, 60, 20));
        assert_eq!(medium.player, Rect::new(2, 1, 56, 18));
        assert_eq!(medium.visual, Rect::new(3, 2, 54, 15));
        assert_eq!(medium.info_y, 16);
        assert_eq!(medium.pitch.y, 17);
        assert_eq!(medium.control_y, 18);
        assert_eq!(medium.status_y, 19);
        assert_eq!(visual_height(20), 15);

        let small = layout(Rect::new(0, 0, 40, 10));
        assert_eq!(small.player, Rect::new(2, 1, 36, 8));
        assert_eq!(small.visual, Rect::new(3, 2, 34, 5));
        assert_eq!(small.pitch, Rect::new(3, 7, 34, 1));
        assert_eq!(small.control_y, 8);
        assert_eq!(small.status_y, 9);
        assert_eq!(visual_height(10), 5);
    }

    #[test]
    fn visual_area_always_fits_above_the_control_row() {
        for height in 6..=40u16 {
            let layout = layout(Rect::new(0, 0, 72, height));
            assert!(layout.visual.height >= 1, "height {height}");
            assert!(
                layout.visual.bottom() <= layout.control_y,
                "height {height}: visual bottom {} past control {}",
                layout.visual.bottom(),
                layout.control_y
            );
            assert_eq!(layout.pitch.bottom(), layout.control_y, "height {height}");
        }
    }

    #[test]
    fn info_line_keeps_the_area_and_truncates_the_title_from_the_left() {
        let long = "这是一个非常非常长的直播标题".repeat(3);
        let line = info_line(&long, "游戏赛事", 30);
        assert!(line.starts_with('…'), "{line}");
        assert!(line.ends_with("游戏赛事"), "{line}");
        assert!(display_width(&line) <= 30, "{}", display_width(&line));
    }

    #[test]
    fn info_line_without_area_is_just_the_title() {
        assert_eq!(info_line("BLAST", "", 40), "BLAST");
        assert_eq!(info_line("", "游戏赛事", 40), "游戏赛事");
        assert_eq!(info_text("BLAST", "游戏赛事"), "BLAST - 游戏赛事");
    }

    #[test]
    fn pitch_fill_is_silent_on_zero_spectrum_and_scales_with_peak_band() {
        assert_eq!(pitch_fill(&[0.0; 16], 64), 0);
        let mut spectrum = vec![0.0f32; 16];
        spectrum[15] = 1.0;
        assert_eq!(pitch_fill(&spectrum, 64), 64);
        let mut low = vec![0.0f32; 16];
        low[0] = 1.0;
        assert_eq!(pitch_fill(&low, 64), 4);
    }

    #[test]
    fn pitch_blocks_follow_the_player_bar_algorithm() {
        // 终端宽 -> pitch 行宽 = 终端宽 - 6。
        assert_eq!(pitch_blocks(66), 28, "72 宽");
        assert_eq!(pitch_blocks(54), 22, "60 宽");
        assert_eq!(pitch_blocks(34), 12, "40 宽");
        // 80 宽的内容区只有 74 列；(74-1-1-8)/2 = 32。
        assert_eq!(pitch_blocks(74), 32, "80 宽");
    }

    #[test]
    fn pitch_bar_grows_with_a_wider_terminal() {
        let app = themed_app();
        let audio = live_audio();
        let terminal = draw_live(80, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        let cells: Vec<&str> = (3..3 + 32 * 2 - 1)
            .map(|x| buffer[(x, 21)].symbol())
            .collect();
        assert_eq!(
            cells.iter().filter(|symbol| **symbol == "▰").count(),
            32,
            "80 宽应有 32 块"
        );
        // 时间块右对齐到内容区最后一格。
        assert_eq!(buffer[(76, 21)].symbol(), "2");
        assert_eq!(buffer[(69, 21)].symbol(), "0");
        assert_eq!(buffer[(75, 21)].symbol(), "2");
    }

    #[test]
    fn buffer_status_is_a_fixed_eleven_columns_without_side_padding() {
        for (current, target) in [(930, 1000), (1100, 1000), (0, 900)] {
            assert_eq!(
                display_width(&buffer_status_text(current, target)),
                usize::from(BUFFER_STATUS_WIDTH),
                "{current}/{target}"
            );
        }
        assert_eq!(buffer_status_text(930, 1000), " 930/1000ms");
        assert_eq!(buffer_status_text(0, 900), "   0/ 900ms");
    }

    #[test]
    fn paused_live_status_swaps_the_space_action() {
        let app = themed_app();
        let audio = live_audio();
        audio.set_live_paused_for_test(true);
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(status.contains("暂停·r刷新·d弹幕·空格继续"), "{status:?}");
    }

    #[test]
    fn live_page_draws_the_72x24_baseline() {
        let app = themed_app();
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();

        let top = compact(&row(&terminal, 0));
        assert!(top.starts_with("┏NOWHEREPLAYER"), "{top:?}");
        assert!(top.contains("MilkGreen"), "{top:?}");

        assert_eq!(buffer[(2, 1)].symbol(), "┏");
        assert_eq!(buffer[(3, 1)].symbol(), " ");
        assert_eq!(buffer[(69, 1)].symbol(), "┓");
        let player_top = compact(&row(&terminal, 1));
        assert!(player_top.contains("测试主播"), "{player_top:?}");
        assert!(player_top.contains("Vol"), "{player_top:?}");
        assert!(
            row(&terminal, 1).contains('━'),
            "player top must use thick fill"
        );

        let info_row = compact(&row(&terminal, 20));
        assert!(
            info_row.contains("BLAST香港对抗赛2026-游戏赛事"),
            "{info_row:?}"
        );
        // 信息行只叠加视觉区最后一行：右侧补丁区不再是 ░，左侧仍是视觉区。
        let patch_width = display_width(&info_line(&audio.title(), &audio.live_area(), 64)) + 2;
        let patch_x = 69u16 - u16::try_from(patch_width).expect("patch width");
        assert!((patch_x..69).all(|x| buffer[(x, 20)].symbol() != "░"));
        assert!((3..patch_x).any(|x| buffer[(x, 20)].symbol() == "░"));

        let pitch_row = row(&terminal, 21);
        let cells: Vec<&str> = (3..59).map(|x| buffer[(x, 21)].symbol()).collect();
        assert_eq!(cells.len(), 56);
        assert!(
            cells.iter().all(|symbol| *symbol == "▰" || *symbol == " "),
            "{pitch_row:?}"
        );
        assert_eq!(buffer[(3, 21)].symbol(), "▰");
        assert_eq!(buffer[(58, 21)].symbol(), " ");
        assert_eq!(buffer[(59, 21)].symbol(), " ");
        assert_eq!(buffer[(60, 21)].symbol(), " ");
        assert_eq!(buffer[(61, 21)].symbol(), "0");
        assert_eq!(buffer[(68, 21)].symbol(), "2");
        assert!(!pitch_row.contains('▱'));

        assert_eq!(buffer[(2, 22)].symbol(), "┗");
        assert_eq!(buffer[(69, 22)].symbol(), "┛");
        let control = compact(&row(&terminal, 22));
        assert!(control.contains('↻'), "{control:?}");
        assert!(control.contains("0/900ms"), "{control:?}");
        assert_eq!(buffer[(4, 22)].symbol(), "↻");
        assert_eq!(buffer[(5, 22)].symbol(), " ");
        assert_eq!(buffer[(8, 22)].symbol(), " ");
        assert_eq!(buffer[(9, 22)].symbol(), "━");

        let status = compact(&row(&terminal, 23));
        assert!(
            status.contains("直播·r刷新·d弹幕·空格暂停·c弹幕·/命令"),
            "{status:?}"
        );
    }

    #[test]
    fn live_status_keeps_the_hint_line_when_a_message_is_present() {
        let mut app = themed_app();
        app.status = "直播：BLAST香港对抗赛2026 已就绪".to_owned();
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(
            status.contains("直播·r刷新·d弹幕·空格暂停·c弹幕·/命令"),
            "{status:?}"
        );

        let terminal = draw_live(40, 10, &app, &audio);
        let status = compact(&row(&terminal, 9));
        assert!(status.starts_with('┗'), "{status:?}");
        assert!(status.contains("直播·r刷新"), "{status:?}");
    }

    #[test]
    fn live_status_shows_the_danmaku_connection_hint() {
        let app = themed_app();
        let audio = live_audio();
        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Connecting);
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(status.contains("弹幕连接中"), "{status:?}");
        // 连接提示放在最前面，但不能挤掉常驻的 `d 弹幕`。
        assert!(status.contains("d弹幕"), "{status:?}");

        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Connected);
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(status.contains("直播·弹幕已连接·r刷新"), "{status:?}");

        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Reconnecting { attempt: 2 });
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(status.contains("弹幕重连2/6"), "{status:?}");
        assert!(status.contains("d弹幕"), "{status:?}");

        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Failed);
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(status.contains("弹幕未连接"), "{status:?}");
    }

    #[test]
    fn own_server_echo_is_highlighted_and_no_local_echo_is_pushed() {
        let mut app = themed_app();
        let audio = live_audio();
        audio.bili_auth_for_test(Some(np_live::BiliAuth::new(
            vec![
                ("SESSDATA".to_owned(), "test-sessdata".to_owned()),
                ("bili_jct".to_owned(), "test-csrf".to_owned()),
                ("DedeUserID".to_owned(), "42".to_owned()),
            ],
            None,
        )));
        audio.danmaku_result_for_test(Ok(()));
        app.toggle_danmaku_overlay(audio.danmaku_received());
        app.begin_live_chat();
        for character in "你好".chars() {
            app.insert_live_chat_char(character);
        }
        app.submit_live_chat(&audio);
        let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
        assert!(app.apply_live_danmaku_result(generation, result));
        app.clear_chat_notice();
        // 不再本地回显：发送成功只清空 Input。
        assert_eq!(audio.danmaku_received(), 0);
        assert!(audio.danmaku_messages().is_empty());

        // 服务器回显：自己的 uid=42 高亮，其它 uid / 无 uid 保持普通样式。
        let (sender, _links) = audio.danmaku_channel_for_test();
        sender
            .send(danmaku_message_uid(0, "我", "你好", Some(42)))
            .expect("own echo");
        sender
            .send(danmaku_message_uid(1, "路人", "普通", Some(7)))
            .expect("other");
        sender
            .send(danmaku_message_uid(2, "匿名", "无uid", None))
            .expect("no uid");
        assert!(audio.poll_danmaku());
        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
        assert!(app.sync_danmaku(audio.danmaku_received()));

        let terminal = draw_live(72, 24, &app, &audio);
        assert!(compact(&row(&terminal, 14)).contains("我:你好"));
        assert!(compact(&row(&terminal, 15)).contains("路人:普通"));
        assert!(compact(&row(&terminal, 16)).contains("匿名:无uid"));

        let buffer = terminal.backend().buffer();
        // 自己的行：用户名与正文都是 accent + BOLD。
        for x in [37, 41] {
            assert_eq!(buffer[(x, 14)].fg, app.palette.accent, "x {x}");
            assert!(
                buffer[(x, 14)].modifier.contains(Modifier::BOLD),
                "x {x} must be bold"
            );
        }
        // 别人的正文保持普通 fg；无 uid 不误判。
        for (y, x) in [(15, 43), (16, 43)] {
            assert_eq!(buffer[(x, y)].fg, app.palette.fg, "{x},{y}");
            assert!(!buffer[(x, y)].modifier.contains(Modifier::BOLD), "{x},{y}");
        }
        assert_eq!(app.danmaku_overlay.unread(), 0, "跟随态下回显不算未读");
    }

    #[test]
    fn pitch_bar_is_28_spaced_glyphs_with_two_colors_and_elapsed_time() {
        let mut app = themed_app();
        app.spectrum = vec![0.0; 16];
        app.spectrum[7] = 1.0;
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        for index in 0..28u16 {
            let x = 3 + index * 2;
            assert_eq!(buffer[(x, 21)].symbol(), "▰");
            if index + 1 < 28 {
                assert_eq!(buffer[(x + 1, 21)].symbol(), " ");
            }
        }
        let filled = pitch_fill(&app.spectrum, 28);
        assert_eq!(filled, 14);
        assert_eq!(buffer[(3, 21)].fg, app.palette.accent);
        assert_eq!(buffer[(3 + 27 * 2, 21)].fg, app.palette.faint);
        assert_eq!(buffer[(61, 21)].symbol(), "0");
        assert_eq!(buffer[(68, 21)].symbol(), "2");
    }

    #[test]
    fn chat_cursor_and_input_use_cjk_display_widths() {
        let mut app = themed_app();
        app.begin_live_chat();
        app.insert_live_chat_char('你');
        app.insert_live_chat_char('好');
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        // 浮层 Input 行 y18：x37 `>` 提示符，x39 起是 30 格内容区。
        assert_eq!(buffer[(36, 18)].symbol(), "┃");
        assert_eq!(buffer[(37, 18)].symbol(), ">");
        assert_eq!(buffer[(39, 18)].symbol(), "你");
        assert_eq!(buffer[(41, 18)].symbol(), "好");
        assert_eq!(buffer[(43, 18)].symbol(), " ");
        assert_eq!(buffer[(43, 18)].bg, app.palette.fg, "cursor after CJK");
        assert_eq!(buffer[(69, 18)].symbol(), " ", "Input 右侧不再画竖线");
        // 硬件光标（IME 候选栏锚点）落在字符串实际单元格。
        assert_eq!(terminal.backend().cursor_position(), (43, 18).into());
        assert!(terminal.backend().cursor_visible());

        app.move_live_chat_cursor(false);
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(41, 18)].symbol(), "好");
        assert_eq!(buffer[(41, 18)].bg, app.palette.fg, "cursor on a wide char");
        assert_eq!(terminal.backend().cursor_position(), (41, 18).into());

        // 失焦：只保留 `c` 提示，硬件光标隐藏。
        app.cancel_live_chat();
        let terminal = draw_live(72, 24, &app, &audio);
        assert!(!terminal.backend().cursor_visible());
        assert!(compact(&row(&terminal, 18)).contains("按c输入弹幕"));
    }

    #[test]
    fn chat_input_never_overflows_its_30_column_window() {
        let mut app = themed_app();
        app.begin_live_chat();
        for _ in 0..crate::chat_input::CHAT_MAX_CHARS {
            app.insert_live_chat_char('弹');
        }
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        // 光标仍落在 Input 内；右侧完全开口。
        let cursor_cells: Vec<u16> = (37..69)
            .filter(|x| buffer[(*x, 18)].bg == app.palette.fg)
            .collect();
        assert_eq!(cursor_cells.len(), 1, "{cursor_cells:?}");
        assert_eq!(cursor_cells[0], 67);
        assert_eq!(buffer[(69, 18)].symbol(), " ");
        assert_eq!(buffer[(68, 18)].symbol(), " ");
    }

    #[test]
    fn extreme_small_termals_do_not_panic() {
        let app = themed_app();
        let audio = live_audio();
        for (width, height) in [(20u16, 8u16), (12, 6), (8, 6), (4, 6)] {
            let _ = draw_live(width, height, &app, &audio);
        }
    }

    #[test]
    fn danmaku_overlay_is_not_drawn_below_ten_rows() {
        let audio = live_audio();
        let mut app = themed_app();
        app.set_viewport_height(9);
        app.toggle_danmaku_overlay(0);
        app.begin_live_chat();
        assert!(!app.danmaku_overlay_visible());
        assert!(!app.live_chat_active());

        let terminal = draw_live(72, 9, &app, &audio);
        let buffer = terminal.backend().buffer();
        // 视觉区左上仍是普通 Live 页的频谱，不是浮层边框。
        assert_eq!(buffer[(37, 2)].symbol(), "░");
        let status = compact(&row(&terminal, 8));
        assert!(status.contains("r刷新"), "{status:?}");
        assert!(!status.contains("d弹幕"), "{status:?}");
        assert!(!status.contains("c弹幕"), "{status:?}");
    }

    #[test]
    fn small_termals_still_render_the_live_page() {
        for (width, height, visual_height_expected) in [(60, 20, 15u16), (40, 10, 5)] {
            let app = themed_app();
            let audio = live_audio();
            let terminal = draw_live(width, height, &app, &audio);
            let top = row(&terminal, 0);
            assert!(top.contains("NOWHERE PLAYER"), "{width}x{height}: {top:?}");
            assert_eq!(
                layout(Rect::new(0, 0, width, height)).visual.height,
                visual_height_expected
            );
            let player_top = row(&terminal, 1);
            assert!(player_top.contains('┏'), "{width}x{height}: {player_top:?}");
            let status = row(&terminal, height - 1);
            assert!(
                compact(&status).contains("直播"),
                "{width}x{height}: {status:?}"
            );
            assert!(!status.contains("▱"));
        }
    }

    // ---- 弹幕浮层（T-0100b） ----

    fn danmaku_message(index: usize, user: &str, text: &str) -> np_live::DanmakuMessage {
        danmaku_message_uid(index, user, text, None)
    }

    fn danmaku_message_uid(
        index: usize,
        user: &str,
        text: &str,
        uid: Option<u64>,
    ) -> np_live::DanmakuMessage {
        np_live::DanmakuMessage {
            user: user.to_owned(),
            uid,
            text: text.to_owned(),
            color: 16_777_215,
            timestamp: u64::try_from(index).unwrap_or(0),
        }
    }

    fn danmaku_audio(messages: &[(&str, &str)]) -> TuiAudio {
        let audio = live_audio();
        let (sender, _links) = audio.danmaku_channel_for_test();
        for (index, (user, text)) in messages.iter().enumerate() {
            sender
                .send(danmaku_message(index, user, text))
                .expect("fake danmaku channel");
        }
        assert!(audio.poll_danmaku());
        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
        audio
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let area = terminal.backend().buffer().area;
        (0..area.height)
            .map(|y| row(terminal, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn overlay_app(audio: &TuiAudio) -> App {
        let mut app = themed_app();
        app.toggle_danmaku_overlay(audio.danmaku_received());
        app
    }

    #[test]
    fn danmaku_overlay_covers_the_right_side_without_shrinking_the_spectrum() {
        let layout = layout(Rect::new(0, 0, 72, 24));
        let overlay = danmaku_overlay::area(&layout).expect("overlay area");
        assert_eq!(overlay, Rect::new(36, 2, 34, 18));
        assert_eq!(layout.info_y, 20);
        assert_eq!(overlay.bottom(), layout.info_y, "Info y20 不被覆盖");

        let audio = live_audio();
        let app = overlay_app(&audio);
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        // 粗线框：左上 ┏ / 右上 ┛ / 左下 ┗ / 右下 ┓。
        assert_eq!(buffer[(36, 2)].symbol(), "┏");
        assert_eq!(buffer[(69, 2)].symbol(), "┛");
        assert_eq!(buffer[(36, 19)].symbol(), "┗");
        assert_eq!(buffer[(69, 19)].symbol(), "┓");
        // 左缘 ┃ 贯穿消息区与 Input；右侧完全开口，只在上下角收口。
        for y in 3..19 {
            let left = if y == 17 { "┣" } else { "┃" };
            assert_eq!(buffer[(36, y)].symbol(), left, "left border y {y}");
            assert_eq!(buffer[(69, y)].symbol(), " ", "right side y {y}");
        }
        // Input 分隔线 y17：┣ x36、" 输入 " x37..42、━ x43..68，无右端。
        assert_eq!(buffer[(36, 17)].symbol(), "┣");
        assert_eq!(buffer[(37, 17)].symbol(), " ");
        assert_eq!(buffer[(38, 17)].symbol(), "输");
        assert_eq!(buffer[(40, 17)].symbol(), "入");
        assert_eq!(buffer[(42, 17)].symbol(), " ");
        assert_eq!(buffer[(43, 17)].symbol(), "━");
        assert_eq!(buffer[(69, 17)].symbol(), " ");
        // Input 行 y18：x37 `>` 提示符，x38 留空，右侧完全开口。
        assert_eq!(buffer[(36, 18)].symbol(), "┃");
        assert_eq!(buffer[(37, 18)].symbol(), ">");
        assert_eq!(buffer[(38, 18)].symbol(), " ");
        assert_eq!(buffer[(69, 18)].symbol(), " ");

        // 视觉区仍完整绘制：x3..x35 是频谱，浮层只盖住右侧。
        assert_eq!(buffer[(3, 2)].symbol(), "░");
        assert_eq!(buffer[(35, 2)].symbol(), "░");
        // 音高行 / 控制行仍在浮层下方。
        assert_ne!(buffer[(37, 21)].symbol(), "┃");
        assert_eq!(buffer[(68, 21)].symbol(), "2");
        assert_eq!(buffer[(2, 22)].symbol(), "┗");
        assert_eq!(buffer[(69, 22)].symbol(), "┛");
    }

    #[test]
    fn danmaku_overlay_hidden_rendering_is_byte_identical() {
        let app = themed_app();
        let audio = live_audio();
        let hidden = buffer_text(&draw_live(72, 24, &app, &audio));

        let mut visible = themed_app();
        visible.toggle_danmaku_overlay(0);
        let _ = buffer_text(&draw_live(72, 24, &visible, &audio));
        visible.hide_danmaku_overlay();
        let hidden_again = buffer_text(&draw_live(72, 24, &visible, &audio));
        assert_eq!(hidden, hidden_again, "Esc 收起后必须回到隐藏态");

        // 左侧 33 列频谱与整条信息行不因浮层出现而移动。
        visible.toggle_danmaku_overlay(0);
        let with_overlay = draw_live(72, 24, &visible, &audio);
        let hidden_terminal = draw_live(72, 24, &app, &audio);
        for y in 2..21 {
            for x in 3..36 {
                assert_eq!(
                    hidden_terminal.backend().buffer()[(x, y)].symbol(),
                    with_overlay.backend().buffer()[(x, y)].symbol(),
                    "left column {x},{y}"
                );
            }
        }
    }

    #[test]
    fn danmaku_overlay_empty_state_shows_the_placeholder() {
        let audio = live_audio();
        let mut app = overlay_app(&audio);
        // d 打开时聚焦 Input；Esc 退出焦点后显示 `c` 提示。
        app.cancel_live_chat();
        let terminal = draw_live(72, 24, &app, &audio);
        let placeholder = danmaku_overlay::EMPTY_TEXT;
        let mut found = false;
        for y in 3..17 {
            if compact(&row(&terminal, y)).contains(placeholder) {
                found = true;
                assert!(y > 2, "占位文案在正文区而不是标题行");
            }
        }
        assert!(found, "空态必须显示 {placeholder}");
        // 标题行只有标题、可选滚动标记与 4 位同接数。
        let title = row(&terminal, 2);
        assert!(compact(&title).contains("弹幕"), "{title:?}");
        assert!(compact(&title).contains("2000"), "{title:?}");
        assert!(!title.contains(placeholder));
        assert_eq!(terminal.backend().buffer()[(36, 2)].symbol(), "┏");
        // d 打开但不聚焦：Input 行显示 `c` 提示。
        assert!(
            compact(&row(&terminal, 18)).contains("按c输入弹幕"),
            "{:?}",
            row(&terminal, 18)
        );
    }

    #[test]
    fn danmaku_overlay_bottom_aligns_received_messages() {
        let audio = danmaku_audio(&[("阿一", "第一条"), ("阿二", "第二条"), ("阿三", "第三条")]);
        let app = overlay_app(&audio);
        let terminal = draw_live(72, 24, &app, &audio);
        // 3 条消息贴在正文底部（消息区 y3..y16）：y14 / y15 / y16。
        assert!(compact(&row(&terminal, 14)).contains("阿一:第一条"));
        assert!(compact(&row(&terminal, 15)).contains("阿二:第二条"));
        assert!(compact(&row(&terminal, 16)).contains("阿三:第三条"));
        assert_eq!(terminal.backend().buffer()[(36, 16)].symbol(), "┃");
    }

    #[test]
    fn danmaku_overlay_scrolls_history_and_tracks_unread() {
        let audio = live_audio();
        let (sender, _links) = audio.danmaku_channel_for_test();
        for index in 0..25 {
            sender
                .send(danmaku_message(index, "观众", &format!("第{index}条")))
                .expect("send");
        }
        assert!(audio.poll_danmaku());
        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
        let mut app = overlay_app(&audio);
        app.sync_danmaku(audio.danmaku_received());
        // d 打开时聚焦 Input；Esc 退出焦点后再滚动历史。
        app.cancel_live_chat();
        // 先绘制一次，浮层记录换行后的总行数（25 行）。
        let _ = draw_live(72, 24, &app, &audio);
        for _ in 0..3 {
            assert!(crate::danmaku_overlay::handle_key(&mut app, KeyCode::Up));
        }
        // 读历史时又到两条：未读 +2，窗口保持在历史位置。
        for index in 25..27 {
            sender
                .send(danmaku_message(index, "观众", &format!("第{index}条")))
                .expect("send");
        }
        assert!(audio.poll_danmaku());
        audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
        assert!(app.sync_danmaku(audio.danmaku_received()));

        let terminal = draw_live(72, 24, &app, &audio);
        let title = row(&terminal, 2);
        assert!(compact(&title).contains("↑/↓2"), "{title:?}");
        // 窗口外的最新消息不应出现在屏幕里；首行是第 10 条。
        assert!(!compact(&row(&terminal, 16)).contains("第26条"));
        assert!(compact(&row(&terminal, 3)).contains("第10条"));

        // f 回到跟随并清零未读。
        assert!(crate::danmaku_overlay::handle_key(
            &mut app,
            KeyCode::Char('f')
        ));
        let terminal = draw_live(72, 24, &app, &audio);
        assert!(compact(&row(&terminal, 16)).contains("第26条"));
        let title = row(&terminal, 2);
        assert!(!compact(&title).contains("↑/↓"), "{title:?}");
    }

    #[test]
    fn danmaku_overlay_wraps_long_messages_without_an_ellipsis() {
        let long = "这是一条非常非常非常长的中文弹幕用来测试按显示宽度截断不会破坏布局";
        let audio = danmaku_audio(&[
            ("坏\n用户", "\u{1b}[31m红色\n第二行\t制表"),
            ("宽字符", long),
            ("\u{202E}反向", "ab\u{202E}cd\u{200B}\u{FEFF}ef"),
        ]);
        let app = overlay_app(&audio);
        let terminal = draw_live(72, 24, &app, &audio);
        let text = buffer_text(&terminal);
        assert!(!text.contains('\u{1b}'), "escape 字符必须被清理");
        for y in 0..24 {
            assert!(
                !row(&terminal, y).chars().any(char::is_control),
                "第 {y} 行残留控制字符"
            );
        }
        // 长 CJK 换行而不是省略：整段文字仍能拼回，且没有 `…`。
        assert!(!text.contains('…'), "长消息不应出现省略号");
        let mut message_text = String::new();
        for y in 3..17 {
            for x in 37..69 {
                message_text.push_str(terminal.backend().buffer()[(x, y)].symbol());
            }
        }
        let flattened: String = message_text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(
            flattened.contains("这是一条非常非常非常长的中文弹幕用来测试按显示宽度"),
            "{flattened}"
        );
        for y in 3..17 {
            assert_eq!(terminal.backend().buffer()[(36, y)].symbol(), "┃", "y {y}");
            assert_eq!(terminal.backend().buffer()[(69, y)].symbol(), " ", "y {y}");
        }
        // 清理后的用户 / 文本按预期落在正文里。
        assert!(flattened.contains("坏用户:"));
        assert!(flattened.contains("[31m红色第二行制表"));
        assert!(flattened.contains("反向:abcdef"));
    }

    #[test]
    fn danmaku_overlay_online_uses_w_above_9999_and_dashes_when_missing() {
        let audio = live_audio();
        audio.live_online_for_test(Some(12345));
        let app = overlay_app(&audio);
        let terminal = draw_live(72, 24, &app, &audio);
        let title = row(&terminal, 2);
        assert!(title.contains(" 1.2w "), "{title:?}");

        audio.live_online_for_test(None);
        let terminal = draw_live(72, 24, &app, &audio);
        let title = row(&terminal, 2);
        assert!(title.contains(" -- "), "{title:?}");
        assert!(compact(&title).contains("弹幕"), "{title:?}");
    }

    #[test]
    fn danmaku_overlay_compresses_messages_but_keeps_three_input_rows() {
        for (width, height) in [(60u16, 20u16), (40, 10)] {
            let layout = layout(Rect::new(0, 0, width, height));
            let overlay = danmaku_overlay::area(&layout).expect("overlay");
            assert!(overlay.height >= 5);
            assert!(overlay.height - 4 >= 1, "至少 1 行消息");
            if overlay.bottom() > layout.info_y {
                // 40×10 用满视觉区换取消息行；72/60 仍让出 Info 行。
                assert_eq!(overlay.bottom(), layout.info_y + 1);
            }

            let audio = live_audio();
            let mut app = themed_app();
            app.begin_live_chat();
            app.insert_live_chat_char('你');
            let terminal = draw_live(width, height, &app, &audio);
            let buffer = terminal.backend().buffer();
            let bottom = overlay.bottom().saturating_sub(1);
            assert_eq!(buffer[(overlay.x, bottom)].symbol(), "┗");
            assert_eq!(
                buffer[(overlay.right().saturating_sub(1), bottom)].symbol(),
                "┓"
            );
            assert_eq!(buffer[(overlay.x, bottom.saturating_sub(2))].symbol(), "┣");
            let input_y = bottom.saturating_sub(1);
            assert_eq!(buffer[(overlay.x, input_y)].symbol(), "┃");
            assert_eq!(
                buffer[(overlay.right().saturating_sub(1), input_y)].symbol(),
                " "
            );
            assert_eq!(buffer[(overlay.x.saturating_add(1), input_y)].symbol(), ">");
            assert_eq!(
                buffer[(overlay.x.saturating_add(3), input_y)].symbol(),
                "你"
            );
            if width == 40 {
                // 40×10 浮层覆盖 Info 行时宁可不画半截标题。
                assert!(
                    !compact(&row(&terminal, layout.info_y)).contains("BLAST"),
                    "{:?}",
                    row(&terminal, layout.info_y)
                );
            }
        }
    }

    #[test]
    fn d_opens_focused_and_is_a_plain_character_while_focused() {
        let audio = live_audio();
        let mut app = themed_app();
        // d：隐藏 → 打开，并同步聚焦 Input。
        app.toggle_danmaku_overlay(0);
        assert!(app.danmaku_overlay_visible());
        assert!(app.live_chat_active());
        let terminal = draw_live(72, 24, &app, &audio);
        assert_eq!(terminal.backend().buffer()[(37, 18)].symbol(), ">");

        // 焦点存在时 d 是普通字符，不关闭浮层。
        crate::chat_input::handle_chat_key(KeyCode::Char('d'), &mut app, &audio);
        assert!(app.danmaku_overlay_visible());
        assert_eq!(
            app.chat_input.as_ref().map(|chat| chat.text()),
            Some("d".to_owned())
        );
        let terminal = draw_live(72, 24, &app, &audio);
        assert!(compact(&row(&terminal, 18)).contains(">d"));

        // Esc：收起浮层并清空焦点（草稿一起丢弃）。
        crate::chat_input::handle_chat_key(KeyCode::Esc, &mut app, &audio);
        assert!(!app.live_chat_active());
        assert!(!app.danmaku_overlay_visible());
        assert!(app.chat_input.is_none());
        let terminal = draw_live(72, 24, &app, &audio);
        assert_ne!(terminal.backend().buffer()[(36, 18)].symbol(), "┃");
        assert!(!compact(&row(&terminal, 18)).contains("按c输入弹幕"));

        // d 重新打开并聚焦 Input。
        app.toggle_danmaku_overlay(0);
        assert!(app.danmaku_overlay_visible());
        assert!(app.live_chat_active());
    }

    #[test]
    fn chat_input_is_inline_and_never_pops_a_bottom_box() {
        let audio = live_audio();
        let mut app = themed_app();
        app.begin_live_chat();
        app.insert_live_chat_char('你');
        let terminal = draw_live(72, 24, &app, &audio);
        let buffer = terminal.backend().buffer();
        // 旧底部输入框会画在 y20..y22；现在 y20 是 Info、y21 音高、y22 控制行。
        assert_ne!(buffer[(3, 20)].symbol(), "┏");
        assert_ne!(buffer[(3, 21)].symbol(), "┏");
        assert_eq!(buffer[(2, 22)].symbol(), "┗");
        assert_eq!(buffer[(68, 21)].symbol(), "2");
        assert!(compact(&row(&terminal, 20)).contains("BLAST香港对抗赛2026"));
    }
    #[test]
    fn danmaku_overlay_status_hints_add_scroll_keys_only_while_visible() {
        let audio = live_audio();
        let hidden = draw_live(72, 24, &themed_app(), &audio);
        let hidden_status = compact(&row(&hidden, 23));
        // 隐藏态也常驻 `d 弹幕`，用户不打开浮层也能发现入口。
        assert!(hidden_status.contains("d弹幕"), "{hidden_status:?}");
        assert!(hidden_status.contains("c弹幕"), "{hidden_status:?}");

        let app = overlay_app(&audio);
        // d 打开先聚焦 Input：状态栏显示发送/退出提示。
        let focused = draw_live(72, 24, &app, &audio);
        let focused_status = compact(&row(&focused, 23));
        assert!(focused_status.contains("Enter发送"), "{focused_status:?}");
        assert!(focused_status.contains("Esc收起"), "{focused_status:?}");

        // Esc 退出焦点后才轮到滚动键提示。
        let mut app = app;
        app.cancel_live_chat();
        let visible = draw_live(72, 24, &app, &audio);
        let visible_status = compact(&row(&visible, 23));
        assert!(visible_status.contains("d弹幕"), "{visible_status:?}");
        assert!(visible_status.contains("↑/↓滚动"), "{visible_status:?}");
        assert!(visible_status.contains("f跟随"), "{visible_status:?}");
        assert!(visible_status.contains("直播"), "{visible_status:?}");
    }

    #[test]
    fn danmaku_scroll_keys_are_consumed_even_at_the_top() {
        let audio = danmaku_audio(&[("阿一", "第一条")]);
        let mut app = overlay_app(&audio);
        app.cancel_live_chat();
        // 只有一条消息，无法向上滚动，但按键仍归浮层，不落到全局键位。
        assert!(crate::danmaku_overlay::handle_key(&mut app, KeyCode::Up));
        assert_eq!(app.danmaku_overlay.offset(), 0);
        assert!(!crate::danmaku_overlay::handle_key(
            &mut app,
            KeyCode::Char('x')
        ));
    }
}
