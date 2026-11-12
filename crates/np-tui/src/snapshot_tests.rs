use ratatui::Terminal;
use ratatui::widgets::Clear;

use crate::app::*;
use crate::audio::TuiAudio;
use crate::input::*;
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
    }];
    app.focus = Focus::List;
    app.row_index = 1;
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
fn golden_completion_popup() {
    let mut app = App::new();
    app.completion = Some(np_core::Completion {
        replace_start: 0,
        replace_end: 3,
        items: vec![
            np_core::CompletionItem {
                text: "search".to_owned(),
                kind: np_core::CompletionKind::Command,
                summary: "搜索".to_owned(),
                usage: "search [关键词]".to_owned(),
            },
            np_core::CompletionItem {
                text: "scan".to_owned(),
                kind: np_core::CompletionKind::Command,
                summary: "扫描".to_owned(),
                usage: "library scan".to_owned(),
            },
            np_core::CompletionItem {
                text: "seek".to_owned(),
                kind: np_core::CompletionKind::Command,
                summary: "跳转".to_owned(),
                usage: "seek <毫秒>".to_owned(),
            },
        ],
    });
    app.completion_index = 1;
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 12)).expect("terminal");
    terminal
        .draw(|frame| {
            frame.render_widget(
                Clear,
                ratatui::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 12,
                },
            );
            draw_completion(
                frame,
                ratatui::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 80,
                    height: 10,
                },
                &app,
            );
        })
        .expect("draw");
    check_golden(
        "completion_popup.txt",
        &buffer_text(terminal.backend().buffer()),
    );
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
    app.focus = Focus::Library;
    snapshot_app("status_library.txt", 100, 30, &mut app, &audio);
    app.focus = Focus::List;
    snapshot_app("status_list.txt", 100, 30, &mut app, &audio);
    app.mode = UiMode::Filter;
    app.filter = "晴".to_owned();
    app.input = "晴".chars().collect();
    app.cursor = app.input.len();
    snapshot_app("status_filter.txt", 100, 30, &mut app, &audio);
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
