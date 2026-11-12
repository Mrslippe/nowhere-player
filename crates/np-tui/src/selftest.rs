use std::sync::Arc;

use crate::audio::TuiAudio;
use crate::theme::TuiThemeApi;
use np_app::{
    Execution, LibraryApi, LibraryHandle, NoopLibraryObserver, PlayerApi, Services, SettingsHandle,
    execute_with_services, online_play_hit,
};
use np_core::{Block, BlockStatus, Output};

pub(crate) fn selftest_block_text(block: &Block) -> String {
    let mut lines = Vec::new();
    for output in &block.outputs {
        match output {
            Output::Text { spans, .. } => lines.push(
                spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
            ),
            Output::Table { columns, rows, .. } => {
                lines.push(columns.join("\t"));
                for row in rows {
                    lines.push(row.join("\t"));
                }
            }
            Output::Progress {
                label,
                current,
                total,
                unit,
            } => lines.push(format!("{label} {current}/{total} {unit}")),
            Output::Error {
                code,
                message,
                hint,
                ..
            } => lines.push(format!(
                "{code}: {message}{}",
                hint.as_deref()
                    .map(|hint| format!(" ({hint})"))
                    .unwrap_or_default()
            )),
            Output::Hint { text, .. } => lines.push(text.clone()),
            Output::Media { kind, r#ref, .. } => lines.push(format!("[media {kind:?}] {ref}")),
        }
    }
    if let Some(footer) = &block.footer {
        lines.push(footer.clone());
    }
    lines.join("\n")
}

pub(crate) fn run_selftest(
    script: &str,
    player: &TuiAudio,
    library: &Arc<LibraryHandle>,
    settings: &SettingsHandle,
    theme: &TuiThemeApi,
) -> Result<(), Box<dyn std::error::Error>> {
    let online = library.online();
    let mut failed = false;
    for command in script
        .split(';')
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        println!("$ {command}");
        if let Ok(Some(parsed)) = np_core::command::parse(command)
            && parsed.spec.name == "search"
            && let Ok(options) = np_app::SearchOptions::parse(&parsed)
            && !options.local
        {
            let template = np_app::search_started_block(
                &format!("selftest-{command}"),
                command,
                &options.keyword,
            );
            let last = std::sync::Arc::new(std::sync::Mutex::new(None));
            let emit = |block: &Block| {
                *last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(block.clone());
            };
            np_app::run_online_search_with_options(
                &options,
                &*online,
                &**library,
                &emit,
                &template,
                &np_app::now_millis,
            );
            let block = last
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(block) = block {
                let text = selftest_block_text(&block);
                println!("{text}");
                if !matches!(block.status, BlockStatus::Done) {
                    eprintln!("{text}");
                    failed = true;
                }
            } else {
                eprintln!("search produced no block");
                failed = true;
            }
            continue;
        }

        if let Ok(Some(parsed)) = np_core::command::parse(command)
            && let Some((index, track)) = online_play_hit(
                &parsed,
                library.active_listing().as_ref(),
                &library.search_hits(),
            )
        {
            match online.prepare(&track, &mut |_| {}) {
                Ok(prepared) => {
                    println!("prepared: {}", prepared.path.display());
                    match player.play(Some(prepared.path.to_string_lossy().into_owned())) {
                        Ok(state) => println!("playing: {:?} · index {}", state.status, index),
                        Err(error) => {
                            eprintln!("play failed: {}: {}", error.code, error.message);
                            failed = true;
                        }
                    }
                }
                Err(error) => {
                    eprintln!("prepare failed: {}: {}", error.code, error.message);
                    failed = true;
                }
            }
            continue;
        }

        let execution = execute_with_services(
            player,
            &**library,
            Services {
                settings: Some(settings),
                theme: Some(theme),
                online: Some(&*online),
            },
            &format!("selftest-{command}"),
            command,
            Arc::new(NoopLibraryObserver),
        );
        match execution {
            Execution::Ignored => println!("(ignored)"),
            Execution::Blocks { done, .. } => {
                let text = selftest_block_text(&done);
                println!("{text}");
                if matches!(done.status, BlockStatus::Error) {
                    eprintln!("{text}");
                    failed = true;
                }
            }
            Execution::Accepted { running } => {
                println!("{}", selftest_block_text(&running));
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
