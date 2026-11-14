use super::*;
use crate::audio::TuiAudio;
use crate::layout::*;
use crate::render::*;
use crate::theme::Palette;
use np_app::PlaybackTarget;
use np_core::{Block, BlockStatus, Output};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
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
fn splash_cycles_themes_and_wraps() {
    assert_eq!(splash_step("milkgreen", 1), "milkgreen");
    assert_eq!(splash_step("milkgreen", -1), "milkgreen");
    assert_eq!(splash_theme_ids(), vec!["milkgreen"]);
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
    assert_eq!(vertical_heights(28), (12, 13, 3)); // 100x30
    assert_eq!(vertical_heights(38), (17, 18, 3)); // 140x40
    assert_eq!(vertical_heights(22), (9, 10, 3)); // 72/80x24
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
        assert!(
            controller.y >= player.bottom(),
            "{width}x{height} controller {controller:?}"
        );
        assert_eq!(controller.bottom(), outer.bottom() - 1);
        assert_eq!(outer.width, width);
    }
}

#[test]
fn chinese_title_truncates_by_display_width() {
    let text = truncate_width("晴天 · Jay Chou", 8);
    assert_eq!(UnicodeWidthStr::width(text.as_str()), 8);
}

#[test]
fn controller_is_always_visible_and_colon_focuses_it() {
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
    assert!(text.replace(' ', "").contains("Controller"));
    app.enter_command();
    assert_eq!(app.focus, Focus::Controller);
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
        },
        ViewData {
            title: "Like".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
        },
        ViewData {
            title: "媒体库".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: vec![vec!["1".to_owned(), "晴天".to_owned()]],
            refs: Vec::new(),
        },
        ViewData {
            title: "搜索".to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
            refs: Vec::new(),
        },
        ViewData {
            title: "队列".to_owned(),
            columns: vec!["序号".to_owned(), "标题".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
        },
        ViewData {
            title: "设置".to_owned(),
            columns: vec!["设置项".to_owned(), "值".to_owned()],
            rows: Vec::new(),
            refs: Vec::new(),
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
    assert!(compact.contains("退出"), "{text:?}");
    assert!(!text.contains("就绪"), "{text:?}");
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
    assert_eq!(buffer[(2, 20)].symbol(), "┏");
    assert_eq!(buffer[(69, 20)].symbol(), "┓");
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
