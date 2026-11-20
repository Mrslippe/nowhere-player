//! np-video：Win32 宿主窗口 + mpv `--wid` 嵌入。
//!
//! 进程边界：TUI 通过 stdin 发送一次 `{url, headers, title}` 请求，之后只发
//! `quit`；np-video 通过 stdout 回 `ready` / `closed` / `error ...`。URL 不进
//! 命令行，mpv 的 stdout/stderr 也不继承。

#[cfg(windows)]
mod host;
#[cfg(windows)]
mod layout;
#[cfg(windows)]
mod logging;
#[cfg(windows)]
mod mpv;
#[cfg(windows)]
mod mpv_ipc;
#[cfg(windows)]
mod protocol;

#[cfg(windows)]
fn main() {
    use std::io::{BufRead, BufReader, Write};

    let log_dir = logging::default_dir();
    match logging::init_from_env(&log_dir) {
        Ok(path) => log::info!(target: "video", "start log={}", path.display()),
        Err(error) => eprintln!("np-video 日志初始化失败：{error}"),
    }
    logging::install_panic_hook();

    fn emit(event: protocol::Event) {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        let _ = writeln!(stdout, "{}", event.line());
        let _ = stdout.flush();
    }

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin);
    let request = match protocol::read_request(&mut reader) {
        Ok(request) => request,
        Err(error) => {
            log::error!(target: "video", "bad request: {error}");
            emit(protocol::Event::Error(error));
            std::process::exit(2);
        }
    };
    log::info!(
        target: "video",
        "request title={} volume={} paused={} buffer_ms={} theme={} qualities={} current_qn={:?}",
        request.display_title(),
        request.volume,
        request.paused,
        request.buffer_ms,
        request.theme.is_some(),
        request.qualities.len(),
        request.current_qn
    );

    let (control_tx, control_rx) = std::sync::mpsc::channel();
    // stdin 关闭（TUI 退出）或收到 quit 时，通知窗口线程收尾并杀 mpv。
    let _ = std::thread::Builder::new()
        .name("np-video-control".to_owned())
        .spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = control_tx.send(host::Control::Quit);
                        break;
                    }
                    Ok(_) => {
                        let command = line.trim();
                        if command.eq_ignore_ascii_case("quit")
                            || command.eq_ignore_ascii_case("close")
                        {
                            let _ = control_tx.send(host::Control::Quit);
                            break;
                        } else if let Some(raw) = command.strip_prefix("volume ") {
                            match raw.trim().parse::<u8>() {
                                Ok(volume) if volume <= 100 => {
                                    let _ = control_tx.send(host::Control::Volume(volume));
                                }
                                _ => log::warn!(target: "video", "bad volume command={command}"),
                            }
                        } else if command.eq_ignore_ascii_case("pause") {
                            let _ = control_tx.send(host::Control::Pause(true));
                        } else if command.eq_ignore_ascii_case("resume") {
                            let _ = control_tx.send(host::Control::Pause(false));
                        } else if let Some(raw) = command.strip_prefix("theme ") {
                            // 非法整行拒绝：不发 Control，宿主保持旧主题。
                            match protocol::Theme::parse(raw) {
                                Ok(theme) => {
                                    log::info!(target: "video", "theme command accepted");
                                    log::debug!(target: "video", "theme line: {}", theme.line());
                                    let _ = control_tx.send(host::Control::Theme(theme));
                                }
                                Err(error) => {
                                    log::warn!(target: "video", "bad theme command: {error}")
                                }
                            }
                        } else if let Some(raw) = command.strip_prefix("load ") {
                            // URL 只在内存 / 管道里；任何日志都不记录命令内容。
                            let url = raw.trim();
                            match protocol::validate_stream_url(url) {
                                Ok(()) => {
                                    log::info!(target: "video", "load command accepted");
                                    let _ = control_tx.send(host::Control::Load(url.to_owned()));
                                }
                                Err(_) => {
                                    log::warn!(target: "video", "bad load command");
                                }
                            }
                        } else if !command.is_empty() {
                            log::debug!(target: "video", "ignored command");
                        }
                    }
                    Err(_) => {
                        let _ = control_tx.send(host::Control::Quit);
                        break;
                    }
                }
            }
        });

    let outcome = host::run(request, control_rx);
    let event = match outcome {
        Ok(host::CloseReason::UserClosed | host::CloseReason::TuiQuit) => protocol::Event::Closed,
        Ok(host::CloseReason::MpvExited(Some(0))) => protocol::Event::Closed,
        Ok(host::CloseReason::MpvExited(Some(code))) => {
            protocol::Event::Error(format!("视频播放器已退出（代码 {code}）"))
        }
        Ok(host::CloseReason::MpvExited(None)) => {
            protocol::Event::Error("视频播放器被终止".to_owned())
        }
        Ok(host::CloseReason::LoopError) => protocol::Event::Error("视频窗口异常退出".to_owned()),
        Err(error) => protocol::Event::Error(error),
    };
    let failed = matches!(event, protocol::Event::Error(_));
    emit(event);
    std::process::exit(if failed { 1 } else { 0 });
}

#[cfg(not(windows))]
fn main() {
    println!("error 视频窗口仅支持 Windows");
}
