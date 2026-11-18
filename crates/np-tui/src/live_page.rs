//! Live 页（T-0095）：直播独占屏幕的布局与绘制。
//!
//! 72×24 基线：外框 72×24 = 上框 1 + Player 68×22 + 下框 1；
//! Player = 上框 1 + 视觉区 66×19 + 音高 66×1 + 下框 1。
//! 信息行叠加在视觉区最后一行，不额外占高度；小尺寸只压缩视觉区，
//! 不退回底部 LIVE 条。

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TuiSpan};
use ratatui::widgets::{Block as TuiBlock, BorderType, Borders, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::audio::TuiAudio;
use crate::glyphs;
use crate::input::{display_width, truncate_width};
use crate::player_bar::draw_visual;

/// 音高条格数（72×24 基线）；更窄时按内容宽收缩。
pub(crate) const PITCH_CELLS: u16 = 28;
/// 开播时长固定宽度 `HH:MM:SS`。
pub(crate) const ELAPSED_WIDTH: u16 = 8;
/// 音量条格数，与 Player 顶边一致。
pub(crate) const VOLUME_CELLS: u16 = 20;
/// `live.buffer_ms` 默认值；与 T-0093 在 `np-core::settings` 定义的默认保持一致。
pub(crate) const DEFAULT_LIVE_BUFFER_MS: u64 = np_core::settings::LIVE_BUFFER_MS_DEFAULT;
/// 状态栏快捷键；`LIVE`/`PAUSE` 由当前状态单独绘制。
pub(crate) const STATUS_HINTS: &str = " · r refresh · space pause · c chat · / command";

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
pub(crate) fn draw(frame: &mut ratatui::Frame, app: &App, player: &TuiAudio) {
    let layout = layout(frame.area());
    if layout.outer.width < 8 || layout.outer.height < 4 {
        return;
    }
    draw_outer(frame, &layout, app);
    draw_player_top(frame, &layout, app, player);
    draw_visual_area(frame, &layout, app);
    draw_info(frame, &layout, app, player);
    draw_pitch(frame, &layout, app, player);
    draw_control(frame, &layout, app, player);
    draw_status(frame, &layout, app, player);
}

fn draw_outer(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App) {
    let outer = TuiBlock::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(app.palette.muted))
        .style(
            Style::default()
                .bg(app.palette.bg.unwrap_or(Color::Reset))
                .fg(app.palette.fg),
        );
    frame.render_widget(outer.title(" NOWHERE PLAYER "), layout.outer);
    let theme = format!(" {} ", app.theme_name);
    let theme_x = layout
        .outer
        .right()
        .saturating_sub(display_width(theme.as_str()) as u16 + 1);
    frame.buffer_mut().set_string(
        theme_x,
        layout.outer.y,
        &theme,
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::ITALIC),
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
    let bright = Style::default().fg(app.palette.accent);
    let dark = Style::default().fg(app.palette.faint);
    for index in 0..VOLUME_CELLS {
        frame.buffer_mut().set_string(
            start.saturating_add(index),
            y,
            glyphs::PROGRESS_DONE,
            if index < filled { bright } else { dark },
        );
    }
    frame.buffer_mut().set_string(
        gap_x,
        y,
        " ",
        Style::default().bg(app.palette.bg.unwrap_or(Color::Reset)),
    );
}

fn draw_visual_area(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App) {
    let visual = layout.visual;
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

fn draw_info(frame: &mut ratatui::Frame, layout: &LiveLayout, app: &App, player: &TuiAudio) {
    let visual = layout.visual;
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
    frame.buffer_mut().set_string(
        patch_x,
        layout.info_y,
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
            y: layout.info_y,
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
    let available_blocks = row.width.saturating_sub(2 + ELAPSED_WIDTH) / 2;
    let cells = PITCH_CELLS.min(available_blocks).max(1);
    let filled = pitch_fill(&app.spectrum, usize::from(cells));
    let bright = Style::default().fg(app.palette.accent);
    let dark = Style::default().fg(app.palette.faint);
    // 进度条整体左移一格；开播时长块保持原位。
    let mut x = row.x;
    for index in 0..cells {
        frame.buffer_mut().set_string(
            x,
            row.y,
            glyphs::PROGRESS_DONE,
            if usize::from(index) < filled {
                bright
            } else {
                dark
            },
        );
        x = x.saturating_add(2);
    }
    let time_x = row.right().saturating_sub(1 + ELAPSED_WIDTH);
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
    let actual = player.live_buffer_ms().unwrap_or(0);
    let buffer_text = format!("{target}/{actual}ms");
    let buffer_width = display_width(&buffer_text) as u16;
    let text_x = area.right().saturating_sub(2 + buffer_width);
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
    let area = layout.outer;
    if area.width < 8 {
        return;
    }
    let y = layout.status_y;
    let edge = Style::default().fg(app.palette.muted);
    // 先把外框下边线补成连续线，避免上一帧的宽字符残留在缓冲里。
    frame.buffer_mut().set_string(
        area.x.saturating_add(1),
        y,
        "━".repeat(usize::from(area.width.saturating_sub(2))),
        edge,
    );
    // 状态栏左右各空 1 格；最左只显示 LIVE / PAUSE。
    frame
        .buffer_mut()
        .set_string(area.x.saturating_add(1), y, " ", edge);
    let mut x = area.x.saturating_add(2);
    let max_x = area.right().saturating_sub(2);
    let state = player.live_short_state();
    frame.buffer_mut().set_string(
        x,
        y,
        state,
        Style::default()
            .fg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    );
    x = x.saturating_add(display_width(state) as u16);
    let available = usize::from(max_x.saturating_sub(x));
    let hints = truncate_width(STATUS_HINTS, available);
    if hints.is_empty() {
        return;
    }
    frame
        .buffer_mut()
        .set_string(x, y, &hints, Style::default().fg(app.palette.muted));
    x = x.saturating_add(display_width(&hints) as u16);
    frame.buffer_mut().set_string(x, y, " ", edge);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::TuiThemeApi;
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
        assert_eq!(buffer[(68, 21)].symbol(), " ");
        assert_eq!(buffer[(60, 21)].symbol(), "0");
        assert_eq!(buffer[(67, 21)].symbol(), "2");
        assert!(!pitch_row.contains('▱'));

        assert_eq!(buffer[(2, 22)].symbol(), "┗");
        assert_eq!(buffer[(69, 22)].symbol(), "┛");
        let control = compact(&row(&terminal, 22));
        assert!(control.contains('↻'), "{control:?}");
        assert!(control.contains("900/0ms"), "{control:?}");
        assert_eq!(buffer[(4, 22)].symbol(), "↻");
        assert_eq!(buffer[(5, 22)].symbol(), " ");
        assert_eq!(buffer[(8, 22)].symbol(), " ");
        assert_eq!(buffer[(9, 22)].symbol(), "━");

        let status = compact(&row(&terminal, 23));
        assert!(
            status.contains("LIVE·rrefresh·spacepause·cchat·/command"),
            "{status:?}"
        );
        assert!(!status.contains("直播"), "{status:?}");
    }

    #[test]
    fn live_status_keeps_the_hint_line_when_a_message_is_present() {
        let mut app = themed_app();
        app.status = "直播：BLAST香港对抗赛2026 已就绪".to_owned();
        let audio = live_audio();
        let terminal = draw_live(72, 24, &app, &audio);
        let status = compact(&row(&terminal, 23));
        assert!(
            status.contains("LIVE·rrefresh·spacepause·cchat·/command"),
            "{status:?}"
        );
        assert!(!status.contains("直播"), "{status:?}");

        let terminal = draw_live(40, 10, &app, &audio);
        let status = compact(&row(&terminal, 9));
        assert!(status.starts_with('┗'), "{status:?}");
        assert!(status.contains("LIVE·rrefresh"), "{status:?}");
        assert!(!status.contains("直播"), "{status:?}");
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
        assert_eq!(buffer[(60, 21)].symbol(), "0");
        assert_eq!(buffer[(67, 21)].symbol(), "2");
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
            assert!(status.contains("LIVE"), "{width}x{height}: {status:?}");
            assert!(!status.contains("▱"));
        }
    }
}
