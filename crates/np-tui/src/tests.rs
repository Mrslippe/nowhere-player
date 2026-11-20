use super::*;
use crate::audio::TuiAudio;
use crate::keymap::Action;
use crate::layout::*;
use crate::live_command::parse_live_command;
use crate::render::*;
use crate::theme::Palette;
use crate::transport::TransportAvailability;
use crate::ui::scroll::ScrollState;
use crossterm::event::KeyCode;
use np_app::{PlaybackTarget, SourceId};
use np_core::{Block, BlockStatus, Output};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use std::path::PathBuf;
use std::time::{Duration, Instant};
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
    assert_eq!(display_width(&line), 6 + 1 + 6);
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
    let theme = TuiThemeApi::new(std::env::temp_dir().join("np-tui-milkgreen-writer"));
    app.load_theme(&theme, "milkgreen");
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("238"),
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
    app.load_theme(&theme, "milkgreen");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let audio = TuiAudio::empty();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    assert_eq!(
        terminal.backend().buffer()[(0, 0)].style().bg,
        Some(Color::Rgb(238, 241, 234))
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
fn left_view_selection_changes_middle_source() {
    let mut app = App::new();
    app.views = vec![
        ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["A".to_owned()],
            rows: vec![vec!["lib".to_owned()]],
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "队列".to_owned(),
            columns: vec!["A".to_owned()],
            rows: vec![vec!["queue".to_owned()]],
            refs: Vec::new(),
            durations: Vec::new(),
        },
    ];
    assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
    app.move_view(true);
    assert_eq!(app.selected_view().unwrap().rows[0][0], "queue");
    app.move_view(false);
    assert_eq!(app.selected_view().unwrap().rows[0][0], "lib");
}

#[test]
fn splash_cycles_themes_and_wraps() {
    assert_eq!(splash_step("milkgreen", 1), "milkgreen");
    assert_eq!(splash_step("milkgreen", -1), "milkgreen");
    assert_eq!(splash_theme_ids(), vec!["milkgreen"]);
}

fn splash_text_at(phase: Option<f32>, width: u16, height: u16) -> String {
    let api = crate::theme::TuiThemeApi::new(std::env::temp_dir().join("np-tui-splash-motion"));
    let mut app = App::new();
    app.load_theme(&api, "milkgreen");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("splash terminal");
    match phase {
        Some(phase) => crate::splash::draw_splash_at(&mut terminal, &app, phase).expect("splash"),
        None => crate::splash::draw_splash(&mut terminal, &app).expect("splash"),
    }
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

/// LOGO 行首个 `█` / `╚` 的 (行, 列)；6 行 LOGO 都各有一个。
fn logo_anchors(text: &str) -> Vec<(usize, usize)> {
    text.lines()
        .enumerate()
        .filter_map(|(row, line)| {
            line.chars()
                .position(|character| character == '█' || character == '╚')
                .map(|column| (row, column))
        })
        .collect()
}

#[test]
fn static_splash_is_phase_zero_grid() {
    assert_eq!(
        splash_text_at(None, 80, 24),
        splash_text_at(Some(0.0), 80, 24)
    );
    assert_eq!(
        splash_text_at(None, 60, 20),
        splash_text_at(Some(0.0), 60, 20)
    );
}

#[test]
fn splash_animation_moves_only_the_logo_by_one_cell() {
    let frozen = splash_text_at(Some(0.0), 80, 24);
    // phase=0.9：bob=+1 行，sway=+1 列。
    let moved = splash_text_at(Some(0.9), 80, 24);
    assert_ne!(frozen, moved);
    let frozen_lines: Vec<&str> = frozen.lines().collect();
    let moved_lines: Vec<&str> = moved.lines().collect();
    assert_eq!(frozen_lines.len(), moved_lines.len());
    // 主题名（第 15 行）与 Press Enter（第 17 行）不跟着晃。
    assert_eq!(frozen_lines[14], moved_lines[14]);
    assert_eq!(frozen_lines[16], moved_lines[16]);
    assert_eq!(logo_anchors(&frozen).len(), 6);
    let moved_anchors = logo_anchors(&moved);
    assert_eq!(moved_anchors.len(), 6);
    for (frozen, moved) in logo_anchors(&frozen).iter().zip(&moved_anchors) {
        assert_eq!(moved.0, frozen.0 + 1, "LOGO 只下移 1 行");
        assert_eq!(moved.1, frozen.1 + 1, "LOGO 只右移 1 列");
    }
}

#[test]
fn splash_motion_degrades_in_small_terminals_without_panicking() {
    // 窄（LOGO 退化为 NOWHERE）、矮（11 行动效布局放不下）、刚好 70 列。
    for (width, height) in [(40, 10), (20, 8), (13, 8), (100, 8), (72, 20), (80, 3)] {
        for phase in [0.9_f32, 2.6, 3.6] {
            let text = splash_text_at(Some(phase), width, height);
            assert_eq!(
                text.lines().count(),
                usize::from(height),
                "{width}x{height} phase {phase}"
            );
            if height >= 10 {
                assert!(
                    text.contains("NOWHERE") || text.contains('█'),
                    "{width}x{height} phase {phase} lost the logo"
                );
            }
        }
    }
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
fn layout_heights_for_three_window_sizes() {
    assert_eq!(vertical_heights(28), (12, 16, 0)); // 100x30
    assert_eq!(vertical_heights(38), (17, 21, 0)); // 140x40
    assert_eq!(vertical_heights(22), (9, 13, 0)); // 72/80x24
    let (top, player, controller) = vertical_heights(28);
    assert!(top + player + controller == 28);
    assert!(player >= 10);
    assert!(top.saturating_sub(3) >= 8); // block borders + header
}

#[test]
fn layout_areas_stay_inside_outer_border() {
    for (width, height) in [(100, 30), (140, 40), (80, 24)] {
        let outer = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let [top, player, controller] = layout_areas(outer);
        assert!(top.y > outer.y, "{width}x{height} top {top:?}");
        assert!(
            player.y >= top.bottom(),
            "{width}x{height} player {player:?}"
        );
        assert_eq!(
            controller.height, 0,
            "{width}x{height} controller must be hidden"
        );
        assert_eq!(
            controller.y,
            player.bottom(),
            "{width}x{height} controller {controller:?}"
        );
        assert_eq!(outer.width, width);
    }
}

#[test]
fn chinese_title_truncates_by_display_width() {
    let text = truncate_width("晴天 · Jay Chou", 8);
    assert_eq!(UnicodeWidthStr::width(text.as_str()), 8);
}

#[test]
fn controller_is_hidden_until_colon_focuses_it() {
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
    assert!(!text.replace(' ', "").contains("Controller"));
    app.enter_command();
    assert_eq!(app.focus, Focus::Controller);
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
    assert!(text.replace(' ', "").contains("Controller"));
    app.leave_command();
    assert_eq!(app.focus, Focus::List);
    assert!(app.input.is_empty());
}

#[test]
fn browse_list_counters_use_data() {
    let mut app = App::new();
    app.views = (0..7)
        .map(|index| ViewData {
            title: format!("视图{index}"),
            columns: vec!["#".to_owned(), "标题".to_owned()],
            rows: (0..4)
                .map(|row| vec![row.to_string(), "晴天".to_owned()])
                .collect(),
            refs: Vec::new(),
            durations: Vec::new(),
        })
        .collect();
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
    let compact = text.replace(' ', "");
    assert!(compact.contains("1of7"), "{compact}");
    assert!(compact.contains("total4"), "{compact}");
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
fn theme_name_is_italic() {
    use ratatui::style::Modifier;

    let mut app = App::new();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let symbols: Vec<&str> = (0..72).map(|x| buffer[(x, 0)].symbol()).collect();
    let width = app.theme_name.chars().count();
    let start = (0..72 - width)
        .find(|&x| symbols[x..x + width].concat() == app.theme_name)
        .expect("theme name in top border");
    for offset in 0..width {
        let cell = &buffer[(start as u16 + offset as u16, 0)];
        assert!(
            cell.style().add_modifier.contains(Modifier::ITALIC),
            "theme name cell {cell:?} is not italic"
        );
    }
}

#[test]
fn settings_file_is_read_as_theme_id() {
    let dir = std::env::temp_dir().join(format!("np-tui-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("settings.json"), r#"{"theme.id":"milkgreen"}"#).unwrap();
    let settings = SettingsHandle::load(dir.clone());
    assert_eq!(settings.theme_id().as_deref(), Some("milkgreen"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn theme_switch_updates_palette_without_restart() {
    let dir = std::env::temp_dir().join(format!("np-tui-theme-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let api = TuiThemeApi::new(dir);
    let mut app = App::new();
    app.load_theme(&api, "milkgreen");
    assert_eq!(app.palette.bg, Some(Color::Rgb(238, 241, 234)));
    assert_eq!(app.theme_id, "milkgreen");
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

fn browse_app() -> App {
    let mut app = App::new();
    app.views = vec![
        ViewData {
            title: "歌单".to_owned(),
            columns: vec!["歌单".to_owned(), "条目".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "Like".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "搜索".to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "队列".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        },
        ViewData {
            title: "设置".to_owned(),
            columns: vec!["设置项".to_owned(), "值".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        },
    ];
    app.view_depth = vec![0, 1, 0, 0, 0, 0];
    app.view_kind = vec![
        BrowseKind::PlaylistRoot,
        BrowseKind::Playlist("Like".to_owned()),
        BrowseKind::Library,
        BrowseKind::Search,
        BrowseKind::Queue,
        BrowseKind::Settings,
    ];
    app.playlist_children = vec![app.views[1].clone()];
    app.playlist_expanded = true;
    app.view_index = 0;
    app
}

#[test]
fn browse_tree_starts_at_playlist_and_toggles_children() {
    let mut app = browse_app();
    assert_eq!(
        app.selected_view().map(|view| view.title.as_str()),
        Some("歌单")
    );
    assert_eq!(app.views.len(), 6);
    app.toggle_playlist();
    assert!(!app.playlist_expanded);
    assert_eq!(app.views.len(), 5);
    assert_eq!(app.views[1].title, "媒体库");
    app.toggle_playlist();
    assert!(app.playlist_expanded);
    assert_eq!(app.views[1].title, "Like");
}

#[test]
fn player_content_is_sixty_eight_columns_on_72x24() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let progress_row = (0..24)
        .max_by_key(|y| {
            (0..72)
                .filter(|x| buffer[(*x, *y)].symbol() == glyphs::PROGRESS_DONE)
                .count()
        })
        .expect("progress row");
    let filled = (0..72)
        .filter(|x| buffer[(*x, progress_row)].symbol() == glyphs::PROGRESS_DONE)
        .count();
    assert!(filled >= 20, "progress glyphs: {filled}");
    let gaps = (0..72)
        .filter(|x| buffer[(*x, progress_row)].symbol() == " ")
        .count();
    assert!(gaps >= 20);
    let top = (0..24)
        .find(|y| buffer[(2, *y)].symbol() == "┏" && buffer[(4, *y)].symbol() == "P")
        .expect("player title");
    assert_eq!(buffer[(2, top)].symbol(), "┏");
    assert_eq!(buffer[(3, top)].symbol(), " ");
}

#[test]
fn status_bar_is_embedded_in_bottom_border() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text: String = (0..72).map(|x| buffer[(x, 23)].symbol()).collect();
    assert!(text.starts_with('┗'), "{text:?}");
    let compact = text.replace(' ', "");
    assert!(compact.contains("新建"), "{text:?}");
    assert!(!text.contains("就绪"), "{text:?}");
}

#[test]
fn terminal_palette_selection_stays_visible_with_reversed() {
    use ratatui::style::Modifier;

    // App::new() 用终端回退 Palette：fg / bg 都是 Reset，旧的反白条不可见。
    let mut app = browse_app();
    assert!(app.palette.terminal, "默认必须是终端回退主题");
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let selected = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == crate::glyphs::SELECT)
        .expect("Browse 选中行标记");
    assert!(
        selected.style().add_modifier.contains(Modifier::REVERSED),
        "终端回退的选中态必须用 REVERSED"
    );
}

#[test]
fn focus_and_status_styles_follow_the_focused_pane() {
    use ratatui::style::Modifier;

    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    let accent = Some(Palette::terminal().accent);
    let line = Some(Palette::terminal().line);

    app.focus = Focus::List;
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let corners_accent = buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == "┓" && cell.style().fg == accent)
        .count();
    assert!(
        corners_accent >= 1,
        "focused List corner missing: {corners_accent}"
    );

    app.focus = Focus::Controller;
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let corners_line = buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == "┏" && cell.style().fg == line)
        .count();
    let corners_accent = buffer
        .content()
        .iter()
        .filter(|cell| cell.symbol() == "┏" && cell.style().fg == accent)
        .count();
    assert!(corners_line >= 1, "unfocused Browse corner missing");
    assert!(corners_accent >= 1, "focused Controller corner missing");

    let counts = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "1" && cell.style().add_modifier.contains(Modifier::ITALIC));
    assert!(counts.is_some(), "1 of N must be italic");
    let status = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "←")
        .expect("status key");
    assert_eq!(status.style().fg, accent);
    assert!(status.style().add_modifier.contains(Modifier::BOLD));
}

#[test]
fn load_views_keeps_search_node_and_playlist_first() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-load-views-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    let cache = root.join("cache");
    let data = root.join("data");
    std::fs::create_dir_all(&config).unwrap();
    let library = np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: cache,
        data_dir: data,
    });
    let settings = np_app::SettingsHandle::load(config);
    let audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &library, &settings);

    let titles: Vec<&str> = app.views.iter().map(|view| view.title.as_str()).collect();
    assert_eq!(titles.first().copied(), Some("歌单"));
    assert!(titles.contains(&"媒体库"));
    assert!(titles.contains(&"搜索"));
    assert!(titles.contains(&"队列"));
    assert!(titles.contains(&"设置"));
    assert_eq!(app.view_index, 0);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn more_than_five_playlists_start_collapsed() {
    use np_app::LibraryApi;

    let root = std::env::temp_dir().join(format!(
        "np-tui-playlists-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    let cache = root.join("cache");
    let data = root.join("data");
    std::fs::create_dir_all(&config).unwrap();
    let library = np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: cache,
        data_dir: data,
    });
    for index in 0..6 {
        library.playlist_create(&format!("P{index}")).unwrap();
    }
    let settings = np_app::SettingsHandle::load(config);
    let audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &library, &settings);

    assert!(!app.playlist_expanded);
    assert!(!app.views.iter().any(|view| view.title.starts_with('P')));
    app.toggle_playlist();
    assert!(app.playlist_expanded);
    assert!(app.views.iter().any(|view| view.title == "P0"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn controller_completion_still_accepts_a_command() {
    let audio = TuiAudio::empty();
    let mut app = App::new();
    app.input = "sea".chars().collect();
    app.cursor = 3;
    app.last_edit = std::time::Instant::now() - std::time::Duration::from_millis(100);
    assert!(app.refresh_completion(&audio));
    assert!(app.completion.is_some());
    assert!(app.accept_completion());
    assert!(app.input_text().starts_with("search"));
}

#[test]
fn long_title_does_not_displace_artist_or_duration() {
    let columns = vec![
        "序号".to_owned(),
        "标题".to_owned(),
        "艺人".to_owned(),
        "时长".to_owned(),
    ];
    let rows = vec![vec![
        "1".to_owned(),
        "一个非常非常非常非常非常非常非常长的歌曲标题".to_owned(),
        "艺人".to_owned(),
        "03:00".to_owned(),
    ]];
    let widths = fit_table_widths(&columns, &rows, 46);
    assert!(widths.iter().sum::<usize>() + 2 * (widths.len() - 1) <= 46);
    assert!(widths[0] <= 4);
    assert_eq!(widths[3], 5);
    let line = format_row(&rows[0], &widths);
    assert!(display_width(&line) <= 46);
}

#[test]
fn list_uses_designed_default_columns_and_stretches_title_artist() {
    let columns = vec![
        "#".to_owned(),
        "Title".to_owned(),
        "Artist".to_owned(),
        "Duration".to_owned(),
    ];
    let rows = vec![vec![
        "1".to_owned(),
        "Song".to_owned(),
        "Artist".to_owned(),
        "03:00".to_owned(),
    ]];
    assert_eq!(list_widths(&columns, &rows, 48), vec![4, 24, 9, 8]);
    assert_eq!(list_widths(&columns, &rows, 49), vec![4, 24, 10, 8]);
    let wider = list_widths(&columns, &rows, 59);
    assert_eq!(wider[0], 4);
    assert_eq!(wider[3], 8);
    assert!(wider[1] > 24);
    assert!(wider[2] > 10);
    assert_eq!(wider.iter().sum::<usize>() + 3, 59);
}

#[test]
fn player_volume_bar_has_twenty_cells() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let row = (0..24)
        .find(|y| buffer[(2, *y)].symbol() == "┏" && buffer[(4, *y)].symbol() == "P")
        .expect("player row");
    let count = (0..72)
        .filter(|x| buffer[(*x, row)].symbol() == glyphs::PROGRESS_DONE)
        .count();
    assert_eq!(count, 20);
}

#[test]
fn list_hides_source_column_even_when_view_has_it() {
    let mut app = browse_app();
    app.views[0].columns = vec![
        "序号".to_owned(),
        "标题".to_owned(),
        "艺人".to_owned(),
        "时长".to_owned(),
        "来源".to_owned(),
    ];
    app.views[0].rows = vec![vec![
        "1".to_owned(),
        "歌".to_owned(),
        "人".to_owned(),
        "03:00".to_owned(),
        "B 站".to_owned(),
    ]];
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
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
    assert!(!text.contains("来 源"));
    assert!(!text.contains("B 站"));
}

#[test]
fn volume_bar_is_contiguous_and_has_one_corner_gap() {
    use np_app::PlayerApi;
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    PlayerApi::volume(&audio, 0.55).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let row = (0..24)
        .find(|y| buffer[(2, *y)].symbol() == "┏" && buffer[(4, *y)].symbol() == "P")
        .expect("player row");
    let positions: Vec<u16> = (0..72)
        .filter(|x| buffer[(*x, row)].symbol() == glyphs::PROGRESS_DONE)
        .collect();
    assert_eq!(positions.len(), 20);
    assert!(positions.windows(2).all(|pair| pair[1] == pair[0] + 1));
    let last = *positions.last().unwrap();
    assert_eq!(buffer[(last + 1, row)].symbol(), " ");
}

#[test]
fn baseline_72x24_horizontal_coordinates() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 1)].symbol(), "┃");
    assert_eq!(buffer[(1, 1)].symbol(), " ");
    assert_eq!(buffer[(2, 1)].symbol(), "┏");
    assert_eq!(buffer[(18, 1)].symbol(), "┳");
    assert_eq!(buffer[(69, 1)].symbol(), "┓");
    assert_eq!(buffer[(70, 1)].symbol(), " ");
    assert_eq!(buffer[(71, 1)].symbol(), "┃");
    assert_eq!(buffer[(2, 10)].symbol(), "┏");
    assert_eq!(buffer[(69, 10)].symbol(), "┓");
    assert_eq!(buffer[(2, 11)].symbol(), " ");
    assert_eq!(buffer[(69, 11)].symbol(), " ");
    assert_eq!(buffer[(2, 22)].symbol(), "┗");
    assert_eq!(buffer[(69, 22)].symbol(), "┛");
}

#[test]
fn playlist_root_uses_list_title_and_four_columns() {
    let mut app = browse_app();
    app.views[0].columns = vec![
        "序号".to_owned(),
        "名称".to_owned(),
        "创建时间".to_owned(),
        "数量".to_owned(),
    ];
    app.views[0].rows = vec![vec![
        "1".to_owned(),
        "Like".to_owned(),
        "2026.11.15".to_owned(),
        "25".to_owned(),
    ]];
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
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
    assert!(text.contains("PlayList"));
    assert!(text.contains("创 建 时 间"));
    assert!(text.contains("Like"));
}

#[test]
fn search_renders_empty_searching_failed_and_result_states() {
    let mut app = browse_app();
    app.view_index = 3;
    app.focus = Focus::List;
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    assert!(draw_text(&mut app, &mut terminal, &audio).contains("Key Words"));
    app.search_pending = true;
    assert!(draw_text(&mut app, &mut terminal, &audio).contains("searching..."));
    app.search_pending = false;
    app.search_attempted = true;
    app.views[3].rows.clear();
    assert!(draw_text(&mut app, &mut terminal, &audio).contains("no results"));
    app.views[3].columns = vec![
        "序号".to_owned(),
        "标题".to_owned(),
        "艺人".to_owned(),
        "平台".to_owned(),
    ];
    app.views[3].rows = vec![vec![
        "1".to_owned(),
        "Luv".to_owned(),
        "Nujabes".to_owned(),
        "bilibili".to_owned(),
    ]];
    let shown = draw_text(&mut app, &mut terminal, &audio);
    assert!(shown.contains("平 台"));
    assert!(shown.contains("bilibili"));
}

#[test]
fn playlist_root_uses_four_column_width_preset() {
    let columns = vec![
        "序号".to_owned(),
        "名称".to_owned(),
        "创建时间".to_owned(),
        "数量".to_owned(),
    ];
    let rows = vec![vec![
        "1".to_owned(),
        "Like".to_owned(),
        "2026.11.15".to_owned(),
        "25".to_owned(),
    ]];
    assert_eq!(list_widths(&columns, &rows, 48), vec![4, 24, 12, 5]);
}

#[test]
fn search_input_builds_keyword_artist_and_source_draft() {
    let draft = parse_search_input("Clarity -artist Mit3uri -source bilibili").unwrap();
    assert_eq!(draft.keyword, "Clarity");
    assert_eq!(draft.artist.as_deref(), Some("Mit3uri"));
    assert_eq!(draft.source, Some(SourceId::Bilibili));
    let draft = parse_search_input(r#""a b" --source netease"#).unwrap();
    assert_eq!(draft.keyword, "a b");
    assert_eq!(draft.source, Some(SourceId::Netease));
    assert!(parse_search_input(" -artist x").is_err());
    assert!(parse_search_input("x -unknown y").is_err());
}

#[test]
fn search_input_handles_cjk_without_byte_boundary_panic() {
    let mut app = App::new();
    app.insert_search_char('花');
    app.insert_search_char('骨');
    app.insert_search_char('朵');
    assert_eq!(app.search_input, "花骨朵");
    app.move_search_cursor(false);
    app.insert_search_char('X');
    assert_eq!(app.search_input, "花骨X朵");
    app.backspace_search_char();
    assert_eq!(app.search_input, "花骨朵");
    app.move_search_cursor(false);
    app.backspace_search_char();
    assert_eq!(app.search_input, "骨朵");
}

#[test]
fn search_input_requires_enter_and_esc_releases_editing() {
    let mut app = browse_app();
    app.view_index = 3;
    app.focus = Focus::List;
    assert!(!app.search_editing_input());
    app.begin_search_edit();
    assert!(app.search_editing_input());
    app.insert_search_char('花');
    assert_eq!(app.search_input, "花");
    app.end_search_edit();
    assert!(!app.search_editing_input());
    app.search_attempted = true;
    app.views[3].rows = vec![vec!["1".to_owned()]];
    app.reset_search_to_empty();
    assert!(!app.search_attempted);
    assert!(app.views[3].rows.is_empty());
    assert!(!app.search_editing_input());
}

fn draw_text(app: &mut App, terminal: &mut Terminal<TestBackend>, audio: &TuiAudio) -> String {
    terminal.draw(|frame| draw(frame, app, audio)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn row_text(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let area = terminal.backend().buffer().area;
    (0..area.width)
        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
        .collect()
}

fn row_containing(terminal: &Terminal<TestBackend>, needle: &str) -> u16 {
    let area = terminal.backend().buffer().area;
    (area.y..area.y + area.height)
        .find(|y| row_text(terminal, *y).contains(needle))
        .unwrap_or_else(|| panic!("row containing {needle:?} not found"))
}

#[test]
fn add_hotkey_starts_pending_track_and_jumps_to_playlist_root() {
    let mut app = browse_app();
    app.view_index = 2;
    app.focus = Focus::List;
    app.views[2].refs = vec![TrackRef::Local(PathBuf::from("/tmp/song.flac"))];
    app.views[2].rows = vec![vec![
        "1".to_owned(),
        "Song".to_owned(),
        "Artist".to_owned(),
        "03:00".to_owned(),
    ]];
    app.begin_add_to_playlist().unwrap();
    assert!(app.pending_add.is_some());
    assert_eq!(app.view_index, 0);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn add_hotkey_cancel_restores_origin_page() {
    let mut app = browse_app();
    app.view_index = 2;
    app.row_index = 0;
    app.focus = Focus::List;
    app.views[2].refs = vec![TrackRef::Local(PathBuf::from("/tmp/song.flac"))];
    app.views[2].rows = vec![vec![
        "1".to_owned(),
        "Song".to_owned(),
        "Artist".to_owned(),
        "03:00".to_owned(),
    ]];
    app.begin_add_to_playlist().unwrap();
    app.cancel_pending_add();
    assert!(app.pending_add.is_none());
    assert_eq!(app.view_index, 2);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn remove_hotkey_arms_and_cancels_without_touching_library() {
    let mut app = browse_app();
    app.view_index = 2;
    app.focus = Focus::List;
    app.views[2].refs = vec![TrackRef::Local(PathBuf::from("/tmp/song.flac"))];
    app.views[2].rows = vec![vec!["1".to_owned(), "Song".to_owned()]];
    app.arm_remove().unwrap();
    assert!(app.pending_remove.is_some());
    app.cancel_pending_remove();
    assert!(app.pending_remove.is_none());
    assert_eq!(app.view_index, 2);
}

#[test]
fn add_from_search_uses_duration_metadata_not_platform_column() {
    let mut app = browse_app();
    app.view_index = 3;
    app.focus = Focus::List;
    app.views[3].columns = vec![
        "序号".to_owned(),
        "标题".to_owned(),
        "艺人".to_owned(),
        "平台".to_owned(),
    ];
    app.views[3].rows = vec![vec![
        "1".to_owned(),
        "Song".to_owned(),
        "Artist".to_owned(),
        "bilibili".to_owned(),
    ]];
    app.views[3].refs = vec![TrackRef::Remote {
        source: "bilibili".to_owned(),
        id: "BV1".to_owned(),
    }];
    app.views[3].durations = vec![171_000];
    app.begin_add_to_playlist().unwrap();
    assert_eq!(
        app.pending_add
            .as_ref()
            .map(|pending| pending.item.duration_ms),
        Some(171_000)
    );
}

#[test]
fn playlist_create_and_delete_use_enter_confirmation_model() {
    use np_app::LibraryApi;

    let root = std::env::temp_dir().join(format!(
        "np-tui-playlist-crud-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    let cache = root.join("cache");
    let data = root.join("data");
    std::fs::create_dir_all(&config).unwrap();
    let library = np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: cache,
        data_dir: data,
    });
    let settings = np_app::SettingsHandle::load(config);
    let audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &library, &settings);
    app.focus = Focus::List;

    app.begin_playlist_create().unwrap();
    for character in "New".chars() {
        app.insert_playlist_char(character);
    }
    app.confirm_playlist_create(&library).unwrap();
    assert_eq!(
        library
            .playlist_list()
            .unwrap()
            .into_iter()
            .map(|playlist| playlist.name)
            .collect::<Vec<_>>(),
        vec!["New".to_owned()]
    );
    assert!(
        app.view_kind
            .iter()
            .any(|kind| matches!(kind, BrowseKind::Playlist(name) if name == "New"))
    );

    app.view_index = 0;
    app.row_index = 0;
    app.arm_playlist_delete().unwrap();
    assert_eq!(app.pending_playlist_delete.as_deref(), Some("New"));
    app.confirm_playlist_delete(&library).unwrap();
    assert!(library.playlist_list().unwrap().is_empty());
    assert!(
        !app.view_kind
            .iter()
            .any(|kind| matches!(kind, BrowseKind::Playlist(name) if name == "New"))
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn status_bar_prioritizes_page_specific_actions() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    let compact = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };

    app.view_index = 0;
    app.focus = Focus::Browse;
    let root = compact(&draw_text(&mut app, &mut terminal, &audio));
    assert!(root.contains("新建"));
    assert!(root.contains("删除"));

    app.view_index = 1;
    app.focus = Focus::List;
    let detail = compact(&draw_text(&mut app, &mut terminal, &audio));
    assert!(detail.contains("添加"));
    assert!(detail.contains("移除"));

    app.view_index = 4;
    terminal.clear().unwrap();
    let queue = compact(&draw_text(&mut app, &mut terminal, &audio));
    assert!(queue.contains("移除"));
    assert!(queue.contains("播放"));
}

#[test]
fn playlist_create_prompt_draws_centered_input() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    app.view_index = 0;
    app.begin_playlist_create().unwrap();
    app.insert_playlist_char('花');
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    let text = draw_text(&mut app, &mut terminal, &audio);
    assert!(text.contains("New Playlist"), "{text}");
    assert!(text.contains('花'), "{text}");
}

#[test]
fn playlist_create_empty_input_has_no_placeholder_under_cursor() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    app.view_index = 0;
    app.begin_playlist_create().unwrap();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    let _ = draw_text(&mut app, &mut terminal, &audio);

    let title_y = row_containing(&terminal, "New Playlist");
    let input_y = title_y + 1;
    let row = row_text(&terminal, input_y);
    assert!(!row.contains("输入歌单名"), "{row}");

    let cursor = terminal.backend().cursor_position();
    assert_eq!(cursor.y, input_y);
    let cell = row.chars().nth(cursor.x as usize).unwrap_or(' ');
    assert_eq!(
        cell, ' ',
        "cursor sits on {cell:?} at x={}: {row}",
        cursor.x
    );
}

#[test]
fn playlist_create_cjk_cursor_scrolls_inside_input() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    app.view_index = 0;
    app.begin_playlist_create().unwrap();
    for _ in 0..20 {
        app.insert_playlist_char('歌');
    }
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    let _ = draw_text(&mut app, &mut terminal, &audio);

    let title_y = row_containing(&terminal, "New Playlist");
    let input_y = title_y + 1;
    let row = row_text(&terminal, input_y);
    let box_left = row
        .chars()
        .position(|character| character == '│')
        .expect("input box border");
    let cursor = terminal.backend().cursor_position();
    assert_eq!(cursor.y, input_y);
    assert_eq!(cursor.x, box_left as u16 + 34, "{row}");
    assert!(cursor.x < box_left as u16 + 38, "{row}");
    assert!(row.contains('歌'), "{row}");
}

#[test]
fn pending_actions_wait_for_enter_not_same_key() {
    let mut app = browse_app();
    app.view_index = 4;
    app.focus = Focus::List;
    app.views[4].refs = vec![TrackRef::Local(PathBuf::from("/tmp/song.flac"))];
    app.views[4].rows = vec![vec!["1".to_owned(), "Song".to_owned()]];
    app.arm_remove().unwrap();
    assert!(app.pending_remove.is_some());
    app.cancel_pending_remove();
    assert!(app.pending_remove.is_none());
}

#[test]
fn input_window_scrolls_by_display_width_for_cjk() {
    let chars = vec!['中', '中', '中'];
    let (start, end) = input_window(&chars, 3, 4);
    let prefix: String = chars.iter().take(3).skip(start).collect();
    assert!(start > 0);
    assert!(end <= chars.len());
    assert!(display_width(&prefix) < 4);
    let (start, end) = input_window(&chars, 2, 4);
    assert!(start <= 2);
    assert!(end >= 3);
}

#[test]
fn settings_page_lists_editable_fields_by_category() {
    let dir = std::env::temp_dir().join(format!(
        "np-tui-settings-view-{}-{}",
        std::process::id(),
        now_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let settings = SettingsHandle::load(dir.clone());
    let page = crate::settings_page::settings_page(&settings);
    assert_eq!(page.view.columns, vec!["分类", "设置项", "值"]);
    let labels: Vec<&str> = page
        .view
        .rows
        .iter()
        .filter_map(|row| row.get(1).map(String::as_str))
        .collect();
    for label in [
        "音量",
        "播放模式",
        "主题",
        "启动页",
        "专注模式",
        "开机自启",
        "B站",
        "网易云",
        "直播缓冲",
    ] {
        assert!(labels.contains(&label), "missing {label}");
    }
    assert_eq!(page.fields.len(), page.view.rows.len());
    assert!(
        page.fields
            .iter()
            .all(|field| field.kind != crate::settings_page::SettingsKind::ReadOnly)
    );
    let mode_row = page
        .view
        .rows
        .iter()
        .find(|row| row.get(1).is_some_and(|label| label == "播放模式"))
        .expect("mode row");
    assert_eq!(mode_row[2], "⇉");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn playback_mode_setting_round_trips() {
    let audio = TuiAudio::empty();
    assert!(audio.set_mode_id("single"));
    assert_eq!(audio.mode_id(), "single");
    assert!(audio.set_mode_id("shuffle"));
    assert_eq!(audio.mode_id(), "shuffle");
    assert!(audio.set_mode_id("list"));
    assert_eq!(audio.mode_id(), "list");
    assert!(audio.set_mode_id("sequential"));
    assert_eq!(audio.mode_id(), "sequential");
}

#[test]
fn settings_editing_updates_audio_and_saves() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-settings-edit-{}-{}",
        std::process::id(),
        now_millis()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let settings = SettingsHandle::load(root.clone());
    let page = crate::settings_page::settings_page(&settings);
    let audio = TuiAudio::empty();
    audio.set_volume(0.5);
    let theme = TuiThemeApi::new(root.join("themes"));
    let mut app = App::new();
    app.views = vec![page.view];
    app.view_kind = vec![BrowseKind::Settings];
    app.view_depth = vec![0];
    app.settings_fields = page.fields;
    app.focus = Focus::List;

    app.row_index = 0;
    crate::settings_editor::adjust_setting(&mut app, -1, &audio, &settings, &theme).unwrap();
    assert!((audio.volume() - 0.49).abs() < 0.001);
    assert!(matches!(
        settings.get("playback.volume"),
        Some(np_core::SettingValue::Num(value)) if (value - 0.49).abs() < 0.001
    ));

    app.row_index = 1;
    crate::settings_editor::adjust_setting(&mut app, 1, &audio, &settings, &theme).unwrap();
    assert_eq!(audio.mode_id(), "list");
    assert_eq!(settings.get_str("playback.mode").as_deref(), Some("list"));
    assert_eq!(app.status, "播放模式 ↻");

    app.row_index = 3;
    crate::settings_editor::adjust_setting(&mut app, 1, &audio, &settings, &theme).unwrap();
    assert_eq!(settings.get_bool("ui.splash"), Some(false));
    assert_eq!(app.status, "启动页 关");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_buffer_setting_is_editable_and_reaches_the_player() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-live-buffer-{}-{}",
        std::process::id(),
        now_millis()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let settings = SettingsHandle::load(root.clone());
    let page = crate::settings_page::settings_page(&settings);
    let audio = TuiAudio::empty();
    assert_eq!(audio.live_buffer_target_ms(), 900);
    let theme = TuiThemeApi::new(root.join("themes"));
    let mut app = App::new();
    app.settings_fields = page.fields.clone();
    app.focus = Focus::List;
    app.row_index = page
        .fields
        .iter()
        .position(|field| field.key == "live.buffer_ms")
        .expect("live.buffer_ms field");

    crate::settings_editor::adjust_setting(&mut app, 1, &audio, &settings, &theme).unwrap();
    assert!(matches!(
        settings.get("live.buffer_ms"),
        Some(np_core::SettingValue::Num(value)) if (value - 1000.0).abs() < 0.001
    ));
    assert_eq!(audio.live_buffer_target_ms(), 1000);
    assert_eq!(app.status, "直播缓冲 1000ms");

    let page = crate::settings_page::settings_page(&settings);
    let row = page
        .view
        .rows
        .iter()
        .find(|row| row.get(1).map(String::as_str) == Some("直播缓冲"))
        .expect("live buffer row");
    assert_eq!(row[2], "1000 ms");

    // Live 页控制条直接显示 target/actual。
    audio.live_for_test(21622811, "BLAST");
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
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
    assert!(text.contains("0/1000ms"), "target/actual missing: {text}");
    let _ = std::fs::remove_dir_all(root);
}

fn settings_fixture() -> (
    App,
    np_app::SettingsHandle,
    TuiAudio,
    crate::theme::TuiThemeApi,
    PathBuf,
) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!(
        "np-tui-settings-edit-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let settings = np_app::SettingsHandle::load(root.clone());
    let page = crate::settings_page::settings_page(&settings);
    let audio = TuiAudio::empty();
    audio.set_volume(0.5);
    let theme = crate::theme::TuiThemeApi::new(root.join("themes"));
    let mut app = App::new();
    app.load_theme(&theme, "milkgreen");
    app.views = vec![page.view];
    app.view_kind = vec![BrowseKind::Settings];
    app.view_depth = vec![0];
    app.settings_fields = page.fields;
    app.focus = Focus::List;
    (app, settings, audio, theme, root)
}

#[test]
fn settings_edit_waits_for_enter_and_esc_restores() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = 0;

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert!(app.settings_editor.is_editing());
    assert_eq!(app.views[0].rows[0][2], "‹ 50% ›");
    assert_eq!(audio.volume(), 0.5);
    assert!(settings.get("playback.volume").is_none());

    crate::settings_editor::adjust_settings_edit(&mut app, 1, &settings, &theme).unwrap();
    assert_eq!(app.views[0].rows[0][2], "‹ 51% ›");
    assert_eq!(audio.volume(), 0.5, "编辑态不能直接改播放器");
    assert!(settings.get("playback.volume").is_none());

    assert!(crate::settings_editor::cancel_settings_edit(
        &mut app, &settings
    ));
    assert!(!app.settings_editor.is_editing());
    assert_eq!(app.views[0].rows[0][2], "100%");
    assert_eq!(audio.volume(), 0.5);

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    crate::settings_editor::adjust_settings_edit(&mut app, 1, &settings, &theme).unwrap();
    crate::settings_editor::commit_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert!(!app.settings_editor.is_editing());
    assert!((audio.volume() - 0.51).abs() < 0.001);
    assert!(matches!(
        settings.get("playback.volume"),
        Some(np_core::SettingValue::Num(value)) if (value - 0.51).abs() < 0.001
    ));
    assert_eq!(app.views[0].rows[0][2], "51%");
    assert!(app.status_expires_at.is_some(), "保存成功应是瞬时状态");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_edit_blocks_global_focus_keys() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = 0;
    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();

    assert!(crate::settings_editor::handle_settings_edit_key(
        KeyCode::Left,
        &mut app,
        &audio,
        &theme,
        &settings
    ));
    assert_eq!(app.focus, Focus::List, "编辑态 ← 不得切焦点");
    assert!(app.settings_editor.is_editing());

    assert!(crate::settings_editor::handle_settings_edit_key(
        KeyCode::Right,
        &mut app,
        &audio,
        &theme,
        &settings
    ));
    assert_eq!(app.focus, Focus::List, "编辑态 → 不得切焦点");

    assert!(crate::settings_editor::handle_settings_edit_key(
        KeyCode::Esc,
        &mut app,
        &audio,
        &theme,
        &settings
    ));
    assert!(!app.settings_editor.is_editing());
    assert_eq!(app.focus, Focus::List);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_bool_enter_toggles_and_commits_without_edit_mode() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = app
        .settings_fields
        .iter()
        .position(|field| field.key == "ui.splash")
        .expect("ui.splash");
    assert_eq!(settings.get_bool("ui.splash"), None);

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert!(!app.settings_editor.is_editing());
    assert_eq!(settings.get_bool("ui.splash"), Some(false));
    assert!(app.status_expires_at.is_some());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_motion_and_sound_are_appended_and_default_on() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-settings-t0124-{}-{}",
        std::process::id(),
        now_millis()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let settings = SettingsHandle::load(root.clone());
    let page = crate::settings_page::settings_page(&settings);
    let keys: Vec<&str> = page.fields.iter().map(|field| field.key.as_str()).collect();
    assert_eq!(
        &keys[keys.len() - 2..],
        &["ui.motion", "ui.sound"],
        "新设置必须追加在末尾，避免既有行号偏移"
    );
    for field in &page.fields[keys.len() - 2..] {
        assert_eq!(field.kind, crate::settings_page::SettingsKind::Bool);
        assert_eq!(field.default, Some(np_core::SettingValue::Bool(true)));
    }
    assert_eq!(
        crate::settings_page::default_value("ui.motion"),
        np_core::SettingValue::Bool(true)
    );
    assert_eq!(
        crate::settings_page::default_value("ui.sound"),
        np_core::SettingValue::Bool(true)
    );
    let motion_row = page
        .view
        .rows
        .iter()
        .find(|row| row.get(1).is_some_and(|label| label == "动效"))
        .expect("动效 row");
    assert_eq!(motion_row[2], "开");
    let sound_row = page
        .view
        .rows
        .iter()
        .find(|row| row.get(1).is_some_and(|label| label == "音效"))
        .expect("音效 row");
    assert_eq!(sound_row[2], "开");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn motion_setting_reports_next_launch() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = app
        .settings_fields
        .iter()
        .position(|field| field.key == "ui.motion")
        .expect("ui.motion");
    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert_eq!(settings.get_bool("ui.motion"), Some(false));
    assert_eq!(app.status, "动效 关 · 下次启动生效");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sound_setting_toggle_applies_immediately() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = app
        .settings_fields
        .iter()
        .position(|field| field.key == "ui.sound")
        .expect("ui.sound");
    // 未写过的设置默认开；第一次 Enter 关掉并立即静音。
    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert_eq!(settings.get_bool("ui.sound"), Some(false));
    assert_eq!(app.status, "音效 关");
    assert!(!app.sound.should_play_at(Instant::now()));
    // 再切回开：立即恢复。
    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert_eq!(settings.get_bool("ui.sound"), Some(true));
    assert!(app.sound.should_play_at(Instant::now()));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_read_only_enter_never_enters_edit() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.settings_fields
        .push(crate::settings_page::SettingsField {
            key: "custom.readonly".to_owned(),
            label: String::new(),
            kind: crate::settings_page::SettingsKind::ReadOnly,
            default: None,
        });
    app.views[0].rows.push(vec![
        "其他".to_owned(),
        "custom.readonly".to_owned(),
        "值".to_owned(),
    ]);
    app.row_index = app.settings_fields.len() - 1;

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert!(!app.settings_editor.is_editing());
    assert!(app.status.contains("只读"), "{}", app.status);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_buffer_uses_100ms_steps_and_commits_on_enter() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = app
        .settings_fields
        .iter()
        .position(|field| field.key == np_core::settings::LIVE_BUFFER_MS_KEY)
        .expect("live buffer");
    assert_eq!(audio.live_buffer_target_ms(), 900);

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    crate::settings_editor::adjust_settings_edit(&mut app, 1, &settings, &theme).unwrap();
    assert_eq!(app.views[0].rows[app.row_index][2], "‹ 1000 ms ›");
    assert_eq!(audio.live_buffer_target_ms(), 900, "提交前不改播放器");

    crate::settings_editor::commit_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert_eq!(audio.live_buffer_target_ms(), 1000);
    assert!(matches!(
        settings.get(np_core::settings::LIVE_BUFFER_MS_KEY),
        Some(np_core::SettingValue::Num(value)) if (value - 1000.0).abs() < 0.001
    ));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn settings_playback_mode_edits_as_an_enum() {
    let (mut app, settings, audio, theme, root) = settings_fixture();
    app.row_index = app
        .settings_fields
        .iter()
        .position(|field| field.key == "playback.mode")
        .expect("playback.mode");
    assert_eq!(audio.mode_id(), "sequential");

    crate::settings_editor::begin_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    crate::settings_editor::adjust_settings_edit(&mut app, 1, &settings, &theme).unwrap();
    assert_eq!(app.views[0].rows[app.row_index][2], "‹ ↻ ›");
    assert_eq!(audio.mode_id(), "sequential");

    crate::settings_editor::commit_settings_edit(&mut app, &audio, &settings, &theme).unwrap();
    assert_eq!(audio.mode_id(), "list");
    assert_eq!(settings.get_str("playback.mode").as_deref(), Some("list"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn transient_status_expires_after_three_seconds() {
    let mut app = App::new();
    let now = Instant::now();
    app.set_transient_status_at("已创建歌单：X", now);
    assert!(!app.expire_status(now + Duration::from_secs(2), "就绪"));
    assert_eq!(app.status, "已创建歌单：X");
    assert!(app.expire_status(now + Duration::from_secs(3), "就绪"));
    assert_eq!(app.status, "就绪");
}

#[test]
fn persistent_status_is_never_auto_cleared() {
    let mut app = App::new();
    app.set_status("搜索中…");
    let now = Instant::now();
    assert!(!app.expire_status(now + Duration::from_secs(30), "就绪"));
    assert_eq!(app.status, "搜索中…");
}

#[test]
fn live_status_default_is_chinese_after_expiry() {
    let mut app = App::new();
    let now = Instant::now();
    app.set_transient_status_at("弹幕已发送", now);
    assert!(app.expire_status(now + Duration::from_secs(3), "直播"));
    assert_eq!(app.status, "直播");
    app.set_transient_status_at("弹幕已发送", now);
    assert!(app.expire_status(now + Duration::from_secs(3), "暂停"));
    assert_eq!(app.status, "暂停");
}

#[test]
fn chat_notice_expires_after_three_seconds() {
    let mut app = App::new();
    let now = Instant::now();
    app.show_chat_notice_at("弹幕已发送", now);
    assert!(!app.expire_chat_notice(now + Duration::from_secs(2)));
    assert_eq!(app.chat_notice.as_deref(), Some("弹幕已发送"));
    assert!(app.expire_chat_notice(now + Duration::from_secs(3)));
    assert!(app.chat_notice.is_none());
}

fn long_tree_app() -> App {
    let mut app = App::new();
    let api = crate::theme::TuiThemeApi::new(std::env::temp_dir().join("np-tui-scroll-theme"));
    app.load_theme(&api, "milkgreen");
    app.views = (0..9)
        .map(|index| ViewData {
            title: format!("页面{index}"),
            columns: vec!["序号".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
            durations: Vec::new(),
        })
        .collect();
    app.view_depth = vec![0; 9];
    app.view_kind = (0..9).map(|_| BrowseKind::Library).collect();
    app.focus = Focus::Browse;
    app.view_index = 8;
    app
}

#[test]
fn browse_tree_scrolls_the_ninth_view_into_seven_rows() {
    let mut scroll = ScrollState::new(7);
    scroll.follow(8, 9);
    assert_eq!(scroll.first, 2);
    let mut app = long_tree_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    let _ = draw_text(&mut app, &mut terminal, &audio);
    let compact = |text: &str| {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    let rows: Vec<(u16, String)> = (2..=8)
        .map(|y| (y, compact(&row_text(&terminal, y))))
        .collect();
    let (selected_y, selected) = rows
        .iter()
        .find(|(_, text)| text.contains("►页面8"))
        .expect("selected ninth view must be visible");
    assert_eq!(*selected_y, 8, "选中行必须在 7 行窗口内");
    assert!(selected.contains("页面8"), "{selected}");
}

#[test]
fn settings_list_scrolls_the_ninth_field_into_six_rows() {
    let (mut app, settings, _audio, _theme, root) = settings_fixture();
    // T-0124 在末尾追加「动效 / 音效」两行，第 9 项仍是「直播缓冲」。
    assert_eq!(app.views[0].rows.len(), 11);
    app.row_index = 8;
    let mut scroll = ScrollState::new(6);
    scroll.follow(8, 11);
    assert_eq!(scroll.first, 3);
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(TestBackend::new(72, 24)).unwrap();
    let _ = draw_text(&mut app, &mut terminal, &audio);

    let visible = row_text(&terminal, 8);
    let compact = visible
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(compact.contains("直播缓冲"), "{visible}");
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(20, 8)].bg, app.palette.fg, "第 9 项反白条在窗口内");
    let _ = std::fs::remove_dir_all(root);
    let _ = settings;
}

#[test]
fn controller_input_scrolls_when_cursor_reaches_right_edge() {
    let mut app = App::new();
    app.enter_command();
    app.input = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    app.cursor = app.input.len();
    let (line, cursor_x) = input_line(&app, 18);
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(text.contains("0123456789"), "{text:?}");
    assert!(!text.contains("abcdefghij"), "{text:?}");
    assert!(cursor_x < 18, "cursor_x={cursor_x}");
}

#[test]
fn player_transport_buttons_reveal_keys_only_with_ctrl() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    app.show_key_hints = false;
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let normal = row_text(&terminal, 22);
    assert!(normal.contains("◀◀ <<"), "{normal:?}");
    assert!(normal.contains(">> ▶▶"), "{normal:?}");
    assert!(normal.contains("┗ ◀◀"), "{normal:?}");
    assert!(!normal.contains("Space"), "{normal:?}");

    app.show_key_hints = true;
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let hinted = row_text(&terminal, 22);
    assert!(hinted.contains("Space"), "{hinted:?}");
    assert!(hinted.contains("[ "), "{hinted:?}");
    assert!(hinted.contains("] "), "{hinted:?}");
    assert!(hinted.contains(". "), "{hinted:?}");
    assert!(hinted.contains(", "), "{hinted:?}");
}

#[test]
fn transport_buttons_are_gray_when_unavailable() {
    use ratatui::style::Modifier;

    let mut app = browse_app();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let prev = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "◀")
        .expect("prev button");
    assert_eq!(prev.style().fg, Some(app.palette.faint));
    assert!(prev.style().add_modifier.contains(Modifier::DIM));
    let play = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "▶")
        .expect("play button");
    assert_eq!(play.style().fg, Some(app.palette.fg));
    assert!(play.style().add_modifier.contains(Modifier::BOLD));
}

#[test]
fn transport_feedback_flashes_the_pressed_button() {
    let mut app = browse_app();
    app.transport_feedback = Some((Action::TogglePlay, std::time::Instant::now()));
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let play = buffer
        .content()
        .iter()
        .find(|cell| cell.symbol() == "▶")
        .expect("play button");
    assert_eq!(play.style().bg, Some(app.palette.accent));
    assert_eq!(
        play.style().fg,
        Some(app.palette.bg.unwrap_or(Color::Reset))
    );
}

#[test]
fn search_platform_is_shown_in_key_words_border() {
    let mut app = App::new();
    app.views = vec![ViewData {
        title: "Search".to_owned(),
        ..ViewData::default()
    }];
    app.view_kind = vec![BrowseKind::Search];
    app.view_depth = vec![0];
    app.focus = Focus::List;
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    let has = |terminal: &Terminal<TestBackend>, needle: &str| {
        (0..24).any(|y| {
            let row: String = row_text(terminal, y)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            row.contains(needle)
        })
    };
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    assert!(has(&terminal, "all"));

    app.search_platform = SearchPlatform::Netease;
    terminal.clear().unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    assert!(has(&terminal, "netease"));

    app.search_platform = SearchPlatform::Bilibili;
    terminal.clear().unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    assert!(has(&terminal, "bilibili"));
}

#[test]
fn status_bar_right_gap_follows_last_text() {
    let mut app = browse_app();
    app.status = "X".to_owned();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    terminal.clear().unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let row: Vec<char> = row_text(&terminal, 23).chars().collect();
    let command = row
        .iter()
        .position(|character| *character == '命')
        .expect("command hint");
    assert_eq!(row[command + 1], ' ');
    assert_eq!(row[command + 2], '令');
    assert_eq!(row[command + 3], ' ');
    assert_eq!(row[command + 4], ' ');
    assert_eq!(row[command + 5], '━');
}

#[test]
fn status_message_uses_highlight_style() {
    use ratatui::style::Modifier;

    let mut app = browse_app();
    app.status = "X".to_owned();
    let audio = TuiAudio::empty();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    terminal.clear().unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app, &audio))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let cell = (0..72)
        .map(|x| &buffer[(x, 23)])
        .find(|cell| cell.symbol() == "X")
        .expect("status cell");
    assert_eq!(cell.style().fg, Some(Palette::terminal().accent));
    assert!(cell.style().add_modifier.contains(Modifier::BOLD));
}

#[test]
fn global_search_entry_reopens_input_and_clears_results() {
    let mut app = App::new();
    app.views = vec![ViewData {
        title: "Search".to_owned(),
        columns: vec!["#".to_owned()],
        rows: vec![vec!["1".to_owned()]],
        ..ViewData::default()
    }];
    app.view_kind = vec![BrowseKind::Search];
    app.view_depth = vec![0];
    app.focus = Focus::Browse;
    app.search_input = "晴天".to_owned();
    app.search_attempted = true;
    crate::search_ui::open_search(&mut app);
    assert_eq!(app.view_index, 0);
    assert_eq!(app.focus, Focus::List);
    assert!(app.search_editing_input());
    assert!(app.views[0].rows.is_empty());
    assert_eq!(app.search_platform, SearchPlatform::All);
    assert_eq!(SearchPlatform::All.next(), SearchPlatform::Netease);
    assert_eq!(SearchPlatform::Netease.next(), SearchPlatform::Bilibili);
    assert_eq!(SearchPlatform::Bilibili.next(), SearchPlatform::All);
}

#[test]
fn controller_executes_resolved_actions() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-controller-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let library = std::sync::Arc::new(np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: root.join("cache"),
        data_dir: root.join("data"),
    }));
    let settings = np_app::SettingsHandle::load(config);
    let theme = TuiThemeApi::new(root.join("themes"));
    let (tx, _rx) = std::sync::mpsc::channel::<(u64, Block)>();
    let mut audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &*library, &settings);
    app.focus = Focus::List;
    app.view_index = app
        .view_kind
        .iter()
        .position(|kind| *kind == BrowseKind::PlaylistRoot)
        .unwrap();
    crate::controller::execute(
        Action::CreatePlaylist,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(app.playlist_input.is_some());

    app.cancel_playlist_create();
    app.view_index = app
        .view_kind
        .iter()
        .position(|kind| *kind == BrowseKind::Search)
        .unwrap();
    crate::controller::execute(
        Action::NextSearchPlatform,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert_eq!(app.search_platform, SearchPlatform::Netease);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_command_is_intercepted_with_a_numeric_room() {
    assert_eq!(parse_live_command("live 123"), Some(Ok(123)));
    assert_eq!(parse_live_command(":live 21622811"), Some(Ok(216_22811)));
    assert_eq!(parse_live_command("/live 23197314"), Some(Ok(23197314)));
    assert_eq!(parse_live_command("  live 510  "), Some(Ok(510)));
    assert!(matches!(parse_live_command("live"), Some(Err(_))));
    assert!(matches!(parse_live_command("live 0"), Some(Err(_))));
    assert!(matches!(parse_live_command("live abc"), Some(Err(_))));
    assert!(matches!(parse_live_command("live 1 extra"), Some(Err(_))));
    assert_eq!(parse_live_command("search 晴天"), None);
    assert_eq!(parse_live_command("play"), None);
}

#[test]
fn live_state_disables_transport_and_has_no_duration() {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    assert!(audio.is_live());
    assert_eq!(audio.live_state(), crate::audio::LiveState::Playing);
    assert_eq!(audio.live_room(), 21622811);
    assert_eq!(audio.duration_ms(), None);
    assert!(!audio.can_next());
    assert!(!audio.can_prev());
    assert!(!audio.can_seek_back());
    assert!(!audio.can_seek_forward());

    let availability = TransportAvailability::from_player(&audio);
    for action in [
        Action::NextTrack,
        Action::PrevTrack,
        Action::SeekBack,
        Action::SeekForward,
    ] {
        assert!(!availability.enabled(action), "{action:?}");
    }
    assert!(availability.enabled(Action::TogglePlay));
}

#[test]
fn live_player_bar_shows_live_instead_of_zero_time() {
    let mut app = browse_app();
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST 决赛");
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    let text = draw_text(&mut app, &mut terminal, &audio);
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(compact.contains("直播"), "live badge missing: {compact}");
    assert!(
        !text.contains("00:00 / 00:00"),
        "live must not render a zero position/duration"
    );
}

#[test]
fn live_page_renders_room_and_anchor_fixtures() {
    let room =
        np_live::api::parse_room_info(include_bytes!("../../np-live/fixtures/room_info.json"))
            .expect("room fixture");
    let anchor = np_live::api::parse_anchor_name(
        br#"{"code":0,"data":{"info":{"uname":"\u76f4\u64ad\u4e3b"}}}"#,
    )
    .expect("anchor fixture");
    assert_eq!(room.area_name, "游戏赛事");

    let audio = TuiAudio::empty();
    audio.live_for_test(room.room_id, &room.title);
    audio.live_meta_for_test(&anchor, &room.area_name);
    let mut app = App::new();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    let compact = |text: &str| -> String { text.chars().filter(|c| !c.is_whitespace()).collect() };
    let text = compact(&draw_text(&mut app, &mut terminal, &audio));
    assert!(text.contains("NOWHEREPLAYER"), "{text}");
    assert!(text.contains(&compact(&anchor)), "{text}");
    assert!(text.contains(&compact(&room.title)), "{text}");
    assert!(text.contains(&compact(&room.area_name)), "{text}");
    assert!(
        text.contains(&compact(&format!("{} - {}", room.title, room.area_name))),
        "info line should join title and area: {text}"
    );
}

#[test]
fn live_buffer_is_exposed_only_while_live() {
    let audio = TuiAudio::empty();
    assert_eq!(audio.live_buffer_ms(), None, "idle has no buffer");

    audio.live_for_test(21622811, "BLAST");
    assert_eq!(
        audio.live_buffer_ms(),
        Some(0),
        "live exposes the queue buffer handle"
    );

    assert!(audio.stop_live());
    assert_eq!(audio.live_buffer_ms(), None, "stopping clears the handle");
}

#[test]
fn live_pause_keeps_room_and_stops_player() {
    use np_app::PlayerApi;

    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    PlayerApi::pause(&audio).expect("pause live");
    assert_eq!(audio.live_state(), crate::audio::LiveState::Paused);
    assert_eq!(audio.live_room(), 21622811);
    assert!(!audio.is_playing());
    let label = audio.live_status_label().expect("paused label");
    assert!(label.starts_with("LIVE PAUSED"), "{label}");
    assert!(audio.stop_live(), "stop must work from paused");
    assert!(!audio.is_live());
}

#[test]
fn live_status_label_exposes_underrun_counters() {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    let label = audio.live_status_label().expect("live label");
    assert!(label.starts_with("LIVE "), "{label}");
    for counter in ["U:", "Q:", "R:", "D:", "E:", "F:", "S:"] {
        assert!(label.contains(counter), "missing {counter} in {label}");
    }
}

#[test]
fn stopping_live_keeps_the_queue() {
    use np_app::PlayerApi;

    let audio = TuiAudio::empty();
    PlayerApi::queue_add(&audio, vec!["a.flac".to_owned(), "b.flac".to_owned()]).unwrap();
    let before = audio.queue_items_snapshot().len();
    assert_eq!(before, 2);
    audio.live_for_test(21622811, "BLAST");
    assert!(audio.stop_live());
    assert!(!audio.is_live());
    assert_eq!(audio.queue_items_snapshot().len(), before);
    assert_eq!(audio.title(), "未播放");
    assert!(!audio.stop_live());
}

#[test]
fn escape_and_transport_actions_respect_live_mode() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-live-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let library = std::sync::Arc::new(np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: root.join("cache"),
        data_dir: root.join("data"),
    }));
    let settings = np_app::SettingsHandle::load(config);
    let theme = TuiThemeApi::new(root.join("themes"));
    let (tx, _rx) = std::sync::mpsc::channel::<(u64, Block)>();
    let mut audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &*library, &settings);
    app.focus = Focus::List;
    audio.live_for_test(21622811, "BLAST");

    crate::controller::execute(
        Action::NextTrack,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert_eq!(app.status, "直播中不可切换");

    crate::controller::execute(
        Action::SeekForward,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert_eq!(app.status, "直播中不可跳转");

    // d 打开浮层并聚焦；此时 Esc 先收起浮层，不进入退出确认。
    crate::controller::execute(
        Action::ToggleDanmaku,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(app.danmaku_overlay_visible());
    assert!(app.live_chat_active());
    crate::controller::execute(
        Action::Escape,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(!app.danmaku_overlay_visible());
    assert!(!app.live_chat_active());
    assert!(!app.live_exit_armed());
    assert!(audio.is_live());

    // T-0101：第一次 Esc 只进入退出确认，不立即停播。
    crate::controller::execute(
        Action::Escape,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(app.live_exit_armed());
    assert!(audio.is_live(), "确认态不能直接停播");
    assert!(app.status.contains("Enter确认退出直播"), "{}", app.status);

    // 确认态 Esc 取消。
    crate::controller::execute(
        Action::Escape,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(!app.live_exit_armed());
    assert!(audio.is_live());
    assert_eq!(app.status, "已取消退出");

    // 再次 Esc 进入确认态，Enter 等价动作确认退出（事件循环调用同一方法）。
    app.arm_live_exit();
    assert!(app.confirm_live_exit(&audio));
    assert_eq!(app.status, "已停止直播");
    assert!(!app.live_exit_armed());
    assert!(!audio.is_live());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn escape_closes_the_video_window_before_live_exit_confirmation() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-video-esc-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let library = std::sync::Arc::new(np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: root.join("cache"),
        data_dir: root.join("data"),
    }));
    let settings = np_app::SettingsHandle::load(config);
    let theme = TuiThemeApi::new(root.join("themes"));
    let (tx, _rx) = std::sync::mpsc::channel::<(u64, Block)>();
    let mut audio = TuiAudio::empty();
    let mut app = App::new();
    app.load_views(&audio, &*library, &settings);
    audio.live_for_test(21622811, "BLAST");
    audio.video_active_for_test();

    crate::controller::execute(
        Action::Escape,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(!audio.video_is_active(), "Esc must close the video first");
    assert!(
        !app.live_exit_armed(),
        "Esc that closes video must not arm the live exit confirmation"
    );
    assert_eq!(app.status, "视频已关闭");
    assert!(audio.is_live(), "closing video must not stop live audio");

    // 视频已关，下一次 Esc 才进入 Live 退出确认。
    crate::controller::execute(
        Action::Escape,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(app.live_exit_armed());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn stop_and_restart_paths_close_the_video_window() {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    audio.video_active_for_test();
    assert!(audio.stop_live());
    assert!(!audio.video_is_active(), "stop_live must close video");

    // F5 / 换房都会先走 shutdown_live；这里不启动网络线程，只验清理路径。
    audio.live_for_test(21622811, "BLAST");
    audio.video_active_for_test();
    audio.shutdown_live_for_test();
    assert!(
        !audio.video_is_active(),
        "refresh/room switch must close video"
    );
    assert!(!audio.is_live());
}

#[test]
fn live_status_bar_shows_a_video_hint_while_the_window_is_open() {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    audio.video_note_for_test("视频准备中…");
    let mut app = App::new();
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(72, 24)).unwrap();
    let text = draw_text(&mut app, &mut terminal, &audio);
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(compact.contains("视频准备中"), "{compact}");
}

fn bili_auth_stub() -> np_live::BiliAuth {
    np_live::BiliAuth::new(
        vec![
            ("SESSDATA".to_owned(), "test-sessdata".to_owned()),
            ("bili_jct".to_owned(), "test-csrf".to_owned()),
            ("DedeUserID".to_owned(), "42".to_owned()),
        ],
        None,
    )
}

fn live_chat_audio() -> TuiAudio {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    audio
}

#[test]
fn live_chat_opens_from_c_and_escape_collapses_the_overlay_without_stopping_live() {
    let root = std::env::temp_dir().join(format!(
        "np-tui-chat-{}-{}",
        std::process::id(),
        now_millis()
    ));
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let library = std::sync::Arc::new(np_app::LibraryHandle::with_paths(np_app::Paths {
        config_dir: config.clone(),
        cache_dir: root.join("cache"),
        data_dir: root.join("data"),
    }));
    let settings = np_app::SettingsHandle::load(config);
    let theme = TuiThemeApi::new(root.join("themes"));
    let (tx, _rx) = std::sync::mpsc::channel::<(u64, Block)>();
    let mut audio = live_chat_audio();
    let mut app = App::new();
    app.load_views(&audio, &*library, &settings);

    // `c` 打开输入框，不写占位状态。
    crate::controller::execute(
        Action::LiveChat,
        &mut app,
        &mut audio,
        &library,
        &tx,
        &theme,
        &settings,
    );
    assert!(app.live_chat_active());
    assert_ne!(app.status, "弹幕输入待接入");

    // Esc 收起整个弹幕框（清空焦点），不停止直播、也不进入退出确认。
    crate::chat_input::handle_chat_key(KeyCode::Esc, &mut app, &audio);
    assert!(!app.live_chat_active());
    assert!(!app.danmaku_overlay_visible());
    assert!(!app.live_exit_armed());
    assert!(audio.is_live(), "Esc in chat must not stop live");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_chat_c_is_a_plain_character_while_the_input_is_open() {
    let audio = live_chat_audio();
    let mut app = App::new();
    app.begin_live_chat();
    for character in ['c', 'd', 'f'] {
        crate::chat_input::handle_chat_key(KeyCode::Char(character), &mut app, &audio);
    }
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.text()),
        Some("cdf".to_owned())
    );
    // c 打开浮层并聚焦 Input；输入时 `d` 只是字符，不会关闭浮层。
    assert!(app.danmaku_overlay_visible());
    assert!(app.live_chat_active());
}

#[test]
fn live_chat_rejects_empty_and_unauthenticated_requests() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(None);
    let mut app = App::new();
    app.begin_live_chat();

    // 空内容不请求。
    app.submit_live_chat(&audio);
    assert_eq!(audio.danmaku_calls_for_test(), 0);
    assert_eq!(
        app.chat_input.as_ref().and_then(|chat| chat.error()),
        Some("弹幕内容不能为空")
    );

    // 未登录不请求，提示 /login bili。
    app.insert_live_chat_char('你');
    app.submit_live_chat(&audio);
    assert_eq!(audio.danmaku_calls_for_test(), 0);
    assert_eq!(
        app.chat_input.as_ref().and_then(|chat| chat.pending()),
        None
    );
    let error = app
        .chat_input
        .as_ref()
        .and_then(|chat| chat.error())
        .expect("login error");
    assert!(error.contains("/login bili"), "{error}");
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.text()),
        Some("你".to_owned())
    );
}

#[test]
fn live_chat_caps_at_twenty_characters_and_counts_cjk_width() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(Some(bili_auth_stub()));
    audio.danmaku_result_for_test(Ok(()));
    let mut app = App::new();
    app.begin_live_chat();

    for index in 0..25 {
        app.insert_live_chat_char(char::from(b'a' + u8::try_from(index).unwrap()));
    }
    let chat = app.chat_input.as_ref().expect("chat");
    assert_eq!(
        chat.text().chars().count(),
        crate::chat_input::CHAT_MAX_CHARS
    );
    assert_eq!(chat.text(), "abcdefghijklmnopqrst");

    // CJK 按字符数计入上限，光标列按显示宽度计。
    let mut cjk = App::new();
    cjk.begin_live_chat();
    for _ in 0..21 {
        cjk.insert_live_chat_char('弹');
    }
    let chat = cjk.chat_input.as_ref().expect("chat");
    assert_eq!(
        chat.text().chars().count(),
        crate::chat_input::CHAT_MAX_CHARS
    );
    assert_eq!(chat.cursor_column(), 40);
    cjk.move_live_chat_cursor(false);
    cjk.move_live_chat_cursor(false);
    assert_eq!(
        cjk.chat_input.as_ref().map(|chat| chat.cursor_column()),
        Some(36)
    );
}

#[test]
fn live_chat_pending_enter_is_idempotent_and_success_clears_the_inline_input() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(Some(bili_auth_stub()));
    audio.danmaku_result_for_test(Ok(()));
    let mut app = App::new();
    app.begin_live_chat();
    app.insert_live_chat_char('你');
    app.insert_live_chat_char('好');

    app.submit_live_chat(&audio);
    assert_eq!(audio.danmaku_calls_for_test(), 1);
    assert!(app.chat_input.as_ref().expect("chat").pending().is_some());

    // pending 期间重复 Enter 不重复发送。
    app.submit_live_chat(&audio);
    assert_eq!(audio.danmaku_calls_for_test(), 1);

    let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
    assert!(result.is_ok());
    assert!(app.apply_live_danmaku_result(generation, result));
    assert!(app.live_chat_active(), "内联 Input 发送成功后保持焦点");
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.text()),
        Some(String::new()),
        "发送成功后清空输入"
    );
    assert_eq!(app.chat_notice.as_deref(), Some("弹幕已发送"));
    // 不写本地回显：等服务器把这条弹幕推回来。
    assert_eq!(audio.danmaku_received(), 0);
    assert!(audio.danmaku_messages().is_empty());
}

#[test]
fn live_chat_failure_keeps_content_and_passes_the_error_through() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(Some(bili_auth_stub()));
    audio.danmaku_result_for_test(Err(np_live::DanmakuError::RateLimited.to_string()));
    let mut app = App::new();
    app.begin_live_chat();
    app.insert_live_chat_char('你');
    app.submit_live_chat(&audio);

    let (generation, result) = audio.poll_live_danmaku().expect("danmaku result");
    assert!(app.apply_live_danmaku_result(generation, result));
    assert!(app.live_chat_active(), "failure keeps the input open");
    let chat = app.chat_input.as_ref().expect("chat");
    assert_eq!(chat.text(), "你");
    assert_eq!(chat.pending(), None);
    let error = chat.error().expect("error");
    assert!(error.contains("过于频繁"), "{error}");
    assert!(error.contains("发送失败"), "{error}");
    assert_eq!(audio.danmaku_calls_for_test(), 1, "no automatic retry");
}

#[test]
fn danmaku_keys_page_while_focused_and_enter_sends() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(Some(bili_auth_stub()));
    audio.danmaku_result_for_test(Ok(()));
    let (sender, _links) = audio.danmaku_channel_for_test();
    for index in 0..40 {
        sender
            .send(np_live::DanmakuMessage {
                user: "观众".to_owned(),
                uid: None,
                text: format!("第{index}条弹幕"),
                color: 16_777_215,
                timestamp: index,
            })
            .expect("send");
    }
    assert!(audio.poll_danmaku());
    audio.danmaku_link_for_test(crate::event_loop::DanmakuLink::Idle);

    let mut app = App::new();
    app.toggle_danmaku_overlay(audio.danmaku_received());
    assert!(app.live_chat_active(), "d 打开时聚焦 Input");
    app.danmaku_overlay.set_scroll_metrics(40, 14);
    app.insert_live_chat_char('你');

    // PgUp/PgDn 在输入聚焦时仍翻页，且不移动文本光标。
    crate::chat_input::handle_chat_key(KeyCode::PageUp, &mut app, &audio);
    assert_eq!(app.danmaku_overlay.offset(), 14);
    crate::chat_input::handle_chat_key(KeyCode::PageUp, &mut app, &audio);
    assert_eq!(app.danmaku_overlay.offset(), 26);
    crate::chat_input::handle_chat_key(KeyCode::PageDown, &mut app, &audio);
    assert_eq!(app.danmaku_overlay.offset(), 12);
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.text()),
        Some("你".to_owned())
    );
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.cursor_column()),
        Some(2),
        "翻页不移动输入光标"
    );
    app.move_live_chat_cursor(false);
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.cursor_column()),
        Some(0),
        "翻页不移动输入光标"
    );

    // f 在输入聚焦时只是普通字符，不会跟随。
    crate::chat_input::handle_chat_key(KeyCode::Char('f'), &mut app, &audio);
    assert_eq!(app.danmaku_overlay.offset(), 12);
    assert_eq!(
        app.chat_input.as_ref().map(|chat| chat.text()),
        Some("f你".to_owned()),
        "f 是普通字符，插在光标位置"
    );

    // Enter 走真正的发送路径（handle_chat_key）。
    crate::chat_input::handle_chat_key(KeyCode::Enter, &mut app, &audio);
    assert_eq!(audio.danmaku_calls_for_test(), 1);
    assert!(app.chat_input.as_ref().expect("chat").pending().is_some());
}

#[test]
fn danmaku_follows_from_the_unfocused_overlay() {
    let mut app = App::new();
    app.toggle_danmaku_overlay(0);
    app.danmaku_overlay.set_scroll_metrics(40, 14);
    assert!(app.scroll_danmaku(true));
    // d 打开会聚焦 Input；Esc 相当于收起，这里直接失焦以验证 f。
    app.cancel_live_chat();
    assert!(app.danmaku_overlay_visible());
    assert!(!app.live_chat_active());
    assert!(crate::danmaku_overlay::handle_key(
        &mut app,
        KeyCode::PageUp
    ));
    assert_eq!(app.danmaku_overlay.offset(), 15);
    assert!(crate::danmaku_overlay::handle_key(
        &mut app,
        KeyCode::PageDown
    ));
    assert_eq!(app.danmaku_overlay.offset(), 1);
    assert!(crate::danmaku_overlay::handle_key(
        &mut app,
        KeyCode::Char('f')
    ));
    assert_eq!(app.danmaku_overlay.offset(), 0);
}

#[test]
fn live_chat_ignores_stale_generations() {
    let audio = live_chat_audio();
    audio.bili_auth_for_test(Some(bili_auth_stub()));
    audio.danmaku_result_for_test(Ok(()));
    let mut app = App::new();
    app.begin_live_chat();
    app.insert_live_chat_char('你');
    app.submit_live_chat(&audio);
    let stale_generation = app
        .chat_input
        .as_ref()
        .and_then(|chat| chat.pending())
        .expect("pending generation");

    // 停播会推进 generation：旧房间结果不再回传。
    assert!(audio.stop_live());
    assert_eq!(audio.poll_live_danmaku(), None);

    // 即使结果被送到 UI，generation 不匹配也不能改变输入框。
    assert!(!app.apply_live_danmaku_result(stale_generation.wrapping_add(1), Ok(())));
    assert!(app.live_chat_active());
    assert_eq!(
        audio.danmaku_received(),
        0,
        "no local echo for stale result"
    );
}

/// T-0116 / T-0117：视频窗口控制条、清晰度菜单与无缝换流的单元测试。
///
/// 根模块测试与 `audio::video` 是兄弟模块，直接无法访问其私有类型；因此
/// `video.rs` 提供 `#[cfg(test)]` 门面（只返回普通结构），这里只测行为。
#[cfg(test)]
mod video_window {
    use crate::audio::TuiAudio;
    use crate::theme::{Palette, VideoTheme};
    use ratatui::style::Color;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("np-video-{tag}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn fake_executable(name: &str) -> String {
        #[cfg(windows)]
        {
            format!("{name}.exe")
        }
        #[cfg(not(windows))]
        {
            name.to_owned()
        }
    }

    fn path_var(dir: &Path) -> std::ffi::OsString {
        std::env::join_paths([dir]).expect("join path")
    }

    fn live_info() -> np_live::api::PlayInfo {
        np_live::api::parse_play_info(
            include_bytes!("../../np-live/fixtures/live_playinfo.json"),
            21622811,
        )
        .expect("live fixture")
    }

    fn exit_child() -> Child {
        #[cfg(windows)]
        let mut command = {
            let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
            let mut command = Command::new(shell);
            command.args(["/C", "exit 0"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 0"]);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fake exit process")
    }

    #[test]
    fn mpv_args_disable_audio_and_carry_live_headers() {
        let args = TuiAudio::video_player_args_for_test(true, "https://example.invalid/live.flv");
        assert!(args.iter().any(|arg| arg == "--no-audio"));
        assert!(args.iter().any(|arg| arg == "--force-window=yes"));
        assert!(args.iter().any(|arg| arg == "--really-quiet"));
        assert!(args.iter().any(|arg| {
            arg == "--http-header-fields=Referer: https://live.bilibili.com/,Origin: https://live.bilibili.com"
        }));
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.invalid/live.flv")
        );
    }

    #[test]
    fn ffplay_args_disable_audio_and_use_crlf_headers() {
        let args = TuiAudio::video_player_args_for_test(false, "https://example.invalid/live.flv");
        assert_eq!(args[0], "-an");
        assert_eq!(args[1], "-loglevel");
        assert_eq!(args[2], "error");
        assert_eq!(args[3], "-window_title");
        assert_eq!(args[4], "NowhereLive");
        assert_eq!(args[5], "-headers");
        assert_eq!(
            args[6],
            "Referer: https://live.bilibili.com/\\r\\nOrigin: https://live.bilibili.com"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.invalid/live.flv")
        );
        assert!(!args.iter().any(|arg| arg == "-vn" || arg == "-no-audio"));
    }

    #[test]
    fn player_command_uses_the_selected_path_and_args() {
        let (program, args) =
            TuiAudio::video_player_command_for_test(true, "https://example.invalid/live.flv");
        assert_eq!(program, PathBuf::from("fake-player"));
        assert_eq!(
            args,
            TuiAudio::video_player_args_for_test(true, "https://example.invalid/live.flv")
        );
    }

    #[test]
    fn mpv_is_preferred_then_ffplay_falls_back() {
        let dir = temp_dir("path-fallback");
        std::fs::write(dir.join(fake_executable("ffplay")), b"").expect("ffplay");
        let (ffplay, path) = TuiAudio::video_resolve_program_for_test(None, Some(&path_var(&dir)))
            .expect("ffplay fallback");
        assert!(ffplay);
        assert_eq!(path, dir.join(fake_executable("ffplay")));

        std::fs::write(dir.join(fake_executable("mpv")), b"").expect("mpv");
        let (ffplay, path) = TuiAudio::video_resolve_program_for_test(None, Some(&path_var(&dir)))
            .expect("mpv preferred");
        assert!(!ffplay);
        assert_eq!(path, dir.join(fake_executable("mpv")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_player_overrides_path_lookup() {
        let dir = temp_dir("override");
        let custom = dir.join(fake_executable("my-ffplay"));
        std::fs::write(&custom, b"").expect("custom player");
        let (ffplay, path) = TuiAudio::video_resolve_program_for_test(
            Some(custom.as_os_str()),
            Some(&path_var(&dir)),
        )
        .expect("override");
        assert!(ffplay);
        assert_eq!(path, custom);

        // 纯命令名走 PATH，而不是当前目录。
        let name = fake_executable("my-mpv");
        std::fs::write(dir.join(&name), b"").expect("named player");
        let (ffplay, path) = TuiAudio::video_resolve_program_for_test(
            Some(OsStr::new(&name)),
            Some(&path_var(&dir)),
        )
        .expect("path name");
        assert!(!ffplay);
        assert_eq!(path, dir.join(&name));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_player_returns_a_chinese_install_hint() {
        let dir = temp_dir("missing");
        let error =
            TuiAudio::video_resolve_program_for_test(None, Some(&path_var(&dir))).unwrap_err();
        assert!(error.contains("未找到视频播放器"), "{error}");
        assert!(error.contains("mpv") && error.contains("ffplay"), "{error}");

        let missing = dir.join(fake_executable("nope"));
        let error = TuiAudio::video_resolve_program_for_test(
            Some(missing.as_os_str()),
            Some(&path_var(&dir)),
        )
        .unwrap_err();
        assert!(error.contains("NP_VIDEO_PLAYER"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn host_resolution_prefers_env_then_sibling_then_path() {
        let dir = temp_dir("hosts");
        let exe_dir = dir.join("bin");
        std::fs::create_dir_all(&exe_dir).expect("bin dir");
        let sibling = exe_dir.join(TuiAudio::video_host_exe_for_test());
        std::fs::write(&sibling, b"").expect("sibling host");
        assert_eq!(
            TuiAudio::video_resolve_host_for_test(None, Some(&exe_dir), Some(&path_var(&dir)))
                .expect("sibling"),
            Some(sibling.clone())
        );

        let override_path = dir.join(fake_executable("custom-video"));
        std::fs::write(&override_path, b"").expect("override host");
        assert_eq!(
            TuiAudio::video_resolve_host_for_test(
                Some(override_path.as_os_str()),
                Some(&exe_dir),
                Some(&path_var(&dir))
            )
            .expect("override"),
            Some(override_path)
        );

        let missing = dir.join(fake_executable("nope"));
        let error = TuiAudio::video_resolve_host_for_test(
            Some(missing.as_os_str()),
            Some(&exe_dir),
            Some(&path_var(&dir)),
        )
        .unwrap_err();
        assert!(error.contains("NP_VIDEO_HOST"), "{error}");

        // sibling 不存在时走 PATH。
        std::fs::remove_file(&sibling).expect("remove sibling");
        let path_host = dir.join(fake_executable("np-video"));
        std::fs::write(&path_host, b"").expect("path host");
        assert_eq!(
            TuiAudio::video_resolve_host_for_test(None, Some(&exe_dir), Some(&path_var(&dir)))
                .expect("path"),
            Some(path_host)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backend_prefers_host_and_falls_back_to_external() {
        let dir = temp_dir("backend");
        std::fs::write(dir.join(fake_executable("mpv")), b"").expect("mpv");
        let path = path_var(&dir);
        let host = TuiAudio::video_resolve_backend_for_test(None, None, Some(&dir), Some(&path))
            .expect("external");
        assert!(!host);

        #[cfg(windows)]
        {
            std::fs::write(dir.join(TuiAudio::video_host_exe_for_test()), b"").expect("host");
            let host =
                TuiAudio::video_resolve_backend_for_test(None, None, Some(&dir), Some(&path))
                    .expect("host");
            assert!(host);

            // 只有 ffplay 时宿主无法嵌入，必须回退外部窗口。
            std::fs::remove_file(dir.join(fake_executable("mpv"))).expect("remove mpv");
            std::fs::write(dir.join(fake_executable("ffplay")), b"").expect("ffplay");
            let host =
                TuiAudio::video_resolve_backend_for_test(None, None, Some(&dir), Some(&path))
                    .expect("ffplay fallback");
            assert!(!host);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn host_request_is_line_based_and_sanitizes_the_title() {
        let request = TuiAudio::video_host_request_for_test(
            "https://example.invalid/live.flv?token=secret",
            "坏\n标题",
            &[],
            None,
            10_000,
            65,
            true,
            1_200,
            VideoTheme::default_dark(),
        )
        .expect("request");
        let lines: Vec<&str> = request.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "v2");
        assert!(lines[1].starts_with("url https://example.invalid/live.flv"));
        assert_eq!(lines[4], "title 坏 标题");
        assert_eq!(lines[5], "volume 65");
        assert_eq!(lines[6], "paused 1");
        assert_eq!(lines[7], "buffer_ms 1200");
        assert_eq!(
            lines[8],
            "theme #ffb454 #14171c #1a1e24 #e9e4da #a7a49e #7d7c78 #1e2023 #ff8b7d #0d0f12"
        );
        assert_eq!(lines[9], "end");
        assert!(!request.contains("\n\n"));
        // 主题行不含 URL / Cookie / 标题。
        assert!(!lines[8].contains("http") && !lines[8].contains("token="));

        assert!(
            TuiAudio::video_host_request_for_test(
                "https://example.invalid/live.flv\nX",
                "",
                &[],
                None,
                10_000,
                65,
                true,
                1_200,
                VideoTheme::default_dark(),
            )
            .unwrap_err()
            .contains("地址")
        );
    }

    #[test]
    fn host_request_carries_the_quality_menu_and_current_qn() {
        let qualities = vec![
            (10_000u64, "原画".to_owned()),
            (400, "蓝光".to_owned()),
            (250, "超清".to_owned()),
        ];
        let request = TuiAudio::video_host_request_for_test(
            "https://example.invalid/live.flv",
            "房间",
            &qualities,
            Some(250),
            10_000,
            100,
            false,
            900,
            VideoTheme::default_dark(),
        )
        .expect("request");
        let lines: Vec<&str> = request.lines().collect();
        assert_eq!(lines.len(), 14);
        assert!(lines[8].starts_with("theme #"));
        assert_eq!(lines[9], "quality 10000 原画");
        assert_eq!(lines[10], "quality 400 蓝光");
        assert_eq!(lines[11], "quality 250 超清");
        assert_eq!(lines[12], "current_qn 250");
        assert_eq!(lines[13], "end");

        // 标签里的控制字符不能注入协议行。
        let hostile = vec![(400u64, "蓝\n光".to_owned())];
        let request = TuiAudio::video_host_request_for_test(
            "https://example.invalid/live.flv",
            "房间",
            &hostile,
            Some(250),
            10_000,
            100,
            false,
            900,
            VideoTheme::default_dark(),
        )
        .expect("sanitized");
        let lines: Vec<&str> = request.lines().collect();
        assert_eq!(lines[9], "quality 400 蓝 光");
        assert_eq!(lines[10], "current_qn 250");
    }

    #[test]
    fn load_line_rejects_urls_that_could_break_the_protocol() {
        assert_eq!(
            TuiAudio::video_load_line_for_test("https://example.invalid/live.flv?token=x"),
            Some("load https://example.invalid/live.flv?token=x\n".to_owned())
        );
        for bad in [
            "",
            "https://e.invalid/a\nb",
            "https://e.invalid/a b",
            "#comment",
            "ftp://e.invalid/x",
        ] {
            assert!(TuiAudio::video_load_line_for_test(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn target_uses_room_title_or_live_fallback() {
        let (url, title, _, _, _, requested_qn) = TuiAudio::video_target_from_parts_for_test(
            21622811,
            live_info(),
            Some("BLAST 直播".to_owned()),
            10_000,
        )
        .expect("target");
        assert_eq!(title, "BLAST 直播");
        assert!(url.starts_with("http"));
        assert_eq!(requested_qn, 10_000);

        let (_, title, _, _, _, _) = TuiAudio::video_target_from_parts_for_test(
            21622811,
            live_info(),
            Some("  ".to_owned()),
            10_000,
        )
        .expect("fallback title");
        assert_eq!(title, "直播 21622811");

        let offline = np_live::api::parse_play_info(
            include_bytes!("../../np-live/fixtures/offline_playinfo.json"),
            510,
        )
        .expect("offline fixture");
        assert_eq!(
            TuiAudio::video_target_from_parts_for_test(510, offline, None, 10_000).unwrap_err(),
            "直播间未开播"
        );
    }

    #[test]
    fn target_maps_accept_qn_to_menu_options() {
        let (_, _, quality_note, qualities, current_qn, requested_qn) =
            TuiAudio::video_target_from_parts_for_test(21622811, live_info(), None, 10_000)
                .expect("quality target");
        // fixture：accept_qn = [10000,400,250]，current_qn = 250。
        assert_eq!(
            qualities,
            vec![
                (10_000, "原画".to_owned()),
                (400, "蓝光".to_owned()),
                (250, "超清".to_owned())
            ]
        );
        assert_eq!(current_qn, Some(250));
        assert_eq!(requested_qn, 10_000);
        let note = quality_note.expect("note");
        assert!(note.contains("超清 250"), "{note}");

        // 空 accept_qn 时至少保留当前请求档位。
        let mut empty = live_info();
        empty.accept_qn.clear();
        let (_, _, _, qualities, _, _) =
            TuiAudio::video_target_from_parts_for_test(21622811, empty, None, 400)
                .expect("fallback option");
        assert_eq!(qualities, vec![(400, "蓝光".to_owned())]);
    }

    /// 多 codec playurl：AVC current=10000、HEVC current=15000，union 含 20000。
    const MULTI_CODEC_PLAYINFO: &str = r#"{"code":0,"data":{"room_id":21622811,"live_status":1,"playurl_info":{"playurl":{"stream":[{"protocol_name":"http_stream","format":[{"format_name":"flv","codec":[
        {"codec_name":"avc","current_qn":10000,"accept_qn":[10000,400,250,150,80],"base_url":"/avc/stream.flv?","url_info":[{"host":"https://avc.invalid"}]},
        {"codec_name":"hevc","current_qn":15000,"accept_qn":[20000,15000,10000,400,250],"base_url":"/hevc/stream.flv?","url_info":[{"host":"https://hevc.invalid"}]}
    ]}]}]}}}}"#;

    #[test]
    fn quality_menu_lists_every_accept_qn_highest_first() {
        let info = np_live::api::parse_play_info(MULTI_CODEC_PLAYINFO.as_bytes(), 21622811)
            .expect("multi codec fixture");
        let (url, _, quality_note, qualities, current_qn, requested_qn) =
            TuiAudio::video_target_from_parts_for_test(21622811, info, None, 20_000)
                .expect("quality target");
        assert!(url.starts_with("https://hevc.invalid/"));
        assert_eq!(
            qualities,
            vec![
                (20_000, "4K".to_owned()),
                (15_000, "2K".to_owned()),
                (10_000, "原画".to_owned()),
                (400, "蓝光".to_owned()),
                (250, "超清".to_owned()),
                (150, "高清".to_owned()),
                (80, "流畅".to_owned()),
            ]
        );
        assert_eq!(current_qn, Some(15_000));
        assert_eq!(requested_qn, 20_000);
        let note = quality_note.expect("quality note");
        assert!(note.contains("2K 15000"), "{note}");
        assert!(note.contains("4K 20000"), "{note}");
    }

    #[test]
    fn video_default_quality_is_the_top_regular_tier() {
        assert_eq!(TuiAudio::video_default_qn_for_test(), 20_000);
    }

    #[test]
    fn host_events_parse_ready_closed_and_error() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("ready\r\n"));
        assert!(audio.video_host_line_for_test("closed"));
        assert!(audio.video_host_line_for_test("error 播放器退出"));
        assert!(!audio.video_host_line_for_test("unknown"));
    }

    #[test]
    fn host_events_parse_control_bar_actions() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("control paused 1"));
        assert!(audio.video_host_line_for_test("control paused 0"));
        assert!(audio.video_host_line_for_test("control refresh"));
        assert!(audio.video_host_line_for_test("control quality 400"));
        assert!(audio.video_host_line_for_test("volume 65"));
        // 非法值不猜。
        assert!(!audio.video_host_line_for_test("control paused maybe"));
        assert!(!audio.video_host_line_for_test("volume 101"));
        assert!(!audio.video_host_line_for_test("control quality abc"));
    }

    #[test]
    fn host_pause_and_volume_events_do_not_tear_down_the_window() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("ready"));
        assert!(audio.poll_video());
        assert!(audio.video_host_line_for_test("control paused 1"));
        assert!(audio.video_host_line_for_test("volume 37"));
        assert!(audio.poll_video());
        assert!(
            audio.video_is_active(),
            "control bar events keep the host alive"
        );
        assert_eq!(audio.video_audio_state_for_test(), (37, true));
        // poll_video 已把宿主音量同步到 TUI master volume。
        assert!((audio.volume() - 0.37).abs() < 0.001);
    }

    #[test]
    fn quality_switch_failure_keeps_the_old_picture() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("ready"));
        audio.poll_video();
        assert!(audio.video_host_line_for_test("error 清晰度切换失败，已恢复原画面"));
        assert!(audio.poll_video());
        assert!(
            audio.video_is_active(),
            "loadfile failure must not tear down the host"
        );
        let hint = audio.video_status_hint().expect("load failure hint");
        assert!(hint.contains("清晰度切换失败"), "{hint}");
    }

    #[test]
    fn playing_child_exit_is_reaped_and_reported() {
        let audio = TuiAudio::empty();
        audio.video_attach_child_for_test(exit_child());
        assert!(audio.video_is_active());

        let mut seen = false;
        for _ in 0..200 {
            if audio.poll_video() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(seen, "poll should reap the exited fake process");
        assert!(!audio.video_is_active());
        let hint = audio.video_status_hint().expect("exit hint");
        assert!(hint.contains("视频窗口已关闭"), "{hint}");
    }

    #[test]
    fn close_is_idempotent_and_marks_video_closed() {
        let audio = TuiAudio::empty();
        audio.video_attach_child_for_test(exit_child());
        assert!(audio.close_video());
        assert!(!audio.video_is_active());
        assert_eq!(audio.video_status_hint().as_deref(), Some("视频已关闭"));
        assert!(!audio.close_video());
    }

    #[test]
    fn toggle_without_live_is_refused_before_any_process() {
        let audio = TuiAudio::empty();
        assert_eq!(
            audio.toggle_video(),
            Err("当前没有直播".to_owned()),
            "idle toggle must not start a player"
        );
        assert!(!audio.video_is_active());
    }

    #[test]
    fn toggle_while_active_closes_instead_of_opening_a_second_player() {
        let audio = TuiAudio::empty();
        audio.video_active_for_test();
        assert_eq!(
            audio.toggle_video(),
            Ok("视频已关闭".to_owned()),
            "second v must close, not spawn"
        );
        assert!(!audio.video_is_active());
    }

    #[test]
    fn only_the_host_backend_uses_mpv_audio() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_uses_mpv_audio());

        let external = TuiAudio::empty();
        external.video_active_for_test();
        assert!(!external.video_uses_mpv_audio());

        audio.set_video_volume(0.37);
        audio.video_set_paused_for_test(true);
        assert_eq!(audio.video_audio_state_for_test(), (37, true));
    }

    #[test]
    fn host_ready_moves_to_playing_and_closed_returns_to_idle() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("ready"));
        assert!(audio.poll_video());
        assert_eq!(
            audio.video_status_hint().as_deref(),
            Some("视频已打开（v 关闭）")
        );

        assert!(audio.video_host_line_for_test("closed"));
        assert!(audio.poll_video());
        assert!(!audio.video_is_active());
        assert_eq!(audio.video_status_hint().as_deref(), Some("视频窗口已关闭"));
    }

    #[test]
    fn host_error_returns_to_idle_with_a_chinese_note() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_host_line_for_test("error mpv 启动失败"));
        assert!(audio.poll_video());
        assert!(!audio.video_is_active());
        let hint = audio.video_status_hint().expect("error hint");
        assert!(hint.contains("视频启动失败"), "{hint}");
        assert!(hint.contains("mpv 启动失败"), "{hint}");
    }

    #[test]
    fn host_process_exit_without_an_event_is_reported() {
        let audio = TuiAudio::empty();
        audio.video_exiting_host_for_test();
        let mut seen = false;
        for _ in 0..200 {
            if audio.poll_video() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(seen);
        assert!(!audio.video_is_active());
        let hint = audio.video_status_hint().expect("exit hint");
        assert!(hint.contains("启动失败") || hint.contains("窗口"), "{hint}");
    }

    #[test]
    fn toggle_while_a_host_is_active_closes_it() {
        let audio = TuiAudio::empty();
        audio.video_host_for_test();
        assert!(audio.video_is_active());
        assert_eq!(audio.toggle_video(), Ok("视频已关闭".to_owned()));
        assert!(!audio.video_is_active());
    }

    #[test]
    fn fetch_error_moves_back_to_idle_with_a_status_hint() {
        let audio = TuiAudio::empty();
        audio.video_inject_fetch_error_for_test("模拟网络断开");
        assert!(audio.video_is_active(), "preparing counts as active");
        assert!(audio.poll_video());
        assert!(!audio.video_is_active());
        let hint = audio.video_status_hint().expect("fetch failure hint");
        assert!(hint.contains("视频取流失败"), "{hint}");
        assert!(hint.contains("模拟网络断开"), "{hint}");
    }

    #[test]
    fn video_theme_sync_is_idempotent_and_reaches_the_host() {
        let audio = TuiAudio::empty();
        let mut palette = Palette::default_dark();
        assert!(audio.sync_video_theme(&palette));
        assert_eq!(
            audio.video_theme_for_test(),
            Some(VideoTheme::from_palette(&palette))
        );
        assert!(
            !audio.sync_video_theme(&palette),
            "unchanged palette must not resend the theme line"
        );

        // 宿主在线时同样只发一次；stdin 为 None 的假宿主不会真的写管道。
        audio.video_host_for_test();
        palette.accent = Color::Rgb(0x11, 0x22, 0x33);
        assert!(audio.sync_video_theme(&palette));
        assert_eq!(
            audio.video_theme_for_test(),
            Some(VideoTheme::from_palette(&palette))
        );
        assert!(!audio.sync_video_theme(&palette));
    }

    #[test]
    fn terminal_palette_sync_uses_the_default_dark_video_theme() {
        let audio = TuiAudio::empty();
        assert!(audio.sync_video_theme(&Palette::terminal()));
        assert_eq!(
            audio.video_theme_for_test(),
            Some(VideoTheme::default_dark())
        );
    }

    #[test]
    fn notes_take_priority_over_phase_hints() {
        let audio = TuiAudio::empty();
        audio.video_note_for_test("视频准备中…");
        assert_eq!(audio.video_status_hint().as_deref(), Some("视频准备中…"));
        audio.video_active_for_test();
        assert_eq!(
            audio.video_status_hint().as_deref(),
            Some("视频准备中…"),
            "fresh note must win over the persistent phase hint"
        );
    }
}
