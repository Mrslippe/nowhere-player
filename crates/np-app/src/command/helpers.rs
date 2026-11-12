use super::*;

/// 现在时刻（Unix 毫秒）。
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 这条命令是否是后台扫描（需要先推 `running` 再启动线程）。
pub(crate) fn is_library_scan(parsed: &Parsed) -> bool {
    parsed.spec.name == "library"
        && parsed
            .subcommand
            .is_some_and(|subcommand| subcommand.name == "scan")
}

/// 解析出 `library scan` 的 `--full`；不是扫描命令时返回 `None`。
pub fn scan_full_flag(input: &str) -> Option<bool> {
    match parse(input) {
        Ok(Some(parsed)) if is_library_scan(&parsed) => Some(parsed.flags.contains_key("full")),
        _ => None,
    }
}

/// `library scan` 的“已受理”输出：文案必须写明“扫描已开始”（T-0023 §11）。
pub(crate) fn scan_started_output(full: bool) -> (Vec<Output>, Option<String>) {
    let kind = if full { "全量扫描" } else { "增量扫描" };
    (
        vec![text_of(vec![bold("扫描已开始"), dim(format!(" · {kind}"))])],
        Some("进度会更新到本块；扫描完成前可继续使用其他命令".to_owned()),
    )
}

/// 读取 `--limit=N`；缺省走 `default`，非数字或 0 回 `E_CMD_ARG`。
pub(crate) fn flag_limit(parsed: &Parsed, default: usize) -> Result<usize, CmdError> {
    let Some(raw) = parsed.flags.get("limit") else {
        return Ok(default);
    };
    let value: usize = raw
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("limit 需要正整数，收到：{raw}")))?;
    if value == 0 {
        return Err(CmdError::new("E_CMD_ARG", "limit 必须大于 0"));
    }
    Ok(value)
}

/// 曲目时长：未知（0）显示 `--:--`。
pub(crate) fn format_track_duration(duration_ms: i64) -> String {
    if duration_ms <= 0 {
        return "--:--".to_owned();
    }
    format_position(u64::try_from(duration_ms).unwrap_or(0))
}

/// 库总时长：`X 小时 Y 分` / `Y 分` / `Z 秒`。
pub(crate) fn format_total_duration(total_ms: u64) -> String {
    let seconds = total_ms / 1_000;
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if hours > 0 {
        format!("{hours} 小时 {minutes} 分")
    } else if minutes > 0 {
        format!("{minutes} 分")
    } else {
        format!("{seconds} 秒")
    }
}

/// 最近扫描的粗粒度相对时间；只用于终端文案，精度到天。
pub(crate) fn format_relative_time(timestamp_ms: i64) -> String {
    let now = i64::try_from(now_millis()).unwrap_or(i64::MAX);
    let seconds = now.saturating_sub(timestamp_ms).max(0) / 1_000;
    if seconds < 60 {
        "刚刚".to_owned()
    } else if seconds < 3_600 {
        format!("{} 分钟前", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} 小时前", seconds / 3_600)
    } else {
        format!("{} 天前", seconds / 86_400)
    }
}

pub(crate) struct QueueRow {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) duration: String,
    pub(crate) source: Option<String>,
}

pub(crate) fn local_draft(track: &Track) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "local".to_owned(),
        path: track.path.clone(),
        source: String::new(),
        source_id: String::new(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
    }
}

pub(crate) fn remote_track_draft(track: &Track) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "remote".to_owned(),
        path: String::new(),
        source: track.source.clone(),
        source_id: track.source_id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
    }
}

pub(crate) fn same_playlist_item(draft: &NewPlaylistItem, item: &PlaylistItem) -> bool {
    if draft.kind == "local" {
        item.kind == "local" && item.path == draft.path
    } else {
        item.kind == "remote" && item.source == draft.source && item.source_id == draft.source_id
    }
}

pub(crate) fn remote_draft(track: &SourceTrack) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: "remote".to_owned(),
        path: String::new(),
        source: track.source.as_str().to_owned(),
        source_id: track.source_id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone().unwrap_or_default(),
        duration_ms: i64::try_from(track.duration_ms).unwrap_or(i64::MAX),
    }
}

pub(crate) fn playlist_draft(item: &PlaylistItem) -> NewPlaylistItem {
    NewPlaylistItem {
        kind: item.kind.clone(),
        path: item.path.clone(),
        source: item.source.clone(),
        source_id: item.source_id.clone(),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms,
    }
}

pub(crate) fn playlist_item_source(item: &PlaylistItem) -> String {
    if item.kind == "local" {
        return "本地".to_owned();
    }
    match item.source.as_str() {
        "bilibili" => "B站".to_owned(),
        "netease" => "网易云".to_owned(),
        other => other.to_owned(),
    }
}

pub(crate) fn source_from_str(raw: &str) -> Result<SourceId, CmdError> {
    match raw {
        "bilibili" => Ok(SourceId::Bilibili),
        "netease" => Ok(SourceId::Netease),
        "ytdlp" => Ok(SourceId::Ytdlp),
        other => Err(CmdError::new(
            "E_SRC_UNAVAILABLE",
            format!("歌单条目来源未知：{other}"),
        )),
    }
}

/// 队列行可显示的来源；本地库的 `local` 不单独占一列。
pub(crate) fn queue_track_source(track: &Track) -> Option<String> {
    match track.source.as_str() {
        "" | "local" => None,
        "bilibili" => Some("B站".to_owned()),
        "netease" => Some("网易云".to_owned()),
        other => Some(other.to_owned()),
    }
}

/// 播放类结果的简短尾部；`queue_length` 未知时给一句通用文案。
pub(crate) fn state_footer(state: &PlaybackState) -> String {
    state.queue_length.map_or_else(
        || "已开始播放".to_owned(),
        |length| format!("队列 {length} 首"),
    )
}

/// 取第一个位置参数。
pub(crate) fn positional_arg<'a>(parsed: &'a Parsed, name: &str) -> Result<&'a str, CmdError> {
    parsed
        .positional
        .first()
        .map(String::as_str)
        .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("缺少参数：{name}")))
}

/// 纯 ASCII 数字（无符号）——只有这种位置参数才是序号，其余按路径处理。
pub fn is_unsigned_integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 用户输入的 1 基序号 → 队列的 0 基下标（T-0018 §6.2 定死）。
pub(crate) fn user_index(value: &str) -> Result<usize, CmdError> {
    let number: u64 = value
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))?;
    if number == 0 {
        return Err(CmdError::new("E_CMD_ARG", "序号从 1 开始"));
    }
    usize::try_from(number - 1)
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("序号超出范围：{value}")))
}

/// `mm:ss` → 毫秒。
pub(crate) fn parse_mm_ss(text: &str) -> Option<u64> {
    let (minutes, seconds) = text.split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: u64 = seconds.parse().ok()?;
    minutes
        .checked_mul(60_000)?
        .checked_add(seconds.checked_mul(1_000)?)
}

/// 绝对位置：带冒号按 `mm:ss`，否则按毫秒。
pub(crate) fn parse_absolute_seek(raw: &str) -> Result<u64, CmdError> {
    if raw.contains(':') {
        parse_mm_ss(raw).ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    } else {
        raw.parse::<u64>()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))
    }
}

/// 相对位置：`±N` 是秒，`±mm:ss` 是分钟秒；结果钳在 `[0, duration]`。
pub(crate) fn parse_relative_seek(
    raw: &str,
    current_ms: u64,
    duration_ms: Option<u64>,
) -> Result<u64, CmdError> {
    let (negative, body) = match raw.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };

    let delta_ms: i128 = if body.contains(':') {
        i128::from(
            parse_mm_ss(body)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    } else {
        let seconds: u64 = body
            .parse()
            .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?;
        i128::from(
            seconds
                .checked_mul(1_000)
                .ok_or_else(|| CmdError::new("E_CMD_ARG", format!("无效的播放位置：{raw}")))?,
        )
    };

    let target = i128::from(current_ms) + if negative { -delta_ms } else { delta_ms };
    let upper = duration_ms.map_or(i128::MAX, i128::from);

    Ok(u64::try_from(target.clamp(0, upper)).unwrap_or(0))
}

/// 相对音量：当前百分比（0–100）+ 增量，钳制留给调用方。
pub(crate) fn relative_volume(raw: &str, current: f32) -> Result<i64, CmdError> {
    let delta: i64 = raw
        .parse()
        .map_err(|_| CmdError::new("E_CMD_ARG", format!("无效的音量：{raw}")))?;
    let current_percent = (current * 100.0).round().clamp(0.0, 100.0) as i64;

    Ok(current_percent + delta)
}

/// 枚举值 → 循环模式（解析层已归一化，未知值按 `off` 兜底）。
pub(crate) fn repeat_of(value: &str) -> RepeatMode {
    match value {
        "one" => RepeatMode::One,
        "all" => RepeatMode::All,
        _ => RepeatMode::Off,
    }
}

pub(crate) fn plain(text: impl Into<String>) -> Span {
    Span::plain(text)
}

pub(crate) fn bold(text: impl Into<String>) -> Span {
    Span {
        bold: true,
        ..Span::plain(text)
    }
}

pub(crate) fn dim(text: impl Into<String>) -> Span {
    Span {
        dim: true,
        ..Span::plain(text)
    }
}

pub(crate) fn text_of(spans: Vec<Span>) -> Output {
    Output::Text { spans, tone: None }
}

pub(crate) fn mode_spans(mode: &PlaybackMode) -> Vec<Span> {
    let repeat = match mode.repeat {
        RepeatMode::Off => "关",
        RepeatMode::One => "单曲",
        RepeatMode::All => "列表",
    };
    let shuffle = if mode.shuffle { "开" } else { "关" };

    vec![
        bold("模式"),
        plain(format!(" · 循环：{repeat} · 随机：{shuffle}")),
    ]
}

/// 曲目 id（本地文件为绝对路径）在终端里只显示文件名。
pub(crate) fn track_display(track_id: &str) -> String {
    std::path::Path::new(track_id)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| track_id.to_owned())
}

/// 毫秒 → `mm:ss`（超过一小时时分钟继续累加）。
pub(crate) fn format_position(position_ms: u64) -> String {
    let total_seconds = position_ms / 1_000;
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// `terminal:block` 事件名。
///
/// Tauri 2 的事件名只允许字母数字与 `- / : _`，**点号非法**，
/// 所以这里用冒号（`cmd.exec` 是命令名，不受同一限制）。
pub const EVENT_TERMINAL_BLOCK: &str = "terminal:block";

/// 扫描进度节流器：按**时间**而不是文件数放行（T-0023 §6.2）。
///
/// 时间由调用方注入，单测用合成时钟，不依赖真实睡眠。
pub(crate) struct TimeThrottle {
    interval_ms: u64,
    last_emitted_ms: Option<u64>,
}

impl TimeThrottle {
    pub(crate) const fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_emitted_ms: None,
        }
    }

    /// 第一次调用立即放行，之后每满 `interval_ms` 放行一次。
    pub(crate) fn allow(&mut self, now_ms: u64) -> bool {
        match self.last_emitted_ms {
            None => {
                self.last_emitted_ms = Some(now_ms);
                true
            }
            Some(last) if now_ms.saturating_sub(last) >= self.interval_ms => {
                self.last_emitted_ms = Some(now_ms);
                true
            }
            Some(_) => false,
        }
    }
}

/// 进度事件的发射函数；单测用闭包捕获推送的块。
pub type EmitFn = dyn Fn(&Block) + Send + Sync;
/// 时钟注入点（毫秒）。
pub type ClockFn = dyn Fn() -> u64 + Send + Sync;
