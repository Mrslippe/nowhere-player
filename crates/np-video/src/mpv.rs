//! mpv 参数与进程启动。URL 只走 stdin（`--playlist=-`），不进命令行。

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::protocol::Request;

/// `NP_VIDEO_PLAYER` 与 T-0104 保持一致：覆盖 mpv 路径或 PATH 命令名。
pub const PLAYER_ENV: &str = "NP_VIDEO_PLAYER";
/// mpv 的 IPC 管道名；单实例宿主，固定名字即可。
pub const IPC_PIPE: &str = r"\\.\pipe\nowhere-video";
/// B站 CDN 对非浏览器 UA 可能直接拒绝，和音频链路保持一致。
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// mpv 参数。URL 不在其中；`--playlist=-` 让 mpv 从 stdin 读 URL。
pub fn mpv_args(hwnd: usize, referer: &str, origin: &str) -> Vec<String> {
    vec![
        format!("--wid={hwnd}"),
        "--no-audio".to_owned(),
        // 视频宽高比优先：窗口比例和视频不一致时，只在一侧留 letterbox。
        "--keepaspect=yes".to_owned(),
        "--no-osc".to_owned(),
        "--no-osd-bar".to_owned(),
        "--force-window=no".to_owned(),
        format!("--input-ipc-server={IPC_PIPE}"),
        format!("--user-agent={BROWSER_UA}"),
        format!("--http-header-fields=Referer: {referer},Origin: {origin},Accept: */*"),
        "--playlist=-".to_owned(),
    ]
}

/// 写到 mpv stdin 的播放列表：一行 URL。
pub fn playlist_payload(url: &str) -> String {
    format!("{url}\n")
}

/// 选择 mpv：`NP_VIDEO_PLAYER` > PATH 里的 mpv。ffplay 不能嵌入，直接报错。
pub fn resolve_mpv(
    override_path: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(raw) = override_path {
        let trimmed = raw.to_string_lossy();
        let trimmed = trimmed.trim();
        if !trimmed.is_empty() {
            let candidate = Path::new(trimmed);
            let looks_like_path = trimmed.contains('/') || trimmed.contains('\\');
            let resolved = if looks_like_path {
                if !candidate.is_file() {
                    return Err(format!("NP_VIDEO_PLAYER 指向的文件不存在：{trimmed}"));
                }
                candidate.to_path_buf()
            } else {
                find_in_path(trimmed, path_var)
                    .ok_or_else(|| format!("NP_VIDEO_PLAYER 指定的播放器不在 PATH 中：{trimmed}"))?
            };
            if is_ffplay(&resolved) {
                return Err("自建视频窗口需要 mpv，ffplay 不支持嵌入".to_owned());
            }
            return Ok(resolved);
        }
    }
    find_in_path("mpv", path_var)
        .ok_or_else(|| "未找到 mpv：请安装 mpv 或设置 NP_VIDEO_PLAYER".to_owned())
}

fn is_ffplay(path: &Path) -> bool {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|stem| stem.contains("ffplay"))
}

/// 在给定 PATH 中查找可执行文件；测试注入临时目录，不碰真实环境。
pub fn find_in_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let path_var = path_var?;
    for directory in std::env::split_paths(path_var) {
        for executable in executable_names(name) {
            let candidate = directory.join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable_names(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = Vec::with_capacity(2);
        if !name.to_ascii_lowercase().ends_with(".exe") {
            names.push(format!("{name}.exe"));
        }
        names.push(name.to_owned());
        names
    }
    #[cfg(not(windows))]
    {
        vec![name.to_owned()]
    }
}

/// 启动 mpv 并把 URL 写进它的 stdin；stdout/stderr 不继承。
pub fn spawn_mpv(program: &Path, hwnd: usize, request: &Request) -> Result<Child, String> {
    request.validate()?;
    let mut command = Command::new(program);
    command
        .args(mpv_args(hwnd, &request.referer, &request.origin))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：mpv 是控制台程序，不要把控制台闪到前台。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("mpv 启动失败：{error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("mpv 的 stdin 不可用".to_owned());
    };
    let payload = playlist_payload(&request.url);
    if let Err(error) = stdin
        .write_all(payload.as_bytes())
        .and_then(|_| stdin.flush())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("mpv 握手失败：{error}"));
    }
    // 关闭 stdin，让 mpv 的播放列表解析读到 EOF。
    drop(stdin);
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::time::{SystemTime, UNIX_EPOCH};

    const REFERER: &str = "https://live.bilibili.com/";
    const ORIGIN: &str = "https://live.bilibili.com";
    const URL: &str = "https://example.invalid/live.flv?token=secret";

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("np-video-mpv-{tag}-{}-{stamp}", std::process::id()));
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

    fn path_var(dir: &Path) -> OsString {
        std::env::join_paths([dir]).expect("join path")
    }

    #[test]
    fn mpv_args_embed_the_window_and_disable_audio_and_osc() {
        let args = mpv_args(42, REFERER, ORIGIN);
        assert_eq!(args[0], "--wid=42");
        assert!(args.iter().any(|arg| arg == "--no-audio"));
        assert!(
            args.iter().any(|arg| arg == "--keepaspect=yes"),
            "mpv must keep the video aspect ratio inside the resized child window"
        );
        assert!(args.iter().any(|arg| arg == "--no-osc"));
        assert!(args.iter().any(|arg| arg == "--no-osd-bar"));
        assert!(args.iter().any(|arg| arg == "--force-window=no"));
        assert!(
            args.iter()
                .any(|arg| arg == r"--input-ipc-server=\\.\pipe\nowhere-video")
        );
        assert!(args.iter().any(|arg| {
            arg == "--http-header-fields=Referer: https://live.bilibili.com/,Origin: https://live.bilibili.com,Accept: */*"
        }));
        assert!(args.iter().any(|arg| arg.starts_with("--user-agent=")));
        assert_eq!(args.last().map(String::as_str), Some("--playlist=-"));
    }

    #[test]
    fn the_url_is_not_in_the_process_arguments() {
        let args = mpv_args(7, REFERER, ORIGIN);
        assert!(
            !args.iter().any(|arg| arg.contains("example.invalid")),
            "URL must travel over stdin, not argv"
        );
        assert_eq!(playlist_payload(URL), format!("{URL}\n"));
    }

    #[test]
    fn resolve_mpv_prefers_override_then_path() {
        let dir = temp_dir("resolve");
        std::fs::write(dir.join(fake_executable("mpv")), b"").expect("mpv");
        let resolved = resolve_mpv(None, Some(&path_var(&dir))).expect("path mpv");
        assert_eq!(resolved, dir.join(fake_executable("mpv")));

        let custom = dir.join(fake_executable("my-mpv"));
        std::fs::write(&custom, b"").expect("custom");
        let resolved =
            resolve_mpv(Some(custom.as_os_str()), Some(&path_var(&dir))).expect("override");
        assert_eq!(resolved, custom);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ffplay_override_is_refused_with_a_chinese_hint() {
        let dir = temp_dir("ffplay");
        let ffplay = dir.join(fake_executable("ffplay"));
        std::fs::write(&ffplay, b"").expect("ffplay");
        let error = resolve_mpv(Some(ffplay.as_os_str()), Some(&path_var(&dir))).unwrap_err();
        assert!(error.contains("mpv") && error.contains("ffplay"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_mpv_returns_a_chinese_install_hint() {
        let dir = temp_dir("missing");
        let error = resolve_mpv(None, Some(&path_var(&dir))).unwrap_err();
        assert!(error.contains("未找到 mpv"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
