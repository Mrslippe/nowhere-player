use ratatui::Terminal;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::audio::TuiAudio;
use crate::input::*;
use crate::logo::{GLYPHS, compose};
use crate::render::*;
use crate::splash::*;
use crate::theme::TuiThemeApi;
use np_core::{Block, BlockStatus, Output};

fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    let mut lines = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_owned());
    }
    lines.join("\n")
}

fn check_golden(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden parent")).expect("mkdir golden");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    if expected == actual {
        return;
    }
    for (index, (left, right)) in expected.lines().zip(actual.lines()).enumerate() {
        if left != right {
            panic!(
                "golden {name} differs at line {}:\n  expected: {left:?}\n  actual:   {right:?}",
                index + 1
            );
        }
    }
    panic!("golden {name} differs in line count");
}

fn snapshot_app(name: &str, width: u16, height: u16, app: &mut App, audio: &TuiAudio) {
    let mut terminal =
        Terminal::new(ratatui::backend::TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| draw(frame, app, audio))
        .expect("draw");
    check_golden(name, &buffer_text(terminal.backend().buffer()));
}

fn library_app() -> App {
    let mut app = App::new();
    let api = TuiThemeApi::new(std::env::temp_dir().join("np-tui-golden-theme"));
    app.load_theme(&api, "milkgreen");
    app.views = vec![ViewData {
        title: "媒体库".to_owned(),
        columns: vec![
            "序号".to_owned(),
            "标题".to_owned(),
            "艺人".to_owned(),
            "时长".to_owned(),
        ],
        rows: (1..=5)
            .map(|index| {
                vec![
                    index.to_string(),
                    format!("测试-{index:02}"),
                    "艺人".to_owned(),
                    "03:00".to_owned(),
                ]
            })
            .collect(),
        refs: Vec::new(),
        durations: Vec::new(),
    }];
    app.view_kind = vec![BrowseKind::Library];
    app.view_depth = vec![0];
    app.focus = Focus::List;
    app.row_index = 1;
    app
}

fn themed_app() -> App {
    let mut app = App::new();
    let api = TuiThemeApi::new(std::env::temp_dir().join("np-tui-golden-theme"));
    app.load_theme(&api, "milkgreen");
    app
}

fn settings_app() -> App {
    let dir = std::env::temp_dir().join(format!(
        "np-tui-golden-settings-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let settings = np_app::SettingsHandle::load(dir.clone());
    let page = crate::settings_page::settings_page(&settings);
    let _ = std::fs::remove_dir_all(dir);
    let mut app = themed_app();
    app.views = vec![page.view];
    app.view_kind = vec![BrowseKind::Settings];
    app.view_depth = vec![0];
    app.settings_fields = page.fields;
    app.focus = Focus::List;
    app.row_index = 0;
    app
}

fn search_app() -> App {
    let mut app = themed_app();
    app.views = vec![ViewData {
        title: "Search".to_owned(),
        ..ViewData::default()
    }];
    app.view_kind = vec![BrowseKind::Search];
    app.view_depth = vec![0];
    app.focus = Focus::List;
    app.search_platform = SearchPlatform::Netease;
    app
}

#[test]
fn golden_library_table() {
    let mut app = library_app();
    let audio = TuiAudio::empty();
    snapshot_app("library_table.txt", 80, 24, &mut app, &audio);
}

#[test]
fn golden_error_block() {
    let mut app = App::new();
    app.blocks.push(Block {
        id: "err".to_owned(),
        input: "play 99".to_owned(),
        started_at: 0,
        duration_ms: Some(1),
        status: BlockStatus::Error,
        outputs: vec![Output::Error {
            code: "E_CMD_ARG".to_owned(),
            message: "媒体库只有 5 条".to_owned(),
            hint: Some("用法：play [<序号>|<路径>]".to_owned()),
            actions: None,
        }],
        footer: None,
        schema_version: 1,
        actions: None,
    });
    let lines = all_lines(&app);
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    check_golden("error_block.txt", &text);
}

#[test]
fn golden_settings_72x24() {
    let mut app = settings_app();
    let audio = TuiAudio::empty();
    snapshot_app("settings_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_search_72x24() {
    let mut app = search_app();
    let audio = TuiAudio::empty();
    snapshot_app("search_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_layout_72x24() {
    let mut app = library_app();
    let audio = TuiAudio::empty();
    snapshot_app("layout_72x24.txt", 72, 24, &mut app, &audio);
}

fn live_audio() -> TuiAudio {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST香港对抗赛2026");
    audio.live_meta_for_test("测试主播", "游戏赛事");
    audio.set_live_buffer_target(900);
    audio
}

#[test]
fn golden_live_72x24() {
    let mut app = themed_app();
    let audio = live_audio();
    snapshot_app("live_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_60x20() {
    let mut app = themed_app();
    let audio = live_audio();
    snapshot_app("live_60x20.txt", 60, 20, &mut app, &audio);
}

#[test]
fn golden_live_40x10() {
    let mut app = themed_app();
    let audio = live_audio();
    snapshot_app("live_40x10.txt", 40, 10, &mut app, &audio);
}

#[test]
fn golden_player_bar() {
    let mut app = library_app();
    app.focus = Focus::List;
    let audio = TuiAudio::empty();
    snapshot_app("player_bar.txt", 100, 30, &mut app, &audio);
}

#[test]
fn golden_status_focus_states() {
    let audio = TuiAudio::empty();
    let mut app = library_app();
    app.focus = Focus::Browse;
    snapshot_app("status_library.txt", 100, 30, &mut app, &audio);
    app.focus = Focus::List;
    snapshot_app("status_list.txt", 100, 30, &mut app, &audio);
}

#[test]
fn golden_splash_sizes() {
    let api = TuiThemeApi::new(std::env::temp_dir().join("np-tui-golden-splash"));
    let mut app = App::new();
    app.load_theme(&api, "milkgreen");
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).expect("terminal");
    draw_splash(&mut terminal, &app).expect("splash 80x24");
    check_golden(
        "splash_80x24.txt",
        &buffer_text(terminal.backend().buffer()),
    );
    let mut small = Terminal::new(ratatui::backend::TestBackend::new(60, 20)).expect("terminal");
    draw_splash(&mut small, &app).expect("splash 60x20");
    check_golden("splash_60x20.txt", &buffer_text(small.backend().buffer()));
}

#[test]
fn glyph_rows_fit_their_slots() {
    for (index, glyph) in GLYPHS.iter().enumerate() {
        for row in glyph.rows {
            assert!(
                UnicodeWidthStr::width(row) <= glyph.width,
                "glyph {index} row {row:?} wider than slot {}",
                glyph.width
            );
        }
    }
}

#[test]
fn composed_rows_are_equal_width() {
    let composed = compose(&GLYPHS);
    let widths: HashSet<usize> = composed
        .iter()
        .map(|row| UnicodeWidthStr::width(row.as_str()))
        .collect();
    assert_eq!(widths.len(), 1, "composed rows differ: {widths:?}");
    assert_eq!(composed.len(), 6);
}

#[test]
fn splash_threshold_is_computed_from_art_width() {
    assert_eq!(splash_art(60), "NOWHERE");
    assert!(splash_art(80).contains('█'));
}

fn chat_auth() -> np_live::BiliAuth {
    np_live::BiliAuth::new(
        vec![
            ("SESSDATA".to_owned(), "test-sessdata".to_owned()),
            ("bili_jct".to_owned(), "test-csrf".to_owned()),
            ("DedeUserID".to_owned(), "42".to_owned()),
        ],
        None,
    )
}

fn chat_pending_app(audio: &TuiAudio) -> App {
    let mut app = themed_app();
    app.begin_live_chat();
    for character in "晚上好，测试弹幕".chars() {
        app.insert_live_chat_char(character);
    }
    audio.bili_auth_for_test(Some(chat_auth()));
    audio.danmaku_result_for_test(Ok(()));
    app.submit_live_chat(audio);
    app
}

#[test]
fn golden_live_chat_input_72x24() {
    let mut app = themed_app();
    app.begin_live_chat();
    for character in "晚上好，测试弹幕".chars() {
        app.insert_live_chat_char(character);
    }
    let audio = live_audio();
    snapshot_app("live_chat_input_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_chat_pending_72x24() {
    let audio = live_audio();
    let mut app = chat_pending_app(&audio);
    snapshot_app("live_chat_pending_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_chat_error_72x24() {
    let audio = live_audio();
    let mut app = themed_app();
    app.begin_live_chat();
    for character in "晚上好，测试弹幕".chars() {
        app.insert_live_chat_char(character);
    }
    audio.bili_auth_for_test(Some(chat_auth()));
    audio.danmaku_result_for_test(Err("发送弹幕过于频繁，请稍后再试".to_owned()));
    app.submit_live_chat(&audio);
    let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
    assert!(app.apply_live_danmaku_result(generation, result));
    snapshot_app("live_chat_error_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_chat_sent_72x24() {
    let audio = live_audio();
    let mut app = chat_pending_app(&audio);
    let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
    assert!(app.apply_live_danmaku_result(generation, result));
    snapshot_app("live_chat_sent_72x24.txt", 72, 24, &mut app, &audio);
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

fn overlay_audio(messages: &[(&str, &str)]) -> TuiAudio {
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

fn overlay_app(audio: &TuiAudio) -> App {
    let mut app = themed_app();
    app.toggle_danmaku_overlay(audio.danmaku_received());
    app
}

/// 隐藏态必须与 T-0098 的 72×24 基线逐字节一致：打开再关闭也不能留痕。
#[test]
fn golden_live_danmaku_hidden_is_the_baseline() {
    let mut app = themed_app();
    let audio = live_audio();
    app.toggle_danmaku_overlay(audio.danmaku_received());
    app.toggle_danmaku_overlay(audio.danmaku_received());
    snapshot_app("live_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_empty_72x24() {
    let audio = live_audio();
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_empty_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_empty_60x20() {
    let audio = live_audio();
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_empty_60x20.txt", 60, 20, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_empty_40x10() {
    let audio = live_audio();
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_empty_40x10.txt", 40, 10, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_messages_72x24() {
    let audio = overlay_audio(&[
        ("小明", "晚上好"),
        ("主播", "欢迎来到直播间"),
        ("观众A", "这首歌唱得真好听！"),
        ("观众B", "打卡"),
    ]);
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_messages_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_messages_60x20() {
    let audio = overlay_audio(&[
        ("小明", "晚上好"),
        ("主播", "欢迎来到直播间"),
        ("观众A", "这首歌唱得真好听！"),
        ("观众B", "打卡"),
    ]);
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_messages_60x20.txt", 60, 20, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_messages_40x10() {
    // 40×10 只有 1 行消息：底部对齐后只显示最新一条。
    let audio = overlay_audio(&[("小明", "晚上好"), ("观众B", "打卡")]);
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_messages_40x10.txt", 40, 10, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_scrolled_72x24() {
    let audio = live_audio();
    let (sender, _links) = audio.danmaku_channel_for_test();
    for index in 0..40 {
        sender
            .send(danmaku_message(index, "观众", &format!("第{index}条弹幕")))
            .expect("send");
    }
    assert!(audio.poll_danmaku());
    audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
    let mut app = overlay_app(&audio);
    app.sync_danmaku(audio.danmaku_received());
    // d 打开时聚焦 Input；Esc 退出焦点后才滚动历史。
    app.cancel_live_chat();
    // 先绘制一次，让浮层记录换行后的总行数，再滚动。
    let mut warmup = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).expect("terminal");
    warmup
        .draw(|frame| draw(frame, &mut app, &audio))
        .expect("warmup draw");
    // 读历史时又到两条：右上角显示 `↑/↓ 2`。
    for index in 40..42 {
        sender
            .send(danmaku_message(index, "观众", &format!("第{index}条弹幕")))
            .expect("send");
    }
    assert!(audio.poll_danmaku());
    audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
    for _ in 0..5 {
        assert!(app.scroll_danmaku(true));
    }
    assert!(app.sync_danmaku(audio.danmaku_received()));
    snapshot_app("live_danmaku_scrolled_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_control_72x24() {
    let audio = overlay_audio(&[
        ("坏\n用户", "\u{1b}[31m红色\n第二行\t制表"),
        (
            "宽字符",
            "这是一条非常非常非常长的中文弹幕用来测试按显示宽度截断不会破坏布局",
        ),
        ("\u{202E}反向", "ab\u{202E}cd\u{200B}\u{FEFF}ef"),
    ]);
    let mut app = overlay_app(&audio);
    snapshot_app("live_danmaku_control_72x24.txt", 72, 24, &mut app, &audio);
}

#[test]
fn golden_live_danmaku_echo_72x24() {
    let audio = live_audio();
    audio.bili_auth_for_test(Some(chat_auth()));
    audio.danmaku_result_for_test(Ok(()));
    let mut app = overlay_app(&audio);
    app.begin_live_chat();
    for character in "晚上好".chars() {
        app.insert_live_chat_char(character);
    }
    app.submit_live_chat(&audio);
    let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
    assert!(app.apply_live_danmaku_result(generation, result));
    app.clear_chat_notice();
    // 服务器回显（uid=42）才进入浮层；本地不再 push。
    let (sender, _links) = audio.danmaku_channel_for_test();
    sender
        .send(danmaku_message_uid(0, "我", "晚上好", Some(42)))
        .expect("server echo");
    assert!(audio.poll_danmaku());
    audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);
    app.sync_danmaku(audio.danmaku_received());
    snapshot_app("live_danmaku_echo_72x24.txt", 72, 24, &mut app, &audio);
}

/// 卡面 T-0100b3 §0：浮层 x36/w34、内容 32、右侧只在上下角与 Input 行收口。
#[test]
fn danmaku_overlay_frame_matches_the_strict_card() {
    let audio = overlay_audio(&[("小明", "晚上好")]);
    let mut app = overlay_app(&audio);
    let mut terminal =
        Terminal::new(ratatui::backend::TestBackend::new(72, 24)).expect("test terminal");
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let symbol = |x: u16, y: u16| buffer[(x, y)].symbol();

    // 顶边 y2：┏ x36，━ 填充，┛ x69；标题 `弹幕` 落在 37..41。
    assert_eq!(symbol(36, 2), "┏");
    assert_eq!(symbol(69, 2), "┛");
    assert_eq!(symbol(37, 2), " ");
    assert_eq!(symbol(38, 2), "弹");
    assert_eq!(symbol(40, 2), "幕");
    for x in 42..63 {
        assert_eq!(symbol(x, 2), "━", "top fill x {x}");
    }
    // 同接数固定 4 位、两侧空格：x63..x68 是 ` 2000 `。
    assert_eq!(symbol(63, 2), " ");
    assert_eq!(symbol(64, 2), "2");
    assert_eq!(symbol(67, 2), "0");
    assert_eq!(symbol(68, 2), " ");

    // 消息行 y3..y16：只有左竖线 ┃，右侧 x69 不画框。
    for y in 3..17 {
        assert_eq!(symbol(36, y), "┃", "left border y {y}");
        assert_eq!(symbol(69, y), " ", "right side stays open y {y}");
    }

    // Input 分隔 y17：┣ x36 + " 输入" x37..41 + ━ 42..68，无右端。
    assert_eq!(symbol(36, 17), "┣");
    assert_eq!(symbol(37, 17), " ");
    assert_eq!(symbol(38, 17), "输");
    assert_eq!(symbol(40, 17), "入");
    assert_eq!(buffer[(38, 17)].fg, app.palette.accent, "输入标签用 accent");
    for x in 42..69 {
        assert_eq!(symbol(x, 17), "━", "separator fill x {x}");
    }
    assert_eq!(symbol(69, 17), " ");

    // Input y18：x37 `>` 提示符、x38 留空、x39..x68 内容、x69 浅色内竖线。
    assert_eq!(symbol(36, 18), "┃");
    assert_eq!(symbol(37, 18), ">");
    assert_eq!(symbol(38, 18), " ");
    assert_eq!(symbol(69, 18), "│");
    assert_eq!(buffer[(69, 18)].fg, app.palette.faint, "Input 内竖线是浅色");

    // 底边 y19：┗ x36，━ 37..68，┓ x69。
    assert_eq!(symbol(36, 19), "┗");
    assert_eq!(symbol(69, 19), "┓");
    for x in 37..69 {
        assert_eq!(symbol(x, 19), "━", "bottom fill x {x}");
    }

    // 内容区宽 32：x37 起是消息正文，x68 是最后一格。
    assert_eq!(symbol(37, 16), "小");
    assert_eq!(symbol(39, 16), "明");
    assert_eq!(symbol(43, 16), "晚");
    assert_eq!(symbol(47, 16), "好");
}
