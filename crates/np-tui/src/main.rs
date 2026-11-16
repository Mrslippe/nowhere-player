//! T-0060 np-tui 骨架：输入行 + 执行 np-app 命令 + 块输出。
//!
//! 这一版只覆盖任务卡的 1/4/5：输入、执行、text/table/error/hint。
//! 在线搜索异步分派、补全、播放器面板留给后续卡片。

use std::io;
use std::sync::Arc;

mod app;
mod app_dirs;
mod audio;
mod fft;
mod glyphs;
mod input;
mod instance;
mod launcher;
mod layout;
mod logo;
mod model;
mod osc;
mod playback;
mod player_bar;
mod render;
mod selftest;
mod settings_page;
mod splash;
mod startup;
mod theme;
#[cfg(windows)]
mod tray;

use app::*;
use audio::TuiAudio;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use event_loop::*;
use input::*;
use selftest::*;
use splash::*;

use np_app::{LibraryHandle, Paths, SettingsHandle};
use ratatui::{Terminal, backend::CrosstermBackend};
use theme::TuiThemeApi;

fn log_singleton(acquired: bool) {
    let path = std::env::temp_dir().join("nowhere-tui-launch.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(
            file,
            "singleton: {}",
            if acquired { "acquired" } else { "secondary" }
        );
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let selftest = std::env::var("NP_TUI_SELFTEST")
        .ok()
        .filter(|text| !text.trim().is_empty());
    if selftest.is_none() && !launcher::in_windows_terminal() {
        let _ = launcher::relaunch_from_explorer();
        return Ok(());
    }
    // bootstrap 之后再取单实例锁：父进程不能持有锁再拉子进程。
    if selftest.is_none() {
        let acquired = instance::acquire();
        log_singleton(acquired);
        if !acquired {
            std::process::exit(0);
        }
    }
    let started = std::time::Instant::now();
    if selftest.is_none() {
        let _ = launcher::install_fragment();
    }
    let fallback = std::env::var_os(launcher::FALLBACK_ENV).is_some();
    let dirs = app_dirs::app_dirs();
    let first_run = dirs.first_run;
    std::fs::create_dir_all(&dirs.config)?;
    std::fs::create_dir_all(&dirs.cache)?;
    let config_dir = dirs.config;
    let cache_dir = dirs.cache;
    let data_dir = dirs.data;
    let settings = SettingsHandle::load(config_dir.clone());
    if let Some(enabled) = settings.get_bool("startup.enabled") {
        let _ = startup::set_enabled(enabled);
    }
    let theme_api = TuiThemeApi::new(config_dir.join("themes"));
    let library = Arc::new(LibraryHandle::with_paths(Paths {
        config_dir,
        cache_dir,
        data_dir,
    }));
    let (search_tx, search_rx) = std::sync::mpsc::channel();
    // NO_COLOR 是用户级约定。音乐播放器的彩色主题是核心体验，
    // 所以这里选择“忽略但明确告知”，而不是静默强制或退化成全单色。
    let no_color = std::env::var_os("NO_COLOR").is_some();
    if no_color {
        crossterm::style::Colored::set_ansi_color_disabled(false);
    }
    let mut player = TuiAudio::from_env();
    if let Some(np_core::SettingValue::Num(volume)) = settings.get("playback.volume") {
        player.set_volume(volume as f32);
    }
    if let Some(mode) = settings.get_str("playback.mode") {
        player.set_mode_id(&mode);
    }
    if let Some(script) = selftest {
        return run_selftest(&script, &player, &library, &settings, &theme_api);
    }
    let mut app = App::new();
    let (remote_tx, remote_rx) = std::sync::mpsc::channel();
    app.remote_tx = Some(remote_tx);
    if first_run {
        app.status = "首次运行：用 library add <目录> 添加音乐".to_owned();
    }
    app.no_color_ignored = no_color;
    if fallback {
        app.status = "未找到 wt，已用 conhost，封面降级为字符".to_owned();
    } else if let Some(warning) = launcher::font_warning() {
        app.status = warning.to_owned();
    }
    #[cfg(windows)]
    {
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        tray::spawn(tray_tx);
        app.tray_rx = Some(tray_rx);
    }
    let osc_supported = osc::probe();
    osc::flush_input();
    let _osc_guard = osc::guard();
    if let Ok(path) = std::env::var("NP_TUI_OSC_FILE") {
        let _ = std::fs::write(path, if osc_supported { "1" } else { "0" });
    }
    std::panic::set_hook(Box::new(|info| {
        osc::restore();
        eprintln!("np-tui panic: {info}");
    }));
    app.load_theme(
        &theme_api,
        settings.theme_id().as_deref().unwrap_or("milkgreen"),
    );
    app.load_views(&player, &*library, &settings);
    if let Ok(view) = std::env::var("NP_TUI_VIEW") {
        let wanted = match view.trim().to_ascii_lowercase().as_str() {
            "search" => Some(BrowseKind::Search),
            "queue" => Some(BrowseKind::Queue),
            "playlist" | "playlists" => Some(BrowseKind::PlaylistRoot),
            "library" => Some(BrowseKind::Library),
            "settings" => Some(BrowseKind::Settings),
            _ => None,
        };
        if let Some(wanted) = wanted
            && let Some(index) = app.view_kind.iter().position(|kind| *kind == wanted)
        {
            app.view_index = index;
        }
    }
    if let Ok(mode) = std::env::var("NP_TUI_VISUAL") {
        app.visual_mode = match mode.trim().to_ascii_lowercase().as_str() {
            "waveform" | "wave" => VisualMode::Waveform,
            "vectorscope" | "vector" => VisualMode::Vectorscope,
            _ => VisualMode::Spectrum,
        };
    }
    #[cfg(windows)]
    if selftest.is_none()
        && let Ok((cols, rows)) = crossterm::terminal::size()
        && launcher::ensure_default_size(cols, rows)
    {
        return Ok(());
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        SetCursorStyle::SteadyBlock
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    if selftest.is_none() && !run_splash(&mut terminal, &mut app, &settings, &theme_api)? {
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableBracketedPaste
        )?;
        osc::restore();
        return Ok(());
    }
    draw_synced(&mut terminal, &mut app, &player)?;
    if let Ok(path) = std::env::var("NP_TUI_STARTUP_FILE") {
        std::fs::write(path, started.elapsed().as_millis().to_string())?;
    }
    let result = run(
        &mut terminal,
        &mut app,
        &mut player,
        &library,
        SearchContext {
            tx: &search_tx,
            rx: &search_rx,
            play_rx: &remote_rx,
        },
        ThemeContext {
            api: &theme_api,
            settings: &settings,
        },
    );
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    osc::restore();
    terminal.show_cursor()?;
    result?;
    Ok(())
}

mod event_loop;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod snapshot_tests;
