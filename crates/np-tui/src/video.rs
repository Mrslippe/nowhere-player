//! 直播画面：自建 Win32 宿主窗口（T-0105），mpv `--wid` 嵌入。
//!
//! TUI 只负责取 FLV 视频地址、拉起 `np-video.exe` 子进程并管理生命周期；
//! 窗口的拖动、缩放、DWM 阴影和标题栏全部由 np-video 负责。音频仍走
//! [`TuiAudio`]，所以 mpv 一律 `--no-audio`。
//!
//! 约束：
//! - URL / header 通过 np-video 的 stdin 传入，不进 mpv 命令行；
//! - mpv 的 stdout/stderr 不继承；
//! - 任何时刻最多一个视频进程；
//! - 找不到 np-video.exe 时回退 T-0104 的外部 mpv/ffplay 窗口。
//!
//! 本文件是 `crate::audio` 的子模块（`#[path = "video.rs"]`），因为 `mod video`
//! 的注册点在 `audio.rs`，而任务卡只授权新增 `src/video.rs`。

use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::audio::TuiAudio;

/// B 站直播 CDN 要求的来源头（缺了会 403）。
pub(crate) const VIDEO_REFERER: &str = "https://live.bilibili.com/";
pub(crate) const VIDEO_ORIGIN: &str = "https://live.bilibili.com";
/// ffplay 窗口标题；mpv 使用默认标题。
pub(crate) const VIDEO_WINDOW_TITLE: &str = "NowhereLive";
/// `NP_VIDEO_PLAYER`：外部回退播放器的路径或 PATH 命令名。
pub(crate) const VIDEO_PLAYER_ENV: &str = "NP_VIDEO_PLAYER";
/// `NP_VIDEO_HOST`：覆盖 np-video.exe 路径或 PATH 命令名。
pub(crate) const VIDEO_HOST_ENV: &str = "NP_VIDEO_HOST";
/// 与 np-tui 同目录的宿主进程名。
#[cfg(windows)]
pub(crate) const VIDEO_HOST_EXE: &str = "np-video.exe";
#[cfg(not(windows))]
pub(crate) const VIDEO_HOST_EXE: &str = "np-video";
/// 状态栏一次性提示的停留时间。
pub(crate) const VIDEO_NOTE_TTL: Duration = Duration::from_secs(4);
/// 关闭宿主时先礼后兵的最长等待。
const HOST_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(200);

/// 支持的外部播放器；两者参数不同但都是“只出画面、不出声音”。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoPlayerKind {
    Mpv,
    Ffplay,
}

impl VideoPlayerKind {
    /// PATH 中查找的命令名（Windows 自动补 `.exe`）。
    pub(crate) fn program_name(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::Ffplay => "ffplay",
        }
    }

    fn title(self) -> &'static str {
        self.program_name()
    }
}

/// 外部回退播放器：类型决定参数，路径决定实际启动的可执行文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoProgram {
    pub(crate) kind: VideoPlayerKind,
    pub(crate) path: PathBuf,
}

/// 一次视频启动选择：优先自建宿主，缺失时回退外部播放器。
#[derive(Debug, Clone, PartialEq, Eq)]
enum VideoBackend {
    Host { path: PathBuf },
    External(VideoProgram),
}

/// 选中后的播放器：`NP_VIDEO_PLAYER` 覆盖 > PATH 里的 mpv > PATH 里的 ffplay。
pub(crate) fn resolve_program(
    override_path: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Result<VideoProgram, String> {
    if let Some(raw) = override_path {
        let trimmed = raw.to_string_lossy();
        let trimmed = trimmed.trim();
        if !trimmed.is_empty() {
            let candidate = Path::new(trimmed);
            let looks_like_path = trimmed.contains('/') || trimmed.contains('\\');
            if looks_like_path || candidate.is_file() {
                if !candidate.is_file() {
                    return Err(format!("NP_VIDEO_PLAYER 指向的文件不存在：{trimmed}"));
                }
                return Ok(VideoProgram {
                    kind: kind_for_path(candidate),
                    path: candidate.to_path_buf(),
                });
            }
            return match find_in_path(trimmed, path_var) {
                Some(path) => Ok(VideoProgram {
                    kind: kind_for_path(&path),
                    path,
                }),
                None => Err(format!(
                    "NP_VIDEO_PLAYER 指定的播放器不在 PATH 中：{trimmed}"
                )),
            };
        }
    }
    for kind in [VideoPlayerKind::Mpv, VideoPlayerKind::Ffplay] {
        if let Some(path) = find_in_path(kind.program_name(), path_var) {
            return Ok(VideoProgram { kind, path });
        }
    }
    Err("未找到视频播放器：请安装 mpv 或 ffplay（或用 NP_VIDEO_PLAYER 指定）".to_owned())
}

/// 在给定的 PATH 中查找可执行文件；测试注入临时目录，不碰真实环境。
pub(crate) fn find_in_path(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
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

fn kind_for_path(path: &Path) -> VideoPlayerKind {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if stem.contains("ffplay") {
        VideoPlayerKind::Ffplay
    } else {
        VideoPlayerKind::Mpv
    }
}

/// 外部回退播放器参数。所有参数集中在这里，单测直接比对，不启动进程。
pub(crate) fn player_args(kind: VideoPlayerKind, url: &str) -> Vec<String> {
    let mut args = Vec::new();
    match kind {
        VideoPlayerKind::Mpv => {
            args.push("--no-audio".to_owned());
            args.push("--force-window=yes".to_owned());
            args.push("--really-quiet".to_owned());
            args.push(format!(
                "--http-header-fields=Referer: {VIDEO_REFERER},Origin: {VIDEO_ORIGIN}"
            ));
        }
        VideoPlayerKind::Ffplay => {
            args.push("-an".to_owned());
            args.push("-loglevel".to_owned());
            args.push("error".to_owned());
            args.push("-window_title".to_owned());
            args.push(VIDEO_WINDOW_TITLE.to_owned());
            args.push("-headers".to_owned());
            // libavformat 把该字符串直接拼进请求头，两条之间用 CRLF 分隔。
            args.push(format!(
                "Referer: {VIDEO_REFERER}\\r\\nOrigin: {VIDEO_ORIGIN}"
            ));
        }
    }
    args.push(url.to_owned());
    args
}

/// 构造外部播放器命令：stdio 全部丢弃，Windows 下不弹控制台窗口。
pub(crate) fn player_command(program: &VideoProgram, url: &str) -> Command {
    let mut command = Command::new(&program.path);
    command.args(player_args(program.kind, url));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：ffplay 是控制台程序，不要把它的控制台闪到前台。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 解析宿主可执行文件：显式覆盖 > np-tui 同目录 > PATH。
pub(crate) fn resolve_host(
    override_path: Option<&OsStr>,
    exe_dir: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Result<Option<PathBuf>, String> {
    if let Some(raw) = override_path {
        let trimmed = raw.to_string_lossy();
        let trimmed = trimmed.trim();
        if !trimmed.is_empty() {
            let candidate = Path::new(trimmed);
            let looks_like_path = trimmed.contains('/') || trimmed.contains('\\');
            if looks_like_path || candidate.is_file() {
                if !candidate.is_file() {
                    return Err(format!("NP_VIDEO_HOST 指向的文件不存在：{trimmed}"));
                }
                return Ok(Some(candidate.to_path_buf()));
            }
            return match find_in_path(trimmed, path_var) {
                Some(path) => Ok(Some(path)),
                None => Err(format!("NP_VIDEO_HOST 指定的命令不在 PATH 中：{trimmed}")),
            };
        }
    }
    if let Some(directory) = exe_dir {
        let candidate = directory.join(VIDEO_HOST_EXE);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(find_in_path("np-video", path_var))
}

/// 宿主优先；没有宿主（或非 Windows）回退 T-0104 的外部播放器。
fn resolve_backend(
    host_override: Option<&OsStr>,
    player_override: Option<&OsStr>,
    exe_dir: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Result<VideoBackend, String> {
    #[cfg(windows)]
    {
        if let Some(path) = resolve_host(host_override, exe_dir, path_var)? {
            match resolve_program(player_override, path_var) {
                // 自建宿主内部固定跑 mpv；ffplay 只能走外部窗口。
                Ok(program) if program.kind == VideoPlayerKind::Mpv => {
                    return Ok(VideoBackend::Host { path });
                }
                Ok(_) => {}
                // 没有外部播放器也不要直接报错：让 np-video 给出 mpv 专属提示。
                Err(_) => return Ok(VideoBackend::Host { path }),
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (host_override, exe_dir);
    }
    resolve_program(player_override, path_var).map(VideoBackend::External)
}

/// 取流结果：地址 + 房间标题（标题只用于窗口 / 标题栏，不进 URL）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoTarget {
    url: String,
    title: String,
}

fn sanitize_title(title: &str) -> String {
    title
        .chars()
        .map(|character| match character {
            '\r' | '\n' | '\t' | '\0' => ' ',
            other => other,
        })
        .take(120)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// 行协议：v1 + 四个字段 + end。标题里的换行会被压成空格。
fn host_request(target: &VideoTarget) -> Result<String, String> {
    let url = target.url.trim();
    if url.is_empty()
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(['\r', '\n', '\0', ' ', '#'])
    {
        return Err("视频地址格式异常".to_owned());
    }
    let title = sanitize_title(&target.title);
    Ok(format!(
        "v1\nurl {url}\nreferer {VIDEO_REFERER}\norigin {VIDEO_ORIGIN}\ntitle {title}\nend\n"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostEvent {
    Ready,
    Closed,
    Error(String),
}

impl HostEvent {
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\r', '\n']);
        match line {
            "ready" => Some(Self::Ready),
            "closed" => Some(Self::Closed),
            _ => line
                .strip_prefix("error ")
                .map(|message| Self::Error(message.trim().to_owned())),
        }
    }
}

struct HostChild {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<HostEvent>,
}

impl HostChild {
    fn drain_events(&self) -> Vec<HostEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }

    fn try_wait(&mut self) -> Option<ExitStatus> {
        match self.child.as_mut()?.try_wait() {
            Ok(Some(status)) => Some(status),
            _ => None,
        }
    }

    /// 先发 quit 让 np-video 自己杀 mpv 并退出；超时才强杀。
    fn shutdown(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"quit\n");
            let _ = stdin.flush();
            drop(stdin);
        }
        if self.wait_for(HOST_SHUTDOWN_TIMEOUT) {
            self.child = None;
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn wait_for(&mut self, timeout: Duration) -> bool {
        let started = Instant::now();
        loop {
            match self.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(_)) => return true,
                    Ok(None) => {}
                    Err(_) => return false,
                },
                None => return true,
            }
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// 拉取视频地址与房间标题；错误文本不含 URL / Cookie。
type VideoFetch = (u64, Result<VideoTarget, String>);

fn host_command(path: &Path) -> Command {
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn spawn_host(path: &Path, target: &VideoTarget) -> Result<HostChild, String> {
    let request = host_request(target)?;
    let mut child = host_command(path)
        .spawn()
        .map_err(|error| format!("np-video 启动失败：{error}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("np-video 的 stdin 不可用".to_owned());
    };
    if let Err(error) = stdin
        .write_all(request.as_bytes())
        .and_then(|_| stdin.flush())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("np-video 握手失败：{error}"));
    }
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("np-video 的 stdout 不可用".to_owned());
    };

    let (tx, events) = mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("np-video-events".to_owned())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Some(event) = HostEvent::parse(&line)
                    && tx.send(event).is_err()
                {
                    break;
                }
            }
        });
    if reader.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("np-video 事件线程启动失败".to_owned());
    }

    Ok(HostChild {
        child: Some(child),
        stdin: Some(stdin),
        events,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoPhase {
    Idle,
    Preparing,
    Starting,
    Playing,
}

struct VideoNote {
    text: String,
    at: Instant,
}

impl VideoNote {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            at: Instant::now(),
        }
    }

    fn is_fresh(&self) -> bool {
        self.at.elapsed() < VIDEO_NOTE_TTL
    }
}

struct VideoState {
    phase: VideoPhase,
    generation: u64,
    backend: Option<VideoBackend>,
    external_child: Option<Child>,
    host: Option<HostChild>,
    fetch_rx: Option<mpsc::Receiver<VideoFetch>>,
    note: Option<VideoNote>,
}

/// 直播画面控制器：放在 [`TuiAudio`] 里，跟随停播 / 换房 / 退出一起清理。
pub(crate) struct VideoController {
    inner: RefCell<VideoState>,
}

impl Default for VideoController {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoController {
    pub(crate) fn new() -> Self {
        Self {
            inner: RefCell::new(VideoState {
                phase: VideoPhase::Idle,
                generation: 0,
                backend: None,
                external_child: None,
                host: None,
                fetch_rx: None,
                note: None,
            }),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.borrow().phase != VideoPhase::Idle
    }

    /// 启动一次取流；URL 到手后在 [`Self::poll`] 里才真正拉起宿主 / 播放器。
    fn begin(&self, backend: VideoBackend, room_id: u64, auth: Option<np_live::BiliAuth>) {
        let mut state = self.inner.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.phase = VideoPhase::Preparing;
        state.backend = Some(backend);
        state.note = None;
        state.external_child = None;
        state.host = None;
        let (tx, rx) = mpsc::channel();
        state.fetch_rx = Some(rx);
        drop(state);

        let spawn = std::thread::Builder::new()
            .name("np-tui-video".to_owned())
            .spawn(move || {
                let result = fetch_video_target(room_id, auth);
                let _ = tx.send((generation, result));
            });
        if let Err(error) = spawn {
            let mut state = self.inner.borrow_mut();
            state.phase = VideoPhase::Idle;
            state.backend = None;
            state.fetch_rx = None;
            state.note = Some(VideoNote::new(format!("视频准备线程启动失败：{error}")));
        }
    }

    /// 主线程每 tick 调用：接住取流结果、宿主事件、子进程退出。
    fn poll(&self) -> bool {
        let mut state = self.inner.borrow_mut();
        let mut changed = false;

        let received = match state.fetch_rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(value) => Some(value),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some((state.generation, Err("视频取流线程意外结束".to_owned())))
                }
            },
            None => None,
        };
        if let Some((generation, result)) = received
            && generation == state.generation
            && state.phase == VideoPhase::Preparing
        {
            state.fetch_rx = None;
            match result {
                Ok(target) => match state.backend.clone() {
                    Some(VideoBackend::Host { path }) => match spawn_host(&path, &target) {
                        Ok(host) => {
                            state.host = Some(host);
                            state.phase = VideoPhase::Starting;
                        }
                        Err(error) => {
                            state.phase = VideoPhase::Idle;
                            state.backend = None;
                            state.note = Some(VideoNote::new(format!("视频窗口启动失败：{error}")));
                        }
                    },
                    Some(VideoBackend::External(program)) => {
                        match player_command(&program, &target.url).spawn() {
                            Ok(child) => {
                                state.external_child = Some(child);
                                state.phase = VideoPhase::Playing;
                                state.note = Some(VideoNote::new(format!(
                                    "视频已打开（{}，v 关闭）",
                                    program.kind.title()
                                )));
                            }
                            Err(error) => {
                                state.phase = VideoPhase::Idle;
                                state.backend = None;
                                state.note =
                                    Some(VideoNote::new(format!("视频播放器启动失败：{error}")));
                            }
                        }
                    }
                    None => {
                        state.phase = VideoPhase::Idle;
                        state.note = Some(VideoNote::new("视频播放器丢失，请重新打开"));
                    }
                },
                Err(message) => {
                    state.phase = VideoPhase::Idle;
                    state.backend = None;
                    state.note = Some(VideoNote::new(format!("视频取流失败：{message}")));
                }
            }
            changed = true;
        }

        // 宿主事件：ready / closed / error。取走宿主时先收尾，避免借冲突。
        let events = state
            .host
            .as_ref()
            .map(HostChild::drain_events)
            .unwrap_or_default();
        for event in events {
            match event {
                HostEvent::Ready => {
                    if state.phase == VideoPhase::Starting {
                        state.phase = VideoPhase::Playing;
                        state.note = Some(VideoNote::new("视频已打开（v 关闭）"));
                        changed = true;
                    }
                }
                HostEvent::Closed => {
                    if let Some(mut host) = state.host.take() {
                        host.shutdown();
                    }
                    state.phase = VideoPhase::Idle;
                    state.backend = None;
                    state.note = Some(VideoNote::new("视频窗口已关闭"));
                    changed = true;
                }
                HostEvent::Error(message) => {
                    let was_starting = state.phase == VideoPhase::Starting;
                    if let Some(mut host) = state.host.take() {
                        host.shutdown();
                    }
                    state.phase = VideoPhase::Idle;
                    state.backend = None;
                    state.note = Some(VideoNote::new(if was_starting {
                        format!("视频启动失败：{message}")
                    } else {
                        format!("视频播放失败：{message}")
                    }));
                    changed = true;
                }
            }
        }

        // 宿主进程退出（用户关窗、mpv 退出或崩溃）：即使没读到事件也要回到 Idle。
        if state.host.is_some() {
            let outcome = state.host.as_mut().and_then(HostChild::try_wait);
            if let Some(status) = outcome {
                let was_starting = state.phase == VideoPhase::Starting;
                if let Some(mut host) = state.host.take() {
                    host.shutdown();
                }
                state.phase = VideoPhase::Idle;
                state.backend = None;
                state.note = Some(VideoNote::new(if was_starting {
                    format!("视频窗口启动失败（退出码 {}）", exit_code(status))
                } else {
                    host_exit_note(status)
                }));
                changed = true;
            }
        }

        // 外部回退播放器退出。
        if state.phase == VideoPhase::Playing && state.external_child.is_some() {
            let outcome = match state.external_child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(Ok(status)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error.to_string())),
                },
                None => None,
            };
            if let Some(outcome) = outcome {
                state.external_child = None;
                state.backend = None;
                state.phase = VideoPhase::Idle;
                state.note = Some(match outcome {
                    Ok(status) => VideoNote::new(exit_note(status)),
                    Err(error) => VideoNote::new(format!("视频进程状态读取失败：{error}")),
                });
                changed = true;
            }
        }
        changed
    }

    /// 关闭窗口并回收子进程；返回之前是否有视频在准备/播放。
    fn close(&self) -> bool {
        let mut state = self.inner.borrow_mut();
        let was_active = state.phase != VideoPhase::Idle
            || state.host.is_some()
            || state.external_child.is_some();
        state.generation = state.generation.wrapping_add(1);
        state.fetch_rx = None;
        state.backend = None;
        if let Some(mut host) = state.host.take() {
            host.shutdown();
        }
        if let Some(mut child) = state.external_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.phase = VideoPhase::Idle;
        if was_active {
            state.note = Some(VideoNote::new("视频已关闭"));
        }
        was_active
    }

    /// 状态栏文本：一次性提示优先，其次是常驻的“准备中 / 启动中 / 已打开”。
    fn hint(&self) -> Option<String> {
        let state = self.inner.borrow();
        if let Some(note) = &state.note
            && note.is_fresh()
        {
            return Some(note.text.clone());
        }
        match state.phase {
            VideoPhase::Idle => None,
            VideoPhase::Preparing => Some("视频准备中…".to_owned()),
            VideoPhase::Starting => Some("视频启动中…".to_owned()),
            VideoPhase::Playing => Some("视频已打开（v 关闭）".to_owned()),
        }
    }
}

impl Drop for VideoController {
    fn drop(&mut self) {
        // 进程退出（q / 托盘 / panic）时不能把播放器窗口留在桌面上。
        self.close();
    }
}

fn exit_code(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "终止".to_owned(), |code| code.to_string())
}

fn host_exit_note(status: ExitStatus) -> String {
    match status.code() {
        Some(0) => "视频窗口已关闭".to_owned(),
        Some(code) => format!("视频窗口异常退出（代码 {code}）"),
        None => "视频窗口被终止".to_owned(),
    }
}

fn exit_note(status: ExitStatus) -> String {
    match status.code() {
        Some(0) => "视频窗口已关闭".to_owned(),
        Some(code) => format!("视频窗口已关闭（退出码 {code}）"),
        None => "视频窗口已关闭（被终止）".to_owned(),
    }
}

/// 后台线程里取视频 FLV 地址和房间标题；错误文本不含 URL / Cookie。
fn fetch_video_target(
    room_id: u64,
    auth: Option<np_live::BiliAuth>,
) -> Result<VideoTarget, String> {
    let transport = np_sources::ReqwestTransport::new()
        .map_err(|error| format!("视频网络初始化失败：{error}"))?;
    let info = np_live::api::fetch_video_play_info_with_auth(&transport, room_id, auth.as_ref())
        .map_err(|error| error.user_message())?;
    // 房间标题只用于标题栏；拉不到就回退“直播 <room>”，不影响播放。
    let title = np_live::api::fetch_room_info(&transport, room_id)
        .ok()
        .map(|room| room.title);
    target_from_parts(room_id, info, title)
}

/// 把 playurl 结果 + 房间标题收敛成一次启动目标；错误保持中文，不暴露 URL。
fn target_from_parts(
    room_id: u64,
    info: np_live::api::PlayInfo,
    room_title: Option<String>,
) -> Result<VideoTarget, String> {
    let url = video_url_from_play_info(info)?;
    if url.is_empty()
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(['\r', '\n', '\0', ' ', '#'])
    {
        return Err("视频地址格式异常".to_owned());
    }
    let title = room_title
        .map(|title| sanitize_title(&title))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("直播 {room_id}"));
    Ok(VideoTarget { url, title })
}

/// 把 playurl 结果收敛成一条 FLV 地址；错误保持中文，不暴露 URL。
fn video_url_from_play_info(info: np_live::api::PlayInfo) -> Result<String, String> {
    if !info.is_live() {
        return Err("直播间未开播".to_owned());
    }
    info.flv_url
        .ok_or_else(|| "直播间没有可用的 FLV 视频流".to_owned())
}

impl TuiAudio {
    /// 当前是否有视频在准备或播放。
    pub(crate) fn video_is_active(&self) -> bool {
        self.video.is_active()
    }

    /// `v`：打开 / 关闭视频窗口。
    pub(crate) fn toggle_video(&self) -> Result<String, String> {
        if self.video.is_active() {
            self.video.close();
            return Ok("视频已关闭".to_owned());
        }
        if !self.is_live() || self.live_room.get() == 0 {
            return Err("当前没有直播".to_owned());
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let backend = resolve_backend(
            std::env::var_os(VIDEO_HOST_ENV).as_deref(),
            std::env::var_os(VIDEO_PLAYER_ENV).as_deref(),
            exe_dir.as_deref(),
            std::env::var_os("PATH").as_deref(),
        )?;
        let auth = self.auth_for_live();
        self.video.begin(backend, self.live_room.get(), auth);
        Ok("视频准备中…".to_owned())
    }

    /// 停止直播 / 换房 / F5 / 退出时关闭视频；返回是否有进程被清理。
    pub(crate) fn close_video(&self) -> bool {
        self.video.close()
    }

    /// 主循环 tick：回收播放器 / 宿主进程、接住取流结果和宿主事件。
    pub(crate) fn poll_video(&self) -> bool {
        self.video.poll()
    }

    /// Live 页状态栏提示；无视频且无提示时返回 `None`，保证旧 golden 不变。
    pub(crate) fn video_status_hint(&self) -> Option<String> {
        self.video.hint()
    }
}

#[cfg(test)]
impl VideoController {
    fn attach_child_for_test(&self, child: Child) {
        let mut state = self.inner.borrow_mut();
        state.phase = VideoPhase::Playing;
        state.backend = Some(VideoBackend::External(VideoProgram {
            kind: VideoPlayerKind::Ffplay,
            path: PathBuf::from("fake-player"),
        }));
        state.external_child = Some(child);
    }

    fn mark_active_for_test(&self) {
        let mut state = self.inner.borrow_mut();
        state.phase = VideoPhase::Playing;
        state.backend = Some(VideoBackend::External(VideoProgram {
            kind: VideoPlayerKind::Ffplay,
            path: PathBuf::from("fake-player"),
        }));
    }

    fn set_note_for_test(&self, text: &str) {
        self.inner.borrow_mut().note = Some(VideoNote::new(text));
    }

    /// 模拟后台取流失败，让 `poll` 走进错误分支。
    fn inject_fetch_error_for_test(&self, message: &str) {
        let mut state = self.inner.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        state.phase = VideoPhase::Preparing;
        state.backend = Some(VideoBackend::External(VideoProgram {
            kind: VideoPlayerKind::Ffplay,
            path: PathBuf::from("fake-player"),
        }));
        let (tx, rx) = mpsc::channel();
        state.fetch_rx = Some(rx);
        tx.send((state.generation, Err(message.to_owned())))
            .expect("inject fetch error");
    }

    /// 测试用：挂一个假的宿主子进程（可无进程），返回事件发送端。
    fn attach_host_for_test(&self) -> mpsc::Sender<HostEvent> {
        let (tx, events) = mpsc::channel();
        self.attach_host_with_events_for_test(events);
        tx
    }

    fn attach_host_with_events_for_test(&self, events: mpsc::Receiver<HostEvent>) {
        let mut state = self.inner.borrow_mut();
        state.phase = VideoPhase::Starting;
        state.backend = Some(VideoBackend::Host {
            path: PathBuf::from("fake-host"),
        });
        state.host = Some(HostChild {
            child: None,
            stdin: None,
            events,
        });
    }
}

#[cfg(test)]
impl TuiAudio {
    /// 测试用：标记视频正在播放，不启动任何进程。
    pub(crate) fn video_active_for_test(&self) {
        self.video.mark_active_for_test();
    }

    /// 测试用：写入状态栏提示，不启动任何进程。
    pub(crate) fn video_note_for_test(&self, text: &str) {
        self.video.set_note_for_test(text);
    }

    /// 测试用：挂一个假的宿主，返回事件发送端。
    fn video_host_for_test(&self) -> mpsc::Sender<HostEvent> {
        self.video.attach_host_for_test()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn mpv_args_disable_audio_and_carry_live_headers() {
        let args = player_args(VideoPlayerKind::Mpv, "https://example.invalid/live.flv");
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
        let args = player_args(VideoPlayerKind::Ffplay, "https://example.invalid/live.flv");
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
        let program = VideoProgram {
            kind: VideoPlayerKind::Mpv,
            path: PathBuf::from(fake_executable("mpv")),
        };
        let command = player_command(&program, "https://example.invalid/live.flv");
        assert_eq!(command.get_program(), program.path.as_os_str());
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            player_args(VideoPlayerKind::Mpv, "https://example.invalid/live.flv")
        );
    }

    #[test]
    fn mpv_is_preferred_then_ffplay_falls_back() {
        let dir = temp_dir("path-fallback");
        std::fs::write(dir.join(fake_executable("ffplay")), b"").expect("ffplay");
        let program = resolve_program(None, Some(&path_var(&dir))).expect("ffplay fallback");
        assert_eq!(program.kind, VideoPlayerKind::Ffplay);
        assert_eq!(program.path, dir.join(fake_executable("ffplay")));

        std::fs::write(dir.join(fake_executable("mpv")), b"").expect("mpv");
        let program = resolve_program(None, Some(&path_var(&dir))).expect("mpv preferred");
        assert_eq!(program.kind, VideoPlayerKind::Mpv);
        assert_eq!(program.path, dir.join(fake_executable("mpv")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_player_overrides_path_lookup() {
        let dir = temp_dir("override");
        let custom = dir.join(fake_executable("my-ffplay"));
        std::fs::write(&custom, b"").expect("custom player");
        let program =
            resolve_program(Some(custom.as_os_str()), Some(&path_var(&dir))).expect("override");
        assert_eq!(program.kind, VideoPlayerKind::Ffplay);
        assert_eq!(program.path, custom);

        // 纯命令名走 PATH，而不是当前目录。
        let name = fake_executable("my-mpv");
        std::fs::write(dir.join(&name), b"").expect("named player");
        let program =
            resolve_program(Some(OsStr::new(&name)), Some(&path_var(&dir))).expect("path name");
        assert_eq!(program.kind, VideoPlayerKind::Mpv);
        assert_eq!(program.path, dir.join(&name));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_player_returns_a_chinese_install_hint() {
        let dir = temp_dir("missing");
        let error = resolve_program(None, Some(&path_var(&dir))).unwrap_err();
        assert!(error.contains("未找到视频播放器"), "{error}");
        assert!(error.contains("mpv") && error.contains("ffplay"), "{error}");

        let missing = dir.join(fake_executable("nope"));
        let error = resolve_program(Some(missing.as_os_str()), Some(&path_var(&dir))).unwrap_err();
        assert!(error.contains("NP_VIDEO_PLAYER"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn host_resolution_prefers_env_then_sibling_then_path() {
        let dir = temp_dir("hosts");
        let exe_dir = dir.join("bin");
        std::fs::create_dir_all(&exe_dir).expect("bin dir");
        let sibling = exe_dir.join(VIDEO_HOST_EXE);
        std::fs::write(&sibling, b"").expect("sibling host");
        assert_eq!(
            resolve_host(None, Some(&exe_dir), Some(&path_var(&dir))).expect("sibling"),
            Some(sibling.clone())
        );

        let override_path = dir.join(fake_executable("custom-video"));
        std::fs::write(&override_path, b"").expect("override host");
        assert_eq!(
            resolve_host(
                Some(override_path.as_os_str()),
                Some(&exe_dir),
                Some(&path_var(&dir))
            )
            .expect("override"),
            Some(override_path)
        );

        let missing = dir.join(fake_executable("nope"));
        let error = resolve_host(
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
            resolve_host(None, Some(&exe_dir), Some(&path_var(&dir))).expect("path"),
            Some(path_host)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backend_prefers_host_and_falls_back_to_external() {
        let dir = temp_dir("backend");
        std::fs::write(dir.join(fake_executable("mpv")), b"").expect("mpv");
        let path = path_var(&dir);
        let backend = resolve_backend(None, None, Some(&dir), Some(&path)).expect("external");
        assert!(matches!(backend, VideoBackend::External(_)));

        #[cfg(windows)]
        {
            std::fs::write(dir.join(VIDEO_HOST_EXE), b"").expect("host");
            let backend = resolve_backend(None, None, Some(&dir), Some(&path)).expect("host");
            assert!(matches!(backend, VideoBackend::Host { .. }));

            // 只有 ffplay 时宿主无法嵌入，必须回退外部窗口。
            std::fs::remove_file(dir.join(fake_executable("mpv"))).expect("remove mpv");
            std::fs::write(dir.join(fake_executable("ffplay")), b"").expect("ffplay");
            let backend =
                resolve_backend(None, None, Some(&dir), Some(&path)).expect("ffplay fallback");
            assert!(matches!(
                backend,
                VideoBackend::External(VideoProgram {
                    kind: VideoPlayerKind::Ffplay,
                    ..
                })
            ));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn host_request_is_line_based_and_sanitizes_the_title() {
        let target = VideoTarget {
            url: "https://example.invalid/live.flv?token=secret".to_owned(),
            title: "坏\n标题".to_owned(),
        };
        let request = host_request(&target).expect("request");
        let lines: Vec<&str> = request.lines().collect();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0], "v1");
        assert!(lines[1].starts_with("url https://example.invalid/live.flv"));
        assert_eq!(lines[4], "title 坏 标题");
        assert_eq!(lines[5], "end");
        assert!(!request.contains("\n\n"));

        let bad = VideoTarget {
            url: "https://example.invalid/live.flv\nX".to_owned(),
            title: String::new(),
        };
        assert!(host_request(&bad).unwrap_err().contains("地址"));
    }

    #[test]
    fn target_uses_room_title_or_live_fallback() {
        let target = target_from_parts(21622811, live_info(), Some("BLAST 直播".to_owned()))
            .expect("target");
        assert_eq!(target.title, "BLAST 直播");
        assert!(target.url.starts_with("http"));

        let target = target_from_parts(21622811, live_info(), Some("  ".to_owned()))
            .expect("fallback title");
        assert_eq!(target.title, "直播 21622811");

        let offline = np_live::api::parse_play_info(
            include_bytes!("../../np-live/fixtures/offline_playinfo.json"),
            510,
        )
        .expect("offline fixture");
        assert_eq!(
            target_from_parts(510, offline, None).unwrap_err(),
            "直播间未开播"
        );
    }

    #[test]
    fn host_events_parse_ready_closed_and_error() {
        assert_eq!(HostEvent::parse("ready\r\n"), Some(HostEvent::Ready));
        assert_eq!(HostEvent::parse("closed"), Some(HostEvent::Closed));
        assert_eq!(
            HostEvent::parse("error 播放器退出"),
            Some(HostEvent::Error("播放器退出".to_owned()))
        );
        assert_eq!(HostEvent::parse("unknown"), None);
    }

    fn exit_child() -> Child {
        #[cfg(windows)]
        {
            let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into());
            Command::new(shell)
                .args(["/C", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("fake exit process")
        }
        #[cfg(not(windows))]
        {
            Command::new("sh")
                .args(["-c", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("fake exit process")
        }
    }

    #[test]
    fn playing_child_exit_is_reaped_and_reported() {
        let controller = VideoController::new();
        controller.attach_child_for_test(exit_child());
        assert!(controller.is_active());

        let mut seen = false;
        for _ in 0..200 {
            if controller.poll() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(seen, "poll should reap the exited fake process");
        assert!(!controller.is_active());
        let hint = controller.hint().expect("exit hint");
        assert!(hint.contains("视频窗口已关闭"), "{hint}");
    }

    #[test]
    fn close_is_idempotent_and_marks_video_closed() {
        let controller = VideoController::new();
        controller.attach_child_for_test(exit_child());
        assert!(controller.close());
        assert!(!controller.is_active());
        assert_eq!(controller.hint().as_deref(), Some("视频已关闭"));
        assert!(!controller.close());
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
    fn host_ready_moves_to_playing_and_closed_returns_to_idle() {
        let controller = VideoController::new();
        let events = controller.attach_host_for_test();
        events.send(HostEvent::Ready).expect("ready");
        assert!(controller.poll());
        assert_eq!(controller.hint().as_deref(), Some("视频已打开（v 关闭）"));

        events.send(HostEvent::Closed).expect("closed");
        assert!(controller.poll());
        assert!(!controller.is_active());
        assert_eq!(controller.hint().as_deref(), Some("视频窗口已关闭"));
    }

    #[test]
    fn host_error_returns_to_idle_with_a_chinese_note() {
        let controller = VideoController::new();
        let events = controller.attach_host_for_test();
        events
            .send(HostEvent::Error("mpv 启动失败".to_owned()))
            .expect("error");
        assert!(controller.poll());
        assert!(!controller.is_active());
        let hint = controller.hint().expect("error hint");
        assert!(hint.contains("视频启动失败"), "{hint}");
        assert!(hint.contains("mpv 启动失败"), "{hint}");
    }

    #[test]
    fn host_process_exit_without_an_event_is_reported() {
        let controller = VideoController::new();
        let (_tx, events) = mpsc::channel();
        controller.attach_host_with_events_for_test(events);
        // 没有 child 时 attach 立即视为已退出；换成一个真实退出的子进程。
        {
            let mut state = controller.inner.borrow_mut();
            if let Some(host) = state.host.as_mut() {
                host.child = Some(exit_child());
            }
        }
        let mut seen = false;
        for _ in 0..200 {
            if controller.poll() {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(seen);
        assert!(!controller.is_active());
        let hint = controller.hint().expect("exit hint");
        assert!(hint.contains("启动失败") || hint.contains("窗口"), "{hint}");
    }

    #[test]
    fn toggle_while_a_host_is_active_closes_it() {
        let audio = TuiAudio::empty();
        let _events = audio.video_host_for_test();
        assert!(audio.video_is_active());
        assert_eq!(audio.toggle_video(), Ok("视频已关闭".to_owned()));
        assert!(!audio.video_is_active());
    }

    #[test]
    fn fetch_error_moves_back_to_idle_with_a_status_hint() {
        let controller = VideoController::new();
        controller.inject_fetch_error_for_test("模拟网络断开");
        assert!(controller.is_active(), "preparing counts as active");
        assert!(controller.poll());
        assert!(!controller.is_active());
        let hint = controller.hint().expect("fetch failure hint");
        assert!(hint.contains("视频取流失败"), "{hint}");
        assert!(hint.contains("模拟网络断开"), "{hint}");
    }

    #[test]
    fn notes_take_priority_over_phase_hints() {
        let controller = VideoController::new();
        controller.set_note_for_test("视频准备中…");
        assert_eq!(controller.hint().as_deref(), Some("视频准备中…"));
        controller.mark_active_for_test();
        assert_eq!(
            controller.hint().as_deref(),
            Some("视频准备中…"),
            "fresh note must win over the persistent phase hint"
        );
    }
}
