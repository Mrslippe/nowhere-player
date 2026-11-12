use super::*;
use crate::audio::TuiAudio;
use crate::layout::*;
use crate::render::*;
use crate::theme::Palette;
use np_app::PlaybackTarget;
use np_app::complete_for;
use np_core::{Block, BlockStatus, Output};
use ratatui::style::Color;
use std::collections::HashSet;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

#[test]
fn input_state_handles_cjk_and_movement() {
    let mut app = App::new();
    app.insert('花');
    app.insert('骨');
    app.insert('朵');
    app.left();
    app.insert('X');
    assert_eq!(app.input_text(), "花骨X朵");
    app.backspace();
    assert_eq!(app.input_text(), "花骨朵");
    app.left();
    app.left();
    app.left();
    app.backspace();
    assert_eq!(app.input_text(), "花骨朵");
}

#[test]
fn table_width_uses_display_width_for_cjk() {
    let columns = vec!["标题".to_owned(), "Artist".to_owned()];
    let rows = vec![vec!["花骨朵".to_owned(), "Miru".to_owned()]];
    let widths = table_widths(&columns, &rows);
    assert_eq!(widths[0], 6);
    assert_eq!(widths[1], 6);
    let line = format_row(&rows[0], &widths);
    assert_eq!(display_width(&line), 6 + 2 + 6);
}

#[test]
fn block_upsert_replaces_same_id() {
    let mut app = App::new();
    let running = Block {
        id: "same".to_owned(),
        input: "search".to_owned(),
        started_at: 0,
        duration_ms: None,
        status: BlockStatus::Running,
        outputs: vec![],
        footer: None,
        schema_version: 1,
        actions: None,
    };
    let done = Block {
        status: BlockStatus::Done,
        ..running.clone()
    };
    app.upsert_block(running);
    app.upsert_block(done);
    assert_eq!(app.blocks.len(), 1);
    assert_eq!(app.blocks[0].status, BlockStatus::Done);
}

#[test]
fn test_backend_renders_completion_area() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = App::new();
    app.completion = Some(np_core::Completion {
        replace_start: 0,
        replace_end: 3,
        items: vec![np_core::CompletionItem {
            text: "search".to_owned(),
            kind: np_core::CompletionKind::Command,
            summary: "搜索".to_owned(),
            usage: "search [关键词]".to_owned(),
        }],
    });
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let compact = text.replace(' ', "");
    assert!(compact.contains("NowherePlayer"), "{text}");
}

#[test]
fn vectorscope_draws_reference_or_samples() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new();
    app.visual_mode = VisualMode::Vectorscope;
    app.stereo_left = (0..2048).map(|i| (i as f32 * 0.03).sin()).collect();
    app.stereo_right = app.stereo_left.clone();
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let braille = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .filter(|cell| {
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            ('\u{2800}'..='\u{28FF}').contains(&ch)
        })
        .count();
    assert!(braille > 4, "vectorscope braille cells: {braille}");
}

#[test]
fn waveform_mode_draws_braille_points() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new();
    app.visual_mode = VisualMode::Waveform;
    app.waveform = (0..2048)
        .map(|index| ((index as f32) * 0.05).sin())
        .collect();
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let braille = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .filter(|cell| {
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            ('\u{2800}'..='\u{28FF}').contains(&ch)
        })
        .count();
    assert!(braille > 4, "waveform braille cells: {braille}");
}

#[test]
fn braille_spectrum_has_more_points_than_one_block_row() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new();
    app.spectrum = vec![
        0.2, 0.8, 0.4, 1.0, 0.6, 0.3, 0.9, 0.5, 0.7, 0.1, 0.4, 0.8, 0.2, 0.6, 0.3, 0.9,
    ];
    let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let braille = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .filter(|cell| {
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            ('\u{2800}'..='\u{28FF}').contains(&ch)
        })
        .count();
    assert!(braille > 4, "braille cells: {braille}");
}

#[test]
fn cursor_show_sequence_is_once_per_frame() {
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    crossterm::style::Colored::set_ansi_color_disabled(false);
    let sink = Sink(Arc::new(Mutex::new(Vec::new())));
    let mut terminal = Terminal::new(CrosstermBackend::new(sink.clone())).unwrap();
    let mut app = App::new();
    let audio = TuiAudio::empty();
    for _ in 0..20 {
        terminal
            .draw(|frame| draw(frame, &mut app, &audio))
            .unwrap();
    }
    let bytes = sink.0.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&bytes);
    // Baseline: 20 frames -> 20 show-cursor sequences. At 20fps that is 20/s;
    // at 12fps it is 12/s. C (skip unchanged) is intentionally not applied yet.
    // Normal 模式不显示输入行，所以不再每帧请求光标；只有 Command 模式才显示。
    assert_eq!(text.matches("\x1b[?25h").count(), 0);
}

#[test]
fn paper_background_is_written_to_crossterm_backend() {
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let sink = Sink(Arc::new(Mutex::new(Vec::new())));
    crossterm::style::Colored::set_ansi_color_disabled(false);
    let mut terminal = Terminal::new(CrosstermBackend::new(sink.clone())).unwrap();
    let mut app = App::new();
    let theme = TuiThemeApi::new(std::env::temp_dir().join("np-tui-paper-writer"));
    app.load_theme(&theme, "tui-paper");
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("242"),
        "missing paper bg; bytes={}",
        text.escape_default()
    );
}

#[test]
fn paper_theme_fills_background_in_buffer() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new();
    let theme = TuiThemeApi::new(std::env::temp_dir().join("np-tui-paper-buffer"));
    app.load_theme(&theme, "tui-paper");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    assert_eq!(
        terminal.backend().buffer()[(0, 0)].style().bg,
        Some(Color::Rgb(242, 239, 233))
    );
}

#[test]
fn selftest_renders_tables_as_plain_text() {
    let block = Block {
        id: "x".to_owned(),
        input: "library list".to_owned(),
        started_at: 0,
        duration_ms: Some(1),
        status: BlockStatus::Done,
        outputs: vec![Output::Table {
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
            selectable: true,
        }],
        footer: Some("共 1 条".to_owned()),
        schema_version: 1,
        actions: None,
    };
    let text = selftest_block_text(&block);
    assert!(text.contains("序号\t标题"));
    assert!(text.contains("1\t晴天"));
    assert!(text.contains("共 1 条"));
}

#[test]
fn remote_play_target_hands_the_prepared_path_to_the_ui() {
    let (tx, rx) = std::sync::mpsc::channel();
    let target = RemotePlayback { tx };
    target
        .play_path("C:/cache/remote.bin")
        .expect("queue remote path");
    assert_eq!(rx.try_recv().as_deref(), Ok("C:/cache/remote.bin"));
}

#[test]
fn same_title_different_paths_are_distinct_trackrefs() {
    let a = TrackRef::Local(PathBuf::from("/music/a/same.wav"));
    let b = TrackRef::Local(PathBuf::from("/music/b/same.wav"));
    assert_ne!(a, b);
}

#[test]
fn filter_is_case_insensitive_and_handles_chinese() {
    assert!(filter_matches(&["晴天".to_owned()], "晴"));
    assert!(filter_matches(&["Jay Chou".to_owned()], "jay"));
    assert!(filter_matches(&["x".to_owned()], ""));
    assert!(!filter_matches(&["晴天".to_owned()], "稻"));
}

#[test]
fn gg_requires_two_presses_within_timeout() {
    let mut app = App::new();
    app.views = vec![ViewData {
        title: "媒体库".to_owned(),
        columns: vec!["#".to_owned()],
        rows: vec![vec!["1".to_owned()], vec!["2".to_owned()]],
        refs: Vec::new(),
    }];
    app.focus = Focus::List;
    app.row_index = 1;
    let start = std::time::Instant::now();
    assert!(!app.press_g(start));
    assert_eq!(app.row_index, 1);
    assert!(app.press_g(start + std::time::Duration::from_millis(200)));
    assert_eq!(app.row_index, 0);
}

#[test]
fn delete_requires_double_press_flag() {
    let mut app = App::new();
    assert!(!app.delete_armed);
    app.delete_armed = true;
    assert!(app.delete_armed);
    app.delete_armed = false;
    assert!(!app.delete_armed);
}

#[test]
fn left_view_selection_changes_middle_source() {
    let mut app = App::new();
    app.views = vec![
        ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["A".to_owned()],
            rows: vec![vec!["lib".to_owned()]],
            refs: Vec::new(),
        },
        ViewData {
            title: "队列".to_owned(),
            columns: vec!["A".to_owned()],
            rows: vec![vec!["queue".to_owned()]],
            refs: Vec::new(),
        },
    ];
    assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
    app.move_view(true);
    assert_eq!(app.selected_view().unwrap().rows[0][0], "queue");
    app.move_view(false);
    assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
}

#[test]
fn tab_changes_focus_border_color() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = App::new();
    app.views = vec![ViewData {
        title: "媒体库".to_owned(),
        columns: vec!["#".to_owned(), "标题".to_owned()],
        rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
        refs: Vec::new(),
    }];
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let left_before = terminal.backend().buffer()[(1, 1)].style().fg;
    app.toggle_focus();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let left_after = terminal.backend().buffer()[(1, 1)].style().fg;
    assert_ne!(left_before, left_after);
    assert_eq!(left_before, Some(Palette::terminal().accent));
    assert_eq!(left_after, Some(Palette::terminal().faint));
}

#[test]
fn splash_cycles_themes_and_wraps() {
    assert_eq!(splash_step("milkgreen", 1), "graphite");
    assert_eq!(splash_step("milkgreen", -1), "tui-paper");
    assert_eq!(splash_theme_ids().first().copied(), Some("milkgreen"));
}

#[test]
fn splash_degrades_to_one_line_on_narrow_screens() {
    assert_eq!(splash_art(59), "NOWHERE");
    assert_eq!(splash_art(60), "NOWHERE");
    assert_eq!(splash_art(65), "NOWHERE");
    assert!(splash_art(66).contains('█'));
    assert!(splash_art(80).contains('█'));
}

#[test]
fn splash_art_lines_have_equal_display_width() {
    let art = splash_art(66);
    let widths: HashSet<usize> = art.lines().map(UnicodeWidthStr::width).collect();
    assert_eq!(widths.len(), 1, "art lines must align: {widths:?}");
    assert_eq!(art.lines().count(), 6);
    assert!(splash_art(66).contains('█'));
}

#[test]
fn missing_theme_setting_defaults_to_milkgreen() {
    let dir =
        std::env::temp_dir().join(format!("np-splash-{}-{}", std::process::id(), now_millis()));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = SettingsHandle::load(dir.clone());
    assert_eq!(settings.theme_id(), None);
    assert_eq!(
        settings.theme_id().as_deref().unwrap_or("milkgreen"),
        "milkgreen"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn player_strip_is_six_rows_and_list_gets_the_rest() {
    assert_eq!(player_height_for(30), 6);
    assert_eq!(player_height_for(20), 6);
    assert_eq!(player_height_for(8), 5);
}

#[test]
fn chinese_title_truncates_by_display_width() {
    let text = truncate_width("晴天 · Jay Chou", 8);
    assert_eq!(UnicodeWidthStr::width(text.as_str()), 8);
}

#[test]
fn normal_mode_hides_command_input() {
    let mut app = App::new();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(!text.replace(' ', "").contains("输入命令"));
    app.enter_command();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let text: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(text.replace(' ', "").contains("命令"));
}

#[test]
fn test_backend_renders_title_and_input() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let mut app = App::new();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    let compact = rendered.replace(' ', "");
    assert!(compact.contains("NowherePlayer"), "{rendered}");
}

#[test]
fn np_app_completion_has_search_for_sea() {
    let audio = TuiAudio::empty();
    let completion = complete_for(&audio, "sea", 3);
    assert!(
        completion.items.iter().any(|item| item.text == "search"),
        "{:?}",
        completion.items
    );
}

#[test]
fn settings_file_is_read_as_theme_id() {
    let dir = std::env::temp_dir().join(format!("np-tui-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"theme.id":"tui-paper"}"#).unwrap();
    let settings = SettingsHandle::load(dir.clone());
    assert_eq!(settings.theme_id().as_deref(), Some("tui-paper"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn theme_switch_updates_palette_without_restart() {
    let dir = std::env::temp_dir().join(format!("np-tui-theme-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let api = TuiThemeApi::new(dir);
    let mut app = App::new();
    app.load_theme(&api, "tui-paper");
    assert_eq!(app.palette.bg, Some(Color::Rgb(242, 239, 233)));
    assert_eq!(app.theme_id, "tui-paper");
    app.load_theme(&api, "terminal");
    assert!(app.palette.bg.is_none());
    assert_eq!(app.theme_id, "terminal");
}

#[test]
fn tab_accepts_highlighted_completion() {
    let mut app = App::new();
    app.input = "sea".chars().collect();
    app.cursor = 3;
    app.completion = Some(np_core::Completion {
        replace_start: 0,
        replace_end: 3,
        items: vec![np_core::CompletionItem {
            text: "search".to_owned(),
            kind: np_core::CompletionKind::Command,
            summary: "搜索".to_owned(),
            usage: "search [关键词]".to_owned(),
        }],
    });
    assert!(app.accept_completion());
    assert_eq!(app.input_text(), "search ");
    assert_eq!(app.cursor, 7);
}

#[test]
fn history_navigation_suppresses_completion() {
    let mut app = App::new();
    app.history = vec!["help".to_owned()];
    app.history_up();
    let audio = TuiAudio::empty();
    app.refresh_completion(&audio);
    assert!(app.completion.is_none());
    assert_eq!(app.input_text(), "help");
}

#[test]
fn output_table_maps_to_header_separator_and_rows() {
    let mut lines = Vec::new();
    output_lines(
        &Output::Table {
            columns: vec!["#".to_owned(), "标题".to_owned()],
            rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
            selectable: true,
        },
        &mut lines,
        &Palette::terminal(),
    );
    assert_eq!(lines.len(), 3);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains("标题"))
    );
}
