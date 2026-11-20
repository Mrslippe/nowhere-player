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
use crate::theme::{Palette, VideoTheme};

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
// 非 Windows 只用外部播放器；保留常量给测试与共享路径，避免平台分叉。
#[cfg(not(windows))]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const VIDEO_HOST_EXE: &str = "np-video";
/// 状态栏一次性提示的停留时间。
pub(crate) const VIDEO_NOTE_TTL: Duration = Duration::from_secs(4);
/// 默认请求的清晰度：最高常规档 4K（20000）；服务端按权限回退到 2K/原画。
pub(crate) const DEFAULT_VIDEO_QN: u64 = 20_000;
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
pub(super) enum VideoBackend {
    // 宿主只在 Windows 构造；非 Windows 保留变体供共享匹配与测试。
    #[cfg_attr(not(windows), allow(dead_code))]
    Host {
        path: PathBuf,
    },
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
///
/// 非 Windows 不启动宿主，但测试与共享解析路径仍保留这份实现。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
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
pub(super) fn resolve_backend(
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

/// 取流结果：地址 + 房间标题 + 清晰度菜单（标题只用于窗口 / 标题栏，不进 URL）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VideoTarget {
    pub(super) url: String,
    pub(super) title: String,
    pub(super) quality_note: Option<String>,
    /// `accept_qn` 并集生成的菜单项：降序，最高档在最前。
    pub(super) qualities: Vec<(u64, String)>,
    /// 所选流的 codec（`avc` / `hevc`），只用于日志。
    pub(super) codec: Option<String>,
    /// 服务端实际返回的档位（可能低于请求 qn）。
    pub(super) current_qn: Option<u64>,
    /// 本次请求的 qn；刷新时沿用它，避免服务端降档后越刷越低。
    pub(super) requested_qn: u64,
}

/// B 站 qn 到中文档位的映射；未知档位显示 `qn <n>`。
fn quality_label(qn: u64) -> String {
    match qn {
        30_000 => "杜比".to_owned(),
        20_000 => "4K".to_owned(),
        15_000 => "2K".to_owned(),
        10_000 => "原画".to_owned(),
        400 => "蓝光".to_owned(),
        250 => "超清".to_owned(),
        150 => "高清".to_owned(),
        80 => "流畅".to_owned(),
        other => format!("qn {other}"),
    }
}

/// 把 accept_qn 收敛成菜单：过滤非法值、去重、降序（最高档在最前）。
fn quality_options(accept_qn: &[u64], fallback: u64) -> Vec<(u64, String)> {
    let mut qns: Vec<u64> = accept_qn
        .iter()
        .copied()
        .filter(|qn| (1..=1_000_000).contains(qn))
        .collect();
    qns.sort_unstable_by(|left, right| right.cmp(left));
    qns.dedup();
    let mut options: Vec<(u64, String)> =
        qns.into_iter().map(|qn| (qn, quality_label(qn))).collect();
    if options.is_empty() {
        options.push((fallback, quality_label(fallback)));
    }
    options
}

/// 视频模式打开时由 TUI 传给 mpv 的初始音频状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VideoAudioOptions {
    volume: u8,
    paused: bool,
    /// 视频追帧目标缓冲（毫秒），握手时传给 np-video；外部播放器忽略。
    buffer_ms: u64,
    /// 启动时的 TUI 主题；运行期变化走 `theme ...` 控制行。
    theme: VideoTheme,
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

/// 行协议：v2 + 基础字段 + 清晰度菜单 + end。标题里的换行会被压成空格。
///
/// `buffer_ms` 是 `live.buffer_ms` 的当前值，np-video 据此调整 mpv speed；
/// `quality <qn> <label>` / `current_qn <qn>` 是 T-0117 新增字段，旧 np-video
/// 会忽略它们。URL / 标签都不含换行，避免注入额外协议行。
pub(super) fn host_request(
    target: &VideoTarget,
    audio: VideoAudioOptions,
) -> Result<String, String> {
    let url = target.url.trim();
    if url.is_empty()
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(['\r', '\n', '\0', ' ', '#'])
    {
        return Err("视频地址格式异常".to_owned());
    }
    let title = sanitize_title(&target.title);
    let mut request = format!(
        "v2\nurl {url}\nreferer {VIDEO_REFERER}\norigin {VIDEO_ORIGIN}\ntitle {title}\nvolume {}\npaused {}\nbuffer_ms {}\n",
        audio.volume.min(100),
        u8::from(audio.paused),
        audio.buffer_ms
    );
    // 主题行只含九个 `#rrggbb`，不含 URL / Cookie。
    request.push_str(&audio.theme.protocol_line());
    for (qn, label) in target.qualities.iter().take(16) {
        let label: String = label
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .take(40)
            .collect();
        let label = label.trim();
        if *qn > 0 && !label.is_empty() {
            request.push_str(&format!("quality {qn} {label}\n"));
        }
    }
    if let Some(qn) = target.current_qn {
        request.push_str(&format!("current_qn {qn}\n"));
    }
    request.push_str("end\n");
    Ok(request)
}

/// 构造 `load <url>\n`；拒绝换行 / 空格 / 注释行，避免协议注入。
pub(super) fn load_line(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty()
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || url.contains(['\r', '\n', '\0', ' ', '#'])
    {
        return None;
    }
    Some(format!("load {url}\n"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostEvent {
    Ready,
    Closed,
    Error(String),
    /// `control paused 1|0`：宿主控制条改了暂停态，TUI 只同步状态。
    ControlPaused(bool),
    /// `control refresh`：TUI 用当前 qn 重新取流并发 `load`。
    ControlRefresh,
    /// `volume <n>`：宿主控制条改了音量，TUI 更新自身音量态。
    Volume(u8),
    /// `control quality <qn>`：TUI 用该 qn 重新取流并发 `load`。
    ControlQuality(u64),
}

impl HostEvent {
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line == "ready" {
            return Some(Self::Ready);
        }
        if line == "closed" {
            return Some(Self::Closed);
        }
        if line == "control refresh" {
            return Some(Self::ControlRefresh);
        }
        if let Some(raw) = line.strip_prefix("control paused ") {
            return match raw.trim() {
                "1" | "true" => Some(Self::ControlPaused(true)),
                "0" | "false" => Some(Self::ControlPaused(false)),
                _ => None,
            };
        }
        if let Some(raw) = line.strip_prefix("control quality ") {
            return raw.trim().parse::<u64>().ok().map(Self::ControlQuality);
        }
        if let Some(raw) = line.strip_prefix("volume ") {
            return raw
                .trim()
                .parse::<u8>()
                .ok()
                .filter(|volume| *volume <= 100)
                .map(Self::Volume);
        }
        line.strip_prefix("error ")
            .map(|message| Self::Error(message.trim().to_owned()))
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

    fn send_control(&mut self, line: &str) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        if let Err(error) = stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()) {
            log::warn!(target: "video", "control send failed: {error}");
        }
    }

    fn set_volume(&mut self, volume: u8) {
        self.send_control(&format!("volume {}\n", volume.min(100)));
    }

    fn set_paused(&mut self, paused: bool) {
        self.send_control(if paused { "pause\n" } else { "resume\n" });
    }

    /// 运行期主题变化：只重绘 np-video 的标题栏 / 控制条，不重启 mpv。
    fn set_theme(&mut self, theme: VideoTheme) {
        self.send_control(&theme.protocol_line());
    }

    /// 让现有 np-video / mpv 换流；URL 不写日志。
    fn load(&mut self, url: &str) -> bool {
        let Some(line) = load_line(url) else {
            log::warn!(target: "video", "event=load_rejected");
            return false;
        };
        self.send_control(&line);
        log::info!(target: "video", "event=load_sent");
        true
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

fn spawn_host(
    path: &Path,
    target: &VideoTarget,
    audio: VideoAudioOptions,
) -> Result<HostChild, String> {
    let request = host_request(target, audio)?;
    let mut command = host_command(path);
    command.env("NP_LOG_DIR", crate::logging::dir());
    command.env(
        "NP_LOG",
        std::env::var("NP_LOG").unwrap_or_else(|_| "info".to_owned()),
    );
    let mut child = command
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

/// 一次清晰度 / 刷新请求：都走独立线程重新取流，成功后向宿主发 `load`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QualitySwitchKind {
    Refresh,
    Quality(u64),
}

impl QualitySwitchKind {
    fn label(self) -> &'static str {
        match self {
            Self::Refresh => "刷新",
            Self::Quality(_) => "清晰度",
        }
    }
}

/// 后台质量取流回传：generation + 请求类型 + 结果。
type QualityFetch = (u64, QualitySwitchKind, Result<VideoTarget, String>);

struct VideoState {
    phase: VideoPhase,
    generation: u64,
    backend: Option<VideoBackend>,
    external_child: Option<Child>,
    host: Option<HostChild>,
    fetch_rx: Option<mpsc::Receiver<VideoFetch>>,
    note: Option<VideoNote>,
    quality_note: Option<String>,
    audio: VideoAudioOptions,
    room_id: u64,
    auth: Option<np_live::BiliAuth>,
    /// 最近一次成功请求的 qn；刷新沿用它。
    requested_qn: u64,
    /// 服务端实际返回的 qn。
    current_qn: Option<u64>,
    /// 当前在播 URL；质量切换失败时保留旧画面。
    current_url: Option<String>,
    quality_rx: Option<mpsc::Receiver<QualityFetch>>,
    quality_pending: Option<QualitySwitchKind>,
    host_volume: Option<u8>,
    host_paused: Option<bool>,
    /// 最近一次发送给宿主的主题；用于每 tick 比较，避免重复发。
    theme: Option<VideoTheme>,
    #[cfg(test)]
    host_test_tx: Option<mpsc::Sender<HostEvent>>,
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
                quality_note: None,
                audio: VideoAudioOptions {
                    volume: 100,
                    paused: false,
                    buffer_ms: crate::live_page::DEFAULT_LIVE_BUFFER_MS,
                    theme: VideoTheme::default_dark(),
                },
                room_id: 0,
                auth: None,
                requested_qn: DEFAULT_VIDEO_QN,
                current_qn: None,
                current_url: None,
                quality_rx: None,
                quality_pending: None,
                host_volume: None,
                host_paused: None,
                theme: None,
                #[cfg(test)]
                host_test_tx: None,
            }),
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.inner.borrow().phase != VideoPhase::Idle
    }

    /// 只有自建宿主由 mpv 出声；外部播放器仍是 TUI 音频。
    pub(crate) fn uses_mpv_audio(&self) -> bool {
        matches!(
            self.inner.borrow().backend.as_ref(),
            Some(VideoBackend::Host { .. })
        )
    }

    /// 每 tick 比较当前 `Palette`；变化时向宿主发 `theme ...`，只重绘窗口。
    ///
    /// 返回是否真的给运行中的宿主发了新主题（用于事件循环判断）。
    pub(crate) fn sync_theme(&self, palette: &Palette) -> bool {
        let theme = VideoTheme::from_palette(palette);
        let mut state = self.inner.borrow_mut();
        // 取流 / 启动阶段也要更新，保证握手用的是最新主题。
        state.audio.theme = theme;
        if state.theme == Some(theme) {
            return false;
        }
        state.theme = Some(theme);
        if let Some(host) = state.host.as_mut() {
            host.set_theme(theme);
        }
        true
    }

    /// 视频模式音量（0–100）；外置播放器无法控制时只改状态。
    pub(crate) fn set_volume(&self, volume: u8) {
        let mut state = self.inner.borrow_mut();
        state.audio.volume = volume.min(100);
        let audio = state.audio;
        if let Some(host) = state.host.as_mut() {
            host.set_volume(audio.volume);
        }
    }

    /// 视频模式暂停/继续；只影响 mpv，TUI 音频状态由调用方维护。
    pub(crate) fn set_paused(&self, paused: bool) {
        let mut state = self.inner.borrow_mut();
        state.audio.paused = paused;
        if let Some(host) = state.host.as_mut() {
            host.set_paused(paused);
        }
    }

    /// 启动一次取流；URL 到手后在 [`Self::poll`] 里才真正拉起宿主 / 播放器。
    fn begin(
        &self,
        backend: VideoBackend,
        room_id: u64,
        auth: Option<np_live::BiliAuth>,
        audio: VideoAudioOptions,
    ) {
        let mut state = self.inner.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.phase = VideoPhase::Preparing;
        state.backend = Some(backend);
        state.note = None;
        state.quality_note = None;
        state.external_child = None;
        state.host = None;
        state.audio = audio;
        state.theme = Some(audio.theme);
        state.room_id = room_id;
        state.auth = auth.clone();
        state.requested_qn = DEFAULT_VIDEO_QN;
        state.current_qn = None;
        state.current_url = None;
        state.quality_rx = None;
        state.quality_pending = None;
        state.host_volume = None;
        state.host_paused = None;
        log::info!(
            target: "video",
            "event=begin room={room_id} volume={} paused={} buffer_ms={} qn={DEFAULT_VIDEO_QN}",
            audio.volume,
            audio.paused,
            audio.buffer_ms
        );
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
                Ok(target) => {
                    log::info!(
                        target: "video",
                        "event=target title={} quality={:?} codec={:?} current_qn={:?} accept_qn={}",
                        target.title,
                        target.quality_note,
                        target.codec,
                        target.current_qn,
                        target.qualities.len()
                    );
                    state.quality_note.clone_from(&target.quality_note);
                    state.requested_qn = target.requested_qn;
                    state.current_qn = target.current_qn;
                    state.current_url = Some(target.url.clone());
                    let audio = state.audio;
                    match state.backend.clone() {
                        Some(VideoBackend::Host { path }) => {
                            match spawn_host(&path, &target, audio) {
                                Ok(host) => {
                                    log::info!(target: "video", "event=host_spawned");
                                    state.host = Some(host);
                                    state.phase = VideoPhase::Starting;
                                }
                                Err(error) => {
                                    log::error!(target: "video", "event=host_failed error={error}");
                                    state.phase = VideoPhase::Idle;
                                    state.backend = None;
                                    state.note =
                                        Some(VideoNote::new(format!("视频窗口启动失败：{error}")));
                                }
                            }
                        }
                        Some(VideoBackend::External(program)) => {
                            match player_command(&program, &target.url).spawn() {
                                Ok(child) => {
                                    state.external_child = Some(child);
                                    state.phase = VideoPhase::Playing;
                                    let quality =
                                        target.quality_note.as_deref().unwrap_or("清晰度未知");
                                    state.note = Some(VideoNote::new(format!(
                                        "视频已打开（{}，{quality}，v 关闭）",
                                        program.kind.title()
                                    )));
                                }
                                Err(error) => {
                                    state.phase = VideoPhase::Idle;
                                    state.backend = None;
                                    state.note = Some(VideoNote::new(format!(
                                        "视频播放器启动失败：{error}"
                                    )));
                                }
                            }
                        }
                        None => {
                            state.phase = VideoPhase::Idle;
                            state.note = Some(VideoNote::new("视频播放器丢失，请重新打开"));
                        }
                    }
                }
                Err(message) => {
                    log::warn!(target: "video", "event=fetch_failed error={message}");
                    state.phase = VideoPhase::Idle;
                    state.backend = None;
                    state.note = Some(VideoNote::new(format!("视频取流失败：{message}")));
                }
            }
            changed = true;
        }

        // 宿主事件：ready / closed / error / 控制条动作。取走宿主时先收尾，避免借冲突。
        let events = state
            .host
            .as_ref()
            .map(HostChild::drain_events)
            .unwrap_or_default();
        let mut switch = None;
        for event in events {
            match event {
                HostEvent::Ready => {
                    if state.phase == VideoPhase::Starting {
                        log::info!(target: "video", "event=ready");
                        state.phase = VideoPhase::Playing;
                        let note = match state.quality_note.as_deref() {
                            Some(quality) => format!("视频已打开（{quality}，v 关闭）"),
                            None => "视频已打开（v 关闭）".to_owned(),
                        };
                        state.note = Some(VideoNote::new(note));
                        let audio = state.audio;
                        if let Some(host) = state.host.as_mut() {
                            host.set_volume(audio.volume);
                            host.set_paused(audio.paused);
                        }
                        changed = true;
                    }
                }
                HostEvent::Closed => {
                    log::info!(target: "video", "event=closed");
                    if let Some(mut host) = state.host.take() {
                        host.shutdown();
                    }
                    state.phase = VideoPhase::Idle;
                    state.backend = None;
                    state.note = Some(VideoNote::new("视频窗口已关闭"));
                    changed = true;
                }
                HostEvent::Error(message) => {
                    // 清晰度切换失败：np-video 已回退旧 URL，TUI 保留旧画面。
                    if state.phase == VideoPhase::Playing && message.contains("清晰度") {
                        log::warn!(target: "video", "event=load_failed error={message}");
                        state.note = Some(VideoNote::new(format!("视频提示：{message}")));
                        changed = true;
                        continue;
                    }
                    log::warn!(target: "video", "event=host_error error={message}");
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
                HostEvent::ControlPaused(paused) => {
                    log::info!(target: "video", "event=host_paused paused={paused}");
                    state.audio.paused = paused;
                    state.host_paused = Some(paused);
                    changed = true;
                }
                HostEvent::Volume(volume) => {
                    log::info!(target: "video", "event=host_volume volume={volume}");
                    state.audio.volume = volume;
                    state.host_volume = Some(volume);
                    changed = true;
                }
                HostEvent::ControlRefresh => {
                    log::info!(target: "video", "event=host_refresh");
                    switch = Some(QualitySwitchKind::Refresh);
                }
                HostEvent::ControlQuality(qn) => {
                    log::info!(target: "video", "event=host_quality qn={qn}");
                    switch = Some(QualitySwitchKind::Quality(qn));
                }
            }
        }

        // 质量 / 刷新取流结果：成功才向宿主发 `load`；失败保留旧画面。
        let quality_result = match state.quality_rx.as_ref() {
            Some(receiver) => match receiver.try_recv() {
                Ok(value) => Some(value),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    state.generation,
                    state.quality_pending.unwrap_or(QualitySwitchKind::Refresh),
                    Err("清晰度取流线程意外结束".to_owned()),
                )),
            },
            None => None,
        };
        if let Some((generation, kind, result)) = quality_result
            && generation == state.generation
            && state.quality_rx.is_some()
        {
            state.quality_rx = None;
            state.quality_pending = None;
            match result {
                Ok(target) => {
                    log::info!(
                        target: "video",
                        "event=quality_target codec={:?} current_qn={:?} accept_qn={} requested_qn={}",
                        target.codec,
                        target.current_qn,
                        target.qualities.len(),
                        target.requested_qn
                    );
                    state.quality_note.clone_from(&target.quality_note);
                    state.requested_qn = target.requested_qn;
                    state.current_qn = target.current_qn;
                    state.current_url = Some(target.url.clone());
                    let loaded = state
                        .host
                        .as_mut()
                        .is_some_and(|host| host.load(&target.url));
                    let note = match kind {
                        QualitySwitchKind::Refresh => {
                            if loaded {
                                "已刷新当前清晰度".to_owned()
                            } else {
                                "刷新失败，保持原画面".to_owned()
                            }
                        }
                        QualitySwitchKind::Quality(qn) => {
                            let actual = target.current_qn.unwrap_or(qn);
                            if !loaded {
                                "清晰度切换失败，保持原画面".to_owned()
                            } else if actual == qn {
                                format!("清晰度已切换（{}）", quality_label(qn))
                            } else {
                                format!(
                                    "清晰度已切换（请求 {}，服务端回退 {}）",
                                    quality_label(qn),
                                    quality_label(actual)
                                )
                            }
                        }
                    };
                    state.note = Some(VideoNote::new(note));
                }
                Err(message) => {
                    log::warn!(target: "video", "event=quality_failed error={message}");
                    state.note = Some(VideoNote::new(format!(
                        "{}失败，保持原画面：{message}",
                        kind.label()
                    )));
                }
            }
            changed = true;
        }

        // 宿主进程退出（用户关窗、mpv 退出或崩溃）：即使没读到事件也要回到 Idle。
        if state.host.is_some() {
            let outcome = state.host.as_mut().and_then(HostChild::try_wait);
            if let Some(status) = outcome {
                let was_starting = state.phase == VideoPhase::Starting;
                if let Some(mut host) = state.host.take() {
                    host.shutdown();
                }
                log::warn!(target: "video", "event=process_exit status={status:?}");
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
        drop(state);
        // 宿主事件读完后才能再借 VideoState 去起取流线程。
        if let Some(kind) = switch
            && self.begin_quality_switch(kind)
        {
            changed = true;
        }
        changed
    }

    /// 用 `kind` 对应的 qn 重新取流；成功后由 [`Self::poll`] 向宿主发 `load`。
    fn begin_quality_switch(&self, kind: QualitySwitchKind) -> bool {
        let mut state = self.inner.borrow_mut();
        if state.phase != VideoPhase::Playing || state.host.is_none() {
            return false;
        }
        // 新的请求覆盖仍在途的旧请求，避免用户快速切换时排长队。
        state.quality_rx = None;
        // 刷新用服务端实际在播的 qn；质量切换用菜单选中的 qn。
        let qn = match kind {
            QualitySwitchKind::Refresh => state.current_qn.unwrap_or(state.requested_qn),
            QualitySwitchKind::Quality(qn) => qn,
        };
        let room_id = state.room_id;
        let auth = state.auth.clone();
        let generation = state.generation;
        let (tx, rx) = mpsc::channel();
        state.quality_rx = Some(rx);
        state.quality_pending = Some(kind);
        drop(state);
        log::info!(target: "video", "event=quality_requested qn={qn} kind={kind:?}");
        match std::thread::Builder::new()
            .name("np-tui-video-quality".to_owned())
            .spawn(move || {
                let result = fetch_video_target_qn(room_id, auth, qn);
                let _ = tx.send((generation, kind, result));
            }) {
            Ok(_) => true,
            Err(error) => {
                log::error!(target: "video", "event=quality_thread_failed error={error}");
                let mut state = self.inner.borrow_mut();
                state.quality_rx = None;
                state.quality_pending = None;
                state.note = Some(VideoNote::new(format!("{}请求失败：{error}", kind.label())));
                true
            }
        }
    }

    /// 取走宿主控制条刚发来的音量 / 暂停态（由 [`TuiAudio::poll_video`] 同步）。
    pub(super) fn take_host_audio(&self) -> (Option<u8>, Option<bool>) {
        let mut state = self.inner.borrow_mut();
        (state.host_volume.take(), state.host_paused.take())
    }
    /// 关闭窗口并回收子进程；返回之前是否有视频在准备/播放。
    fn close(&self) -> bool {
        let mut state = self.inner.borrow_mut();
        let was_active = state.phase != VideoPhase::Idle
            || state.host.is_some()
            || state.external_child.is_some();
        state.generation = state.generation.wrapping_add(1);
        state.fetch_rx = None;
        state.quality_rx = None;
        state.quality_pending = None;
        state.current_url = None;
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
    let info = np_live::api::fetch_video_play_info_with_auth_qn(
        &transport,
        room_id,
        auth.as_ref(),
        DEFAULT_VIDEO_QN,
    )
    .map_err(|error| error.user_message())?;
    // 房间标题只用于标题栏；拉不到就回退“直播 <room>”，不影响播放。
    let title = np_live::api::fetch_room_info(&transport, room_id)
        .ok()
        .map(|room| room.title);
    target_from_parts(room_id, info, title, DEFAULT_VIDEO_QN)
}

/// 清晰度切换 / 刷新时只重取 playurl，不再拉一次房间资料。
fn fetch_video_target_qn(
    room_id: u64,
    auth: Option<np_live::BiliAuth>,
    qn: u64,
) -> Result<VideoTarget, String> {
    let transport = np_sources::ReqwestTransport::new()
        .map_err(|error| format!("视频网络初始化失败：{error}"))?;
    let info =
        np_live::api::fetch_video_play_info_with_auth_qn(&transport, room_id, auth.as_ref(), qn)
            .map_err(|error| error.user_message())?;
    target_from_parts(room_id, info, None, qn)
}

/// 把 playurl 结果 + 房间标题收敛成一次启动目标；错误保持中文，不暴露 URL。
pub(super) fn target_from_parts(
    room_id: u64,
    info: np_live::api::PlayInfo,
    room_title: Option<String>,
    requested_qn: u64,
) -> Result<VideoTarget, String> {
    let current_qn = info.current_qn;
    let accept_qn = info.accept_qn.clone();
    let codec = info.codec.clone();
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
    Ok(VideoTarget {
        url,
        title,
        quality_note: quality_note(current_qn, &accept_qn),
        qualities: quality_options(&accept_qn, requested_qn),
        codec,
        current_qn,
        requested_qn,
    })
}

/// 当前画面档位；服务端按权限回退时附带菜单里的最高可用档。
fn quality_note(current_qn: Option<u64>, accept_qn: &[u64]) -> Option<String> {
    let qn = current_qn?;
    let current = format!("{} {qn}", quality_label(qn));
    match accept_qn.iter().copied().filter(|best| *best > qn).max() {
        Some(best) => Some(format!(
            "当前 {current}，最高可用 {} {best}",
            quality_label(best)
        )),
        None => Some(format!("当前 {current}")),
    }
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

    /// 只有 np-video 宿主由 mpv 出声；外部 mpv/ffplay 仍走 TUI 音频。
    pub(crate) fn video_uses_mpv_audio(&self) -> bool {
        self.video.is_active() && self.video.uses_mpv_audio()
    }

    /// `v`：打开 / 关闭视频窗口；无主题参数的入口给测试 / 关闭路径用。
    pub(crate) fn toggle_video(&self) -> Result<String, String> {
        self.toggle_video_with_theme(&Palette::default_dark())
    }

    /// `v`：打开 / 关闭视频窗口，启动时把当前 TUI 主题写进握手。
    pub(crate) fn toggle_video_with_theme(&self, palette: &Palette) -> Result<String, String> {
        if self.video.is_active() {
            let mpv_audio = self.video_uses_mpv_audio();
            self.video.close();
            if mpv_audio && let Some(player) = self.player.borrow().as_ref() {
                player.set_output_muted(false);
                if self.live_state.get() == super::LiveState::Paused {
                    let _ = player.pause();
                }
            }
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
        let volume = (self.volume() * 100.0).round().clamp(0.0, 100.0) as u8;
        let paused = self.live_state.get() == super::LiveState::Paused;
        let buffer_ms = self.live_buffer_target_ms();
        let mpv_audio = matches!(backend, VideoBackend::Host { .. });
        self.video.begin(
            backend,
            self.live_room.get(),
            auth,
            VideoAudioOptions {
                volume,
                paused,
                buffer_ms,
                theme: VideoTheme::from_palette(palette),
            },
        );
        if mpv_audio && let Some(player) = self.player.borrow().as_ref() {
            player.set_output_muted(true);
        }
        Ok("视频准备中…".to_owned())
    }

    /// 视频模式音量：把 TUI 0–1 音量映射到 mpv 0–100。
    pub(crate) fn set_video_volume(&self, volume: f32) {
        let volume = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        self.video.set_volume(volume);
    }

    /// 停止直播 / 换房 / F5 / 退出时关闭视频；返回是否有进程被清理。
    pub(crate) fn close_video(&self) -> bool {
        self.video.close()
    }

    /// 主循环每 tick：把当前 TUI 主题同步给视频宿主（变化时才发）。
    pub(crate) fn sync_video_theme(&self, palette: &Palette) -> bool {
        self.video.sync_theme(palette)
    }

    /// 主循环 tick：回收播放器 / 宿主进程、接住取流结果和宿主事件。
    pub(crate) fn poll_video(&self) -> bool {
        let changed = self.video.poll();
        // 宿主控制条是权威：音量 / 暂停事件到达后同步 TUI 状态，但不回发。
        self.sync_video_host_state();
        // 宿主启动失败 / 退出后自动解除 TUI 静音；成功时保持静音。
        if let Some(player) = self.player.borrow().as_ref() {
            player.set_output_muted(self.video_uses_mpv_audio());
            if !self.video_uses_mpv_audio()
                && self.is_live()
                && self.live_state.get() == super::LiveState::Paused
            {
                let _ = player.pause();
            }
        }
        changed
    }

    /// 把宿主控制条的音量 / 暂停同步到 TUI（宿主为准，不回发避免双控制）。
    fn sync_video_host_state(&self) {
        let (volume, paused) = self.video.take_host_audio();
        if let Some(volume) = volume {
            let volume = f32::from(volume) / 100.0;
            self.volume.set(volume);
            if let Some(player) = self.player.borrow().as_ref() {
                player.set_volume(volume);
            }
        }
        if let Some(paused) = paused
            && self.is_live()
        {
            let target = if paused {
                super::LiveState::Paused
            } else {
                super::LiveState::Playing
            };
            let current = self.live_state.get();
            let syncable = !matches!(
                current,
                super::LiveState::Idle
                    | super::LiveState::Preparing
                    | super::LiveState::Reconnecting
            );
            if syncable && current != target {
                self.live_state.set(target);
                self.playing.set(!paused);
                *self.live_note.borrow_mut() = Some(if paused {
                    "视频已暂停".to_owned()
                } else {
                    "视频已继续".to_owned()
                });
            }
        }
    }

    /// Live 页状态栏提示；无视频且无提示时返回 `None`，保证旧 golden 不变。
    pub(crate) fn video_status_hint(&self) -> Option<String> {
        self.video.hint()
    }
}

#[cfg(test)]
impl VideoController {
    pub(super) fn attach_child_for_test(&self, child: Child) {
        let mut state = self.inner.borrow_mut();
        state.phase = VideoPhase::Playing;
        state.backend = Some(VideoBackend::External(VideoProgram {
            kind: VideoPlayerKind::Ffplay,
            path: PathBuf::from("fake-player"),
        }));
        state.external_child = Some(child);
    }

    pub(super) fn mark_active_for_test(&self) {
        let mut state = self.inner.borrow_mut();
        state.phase = VideoPhase::Playing;
        state.backend = Some(VideoBackend::External(VideoProgram {
            kind: VideoPlayerKind::Ffplay,
            path: PathBuf::from("fake-player"),
        }));
    }

    pub(super) fn set_note_for_test(&self, text: &str) {
        self.inner.borrow_mut().note = Some(VideoNote::new(text));
    }

    pub(super) fn audio_for_test(&self) -> (u8, bool) {
        let state = self.inner.borrow();
        (state.audio.volume, state.audio.paused)
    }

    pub(super) fn theme_for_test(&self) -> Option<VideoTheme> {
        self.inner.borrow().theme
    }

    /// 模拟后台取流失败，让 `poll` 走进错误分支。
    pub(super) fn inject_fetch_error_for_test(&self, message: &str) {
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

    /// 测试用：挂一个假的宿主子进程（可无进程），并保留事件发送端。
    pub(super) fn attach_host_for_test(&self) {
        let (tx, events) = mpsc::channel();
        self.attach_host_with_events_for_test(events);
        self.inner.borrow_mut().host_test_tx = Some(tx);
    }

    /// 测试用：按协议行注入一条宿主事件；返回是否被解析并送达。
    pub(super) fn host_event_for_test(&self, line: &str) -> bool {
        let Some(event) = HostEvent::parse(line) else {
            return false;
        };
        let state = self.inner.borrow();
        state
            .host_test_tx
            .as_ref()
            .is_some_and(|tx| tx.send(event).is_ok())
    }

    /// 测试用：挂一个真实会退出的宿主子进程（用于进程退出兜底）。
    pub(super) fn attach_exiting_host_for_test(&self) {
        let (_tx, events) = mpsc::channel();
        self.attach_host_with_events_for_test(events);
        if let Some(host) = self.inner.borrow_mut().host.as_mut() {
            host.child = Some(exit_child());
        }
    }

    pub(super) fn attach_host_with_events_for_test(&self, events: mpsc::Receiver<HostEvent>) {
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

    /// 测试用：挂一个假的宿主。
    pub(crate) fn video_host_for_test(&self) {
        self.video.attach_host_for_test();
    }
}

/// 测试专用门面：根模块 `tests.rs` 与 `audio::video` 是兄弟模块，无法直接
/// 访问本模块的私有类型 / 函数；这里只暴露返回普通数据结构的入口。
#[cfg(test)]
impl TuiAudio {
    pub(crate) fn video_player_args_for_test(mpv: bool, url: &str) -> Vec<String> {
        player_args(
            if mpv {
                VideoPlayerKind::Mpv
            } else {
                VideoPlayerKind::Ffplay
            },
            url,
        )
    }

    pub(crate) fn video_player_command_for_test(mpv: bool, url: &str) -> (PathBuf, Vec<String>) {
        let program = VideoProgram {
            kind: if mpv {
                VideoPlayerKind::Mpv
            } else {
                VideoPlayerKind::Ffplay
            },
            path: PathBuf::from("fake-player"),
        };
        let command = player_command(&program, url);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        (PathBuf::from(command.get_program()), args)
    }

    pub(crate) fn video_resolve_program_for_test(
        override_path: Option<&OsStr>,
        path_var: Option<&OsStr>,
    ) -> Result<(bool, PathBuf), String> {
        resolve_program(override_path, path_var)
            .map(|program| (program.kind == VideoPlayerKind::Ffplay, program.path))
    }

    pub(crate) fn video_resolve_host_for_test(
        override_path: Option<&OsStr>,
        exe_dir: Option<&Path>,
        path_var: Option<&OsStr>,
    ) -> Result<Option<PathBuf>, String> {
        resolve_host(override_path, exe_dir, path_var)
    }

    pub(crate) fn video_resolve_backend_for_test(
        host_override: Option<&OsStr>,
        player_override: Option<&OsStr>,
        exe_dir: Option<&Path>,
        path_var: Option<&OsStr>,
    ) -> Result<bool, String> {
        resolve_backend(host_override, player_override, exe_dir, path_var)
            .map(|backend| matches!(backend, VideoBackend::Host { .. }))
    }

    pub(crate) fn video_load_line_for_test(url: &str) -> Option<String> {
        load_line(url)
    }

    pub(crate) fn video_host_exe_for_test() -> &'static str {
        VIDEO_HOST_EXE
    }

    pub(crate) fn video_default_qn_for_test() -> u64 {
        DEFAULT_VIDEO_QN
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn video_host_request_for_test(
        url: &str,
        title: &str,
        qualities: &[(u64, String)],
        current_qn: Option<u64>,
        requested_qn: u64,
        volume: u8,
        paused: bool,
        buffer_ms: u64,
        theme: VideoTheme,
    ) -> Result<String, String> {
        let target = VideoTarget {
            url: url.to_owned(),
            title: title.to_owned(),
            quality_note: None,
            qualities: qualities.to_vec(),
            codec: None,
            current_qn,
            requested_qn,
        };
        host_request(
            &target,
            VideoAudioOptions {
                volume,
                paused,
                buffer_ms,
                theme,
            },
        )
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn video_target_from_parts_for_test(
        room_id: u64,
        info: np_live::api::PlayInfo,
        title: Option<String>,
        requested_qn: u64,
    ) -> Result<
        (
            String,
            String,
            Option<String>,
            Vec<(u64, String)>,
            Option<u64>,
            u64,
        ),
        String,
    > {
        target_from_parts(room_id, info, title, requested_qn).map(|target| {
            (
                target.url,
                target.title,
                target.quality_note,
                target.qualities,
                target.current_qn,
                target.requested_qn,
            )
        })
    }

    pub(crate) fn video_attach_child_for_test(&self, child: Child) {
        self.video.attach_child_for_test(child);
    }

    pub(crate) fn video_host_line_for_test(&self, line: &str) -> bool {
        self.video.host_event_for_test(line)
    }

    pub(crate) fn video_audio_state_for_test(&self) -> (u8, bool) {
        self.video.audio_for_test()
    }

    pub(crate) fn video_theme_for_test(&self) -> Option<VideoTheme> {
        self.video.theme_for_test()
    }

    pub(crate) fn video_inject_fetch_error_for_test(&self, message: &str) {
        self.video.inject_fetch_error_for_test(message);
    }

    pub(crate) fn video_set_paused_for_test(&self, paused: bool) {
        self.video.set_paused(paused);
    }

    pub(crate) fn video_exiting_host_for_test(&self) {
        self.video.attach_exiting_host_for_test();
    }
}

/// 测试用：一个会正常退出的子进程；不依赖 shell 参数之外的环境。
#[cfg(test)]
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
