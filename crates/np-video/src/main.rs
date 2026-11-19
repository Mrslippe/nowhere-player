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
mod mpv;
#[cfg(windows)]
mod protocol;

#[cfg(windows)]
fn main() {
    use std::io::{BufRead, BufReader, Write};

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
            emit(protocol::Event::Error(error));
            std::process::exit(2);
        }
    };

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
