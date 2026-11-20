//! TUI → np-video 的一次性启动请求，以及 np-video → TUI 的状态行。
//!
//! 请求走 stdin，URL / header 不进入命令行（进程列表里看不到）。格式是行协议，
//! 便于人工调试，也避免在 np-video 里再引入 serde：
//!
//! ```text
//! v2
//! url https://...
//! referer https://live.bilibili.com/
//! origin https://live.bilibili.com
//! title 房间标题
//! volume 65
//! paused 0
//! buffer_ms 900
//! theme #accent #surface #hover #text #muted #faint #line #danger #accentFg
//! quality 10000 原画
//! quality 400 蓝光
//! current_qn 10000
//! end
//! ```
//!
//! `volume` / `paused` / `buffer_ms` / `theme` / `quality` / `current_qn` 可选；
//! 旧 TUI 不发时分别默认 100 / 不暂停 / [`DEFAULT_BUFFER_MS`] /
//! [`Theme::default`] / 无清晰度菜单 / 无当前档位（旧 np-video 也会忽略这些
//! 字段）。之后的 stdin 用于 `volume <0-100>`、`pause`、`resume`、
//! `theme #.. ×9`、`load <url>`、`quit`。
//!
//! np-video 回给 TUI 的行：`ready` / `closed` / `error ...`，以及控制条产生的
//! `control paused 1|0`、`control refresh`、`control quality <qn>`、`volume <n>`。

use std::io::BufRead;

/// 请求版本；TUI 与 np-video 必须一致。
pub const VERSION: &str = "v2";
/// 视频追帧目标缓冲（毫秒）缺省值；与 `live.buffer_ms` 默认值一致。
pub const DEFAULT_BUFFER_MS: u64 = 900;
/// `buffer_ms` 合法下限；与 `live.buffer_ms` 设置范围一致。
const MIN_BUFFER_MS: u64 = 500;
/// `buffer_ms` 合法上限。
const MAX_BUFFER_MS: u64 = 2_000;
/// 清晰度菜单项上限（B 站 accept_qn 最多 8 档，留足余量）。
pub const MAX_QUALITIES: usize = 16;
/// 清晰度标签最大字符数。
const MAX_QUALITY_LABEL_CHARS: usize = 40;
const MAX_REQUEST_LINES: usize = 64;
const MAX_REQUEST_LINE_BYTES: usize = 16 * 1024;
const MAX_EVENT_MESSAGE_CHARS: usize = 300;

/// 视频窗口的 9 色主题，与 TUI `theme ...` 行顺序一一对应。
///
/// 视频宿主是独立进程：不读主题 JSON，也不认识终端命名色，只接受具体 RGB。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub accent: (u8, u8, u8),
    /// 控制条 / 标题栏背景（TUI `bgElev`）。
    pub surface: (u8, u8, u8),
    pub hover: (u8, u8, u8),
    pub text: (u8, u8, u8),
    pub muted: (u8, u8, u8),
    pub faint: (u8, u8, u8),
    pub line: (u8, u8, u8),
    pub danger: (u8, u8, u8),
    pub accent_fg: (u8, u8, u8),
}

impl Default for Theme {
    /// graphite，与 TUI `Palette::default_dark` 对齐；旧 TUI 不发 theme 时使用。
    fn default() -> Self {
        Self {
            accent: (0xff, 0xb4, 0x54),
            surface: (0x14, 0x17, 0x1c),
            hover: (0x1a, 0x1e, 0x24),
            text: (0xe9, 0xe4, 0xda),
            muted: (0xa7, 0xa4, 0x9e),
            faint: (0x7d, 0x7c, 0x78),
            line: (0x1e, 0x20, 0x23),
            danger: (0xff, 0x8b, 0x7d),
            accent_fg: (0x0d, 0x0f, 0x12),
        }
    }
}

impl Theme {
    /// 解析 `#rrggbb`（大小写均可）；九个颜色缺一不可，多一个也拒绝。
    pub fn parse(value: &str) -> Result<Self, String> {
        let parts: Vec<&str> = value.split_whitespace().collect();
        if parts.len() != 9 {
            return Err(format!("视频主题需要 9 个颜色（收到 {} 个）", parts.len()));
        }
        let mut colors = [(0u8, 0u8, 0u8); 9];
        for (slot, part) in colors.iter_mut().zip(parts) {
            *slot = parse_hex(part).ok_or_else(|| "视频主题颜色格式异常".to_owned())?;
        }
        Ok(Self {
            accent: colors[0],
            surface: colors[1],
            hover: colors[2],
            text: colors[3],
            muted: colors[4],
            faint: colors[5],
            line: colors[6],
            danger: colors[7],
            accent_fg: colors[8],
        })
    }

    /// 与 [`Theme::parse`] 互逆的协议行。
    pub fn line(self) -> String {
        let hex = |(red, green, blue): (u8, u8, u8)| format!("#{red:02x}{green:02x}{blue:02x}");
        format!(
            "theme {} {} {} {} {} {} {} {} {}",
            hex(self.accent),
            hex(self.surface),
            hex(self.hover),
            hex(self.text),
            hex(self.muted),
            hex(self.faint),
            hex(self.line),
            hex(self.danger),
            hex(self.accent_fg),
        )
    }
}

fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

/// 启动一次嵌入播放所需的全部信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub referer: String,
    pub origin: String,
    pub title: String,
    /// 初始音量 0–100，默认 100。
    pub volume: u8,
    /// 初始暂停态，默认 false。
    pub paused: bool,
    /// 视频追帧目标缓冲（毫秒），默认 [`DEFAULT_BUFFER_MS`]。
    pub buffer_ms: u64,
    /// 启动主题；`None` = 旧 TUI，宿主用 [`Theme::default`]。
    pub theme: Option<Theme>,
    /// 清晰度菜单项 `(qn, 标签)`；空表示不显示清晰度入口。
    pub qualities: Vec<(u64, String)>,
    /// 当前清晰度档位；菜单里打勾。
    pub current_qn: Option<u64>,
}

impl Request {
    /// 校验来自 TUI 的字段；错误文本不回显字段值（URL 可能带 token）。
    pub fn validate(&self) -> Result<(), String> {
        validate_url(&self.url)?;
        validate_header(&self.referer, "Referer")?;
        validate_header(&self.origin, "Origin")?;
        validate_text(&self.title, "标题")?;
        if self.qualities.len() > MAX_QUALITIES {
            return Err("视频清晰度项过多".to_owned());
        }
        for (qn, label) in &self.qualities {
            validate_quality(*qn, label)?;
        }
        Ok(())
    }

    /// 标题为空时给一个可读占位。
    pub fn display_title(&self) -> &str {
        let title = self.title.trim();
        if title.is_empty() { "直播" } else { title }
    }
}

fn validate_url(url: &str) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("视频地址为空".to_owned());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("视频地址格式异常".to_owned());
    }
    if url.contains(['\r', '\n', '\0']) {
        return Err("视频地址格式异常".to_owned());
    }
    if url.starts_with('#') {
        // mpv --playlist=- 会把 # 开头的行当成注释。
        return Err("视频地址格式异常".to_owned());
    }
    Ok(())
}

fn validate_header(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{name} 为空"));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{name} 格式异常"));
    }
    // mpv 的 --http-header-fields 用逗号分隔多个 header。
    if value.contains(',') {
        return Err(format!("{name} 不能包含逗号"));
    }
    Ok(())
}

fn validate_text(value: &str, name: &str) -> Result<(), String> {
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{name} 格式异常"));
    }
    Ok(())
}

/// 校验后续 `load <url>` 命令里的流地址；错误文本不包含地址本身。
pub fn validate_stream_url(url: &str) -> Result<(), String> {
    validate_url(url)
}

fn validate_quality(qn: u64, label: &str) -> Result<(), String> {
    if !(1..=1_000_000).contains(&qn) {
        return Err("视频清晰度档位异常".to_owned());
    }
    let label = label.trim();
    if label.is_empty()
        || label.chars().count() > MAX_QUALITY_LABEL_CHARS
        || label.chars().any(char::is_control)
    {
        return Err("视频清晰度名称异常".to_owned());
    }
    Ok(())
}

/// 解析已经按行拆好的请求；第一行必须是 [`VERSION`]，最后一行必须是 `end`。
pub fn parse_request<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Request, String> {
    let mut lines = lines
        .into_iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    match lines.next().map(str::trim) {
        Some(version) if version == VERSION => {}
        Some(_) => return Err("视频协议版本不匹配".to_owned()),
        None => return Err("视频请求为空".to_owned()),
    }

    let mut url = None;
    let mut referer = None;
    let mut origin = None;
    let mut title = String::new();
    let mut volume = 100u8;
    let mut paused = false;
    let mut buffer_ms = DEFAULT_BUFFER_MS;
    let mut theme: Option<Theme> = None;
    let mut qualities: Vec<(u64, String)> = Vec::new();
    let mut current_qn = None;
    let mut ended = false;
    let mut count = 0usize;
    for line in lines {
        count += 1;
        if count > MAX_REQUEST_LINES {
            return Err("视频请求字段过多".to_owned());
        }
        let line = line.trim_end();
        if line == "end" {
            ended = true;
            break;
        }
        if line.is_empty() {
            continue;
        }
        // 空值字段（例如空标题）可以只写 key。
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "url" => set_once(&mut url, value, "url")?,
            "referer" => set_once(&mut referer, value, "referer")?,
            "origin" => set_once(&mut origin, value, "origin")?,
            "title" => title = value.to_owned(),
            "volume" => {
                volume = value
                    .parse::<u8>()
                    .ok()
                    .filter(|value| *value <= 100)
                    .ok_or_else(|| "视频音量必须是 0–100".to_owned())?;
            }
            "paused" => {
                paused = match value.trim() {
                    "1" | "true" | "yes" => true,
                    "0" | "false" | "no" => false,
                    _ => return Err("视频暂停态必须是 0/1".to_owned()),
                };
            }
            "buffer_ms" => {
                buffer_ms = value
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|value| (MIN_BUFFER_MS..=MAX_BUFFER_MS).contains(value))
                    .ok_or_else(|| "视频缓冲目标必须是 500–2000".to_owned())?;
            }
            "theme" => {
                // 非法 theme 整行拒绝，保留上一份主题（没有则用默认暗色）。
                if let Ok(parsed) = Theme::parse(value) {
                    theme = Some(parsed);
                }
            }
            "quality" => {
                if qualities.len() >= MAX_QUALITIES {
                    return Err("视频清晰度项过多".to_owned());
                }
                let (qn, label) = value
                    .split_once(' ')
                    .ok_or_else(|| "视频清晰度格式异常".to_owned())?;
                let qn = qn
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "视频清晰度档位异常".to_owned())?;
                if qualities.iter().any(|(existing, _)| *existing == qn) {
                    return Err("视频清晰度档位重复".to_owned());
                }
                let label = label.trim();
                validate_quality(qn, label)?;
                qualities.push((qn, label.to_owned()));
            }
            "current_qn" => {
                if current_qn.is_some() {
                    return Err("视频请求重复 current_qn".to_owned());
                }
                let qn = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "视频当前清晰度异常".to_owned())?;
                if !(1..=1_000_000).contains(&qn) {
                    return Err("视频当前清晰度异常".to_owned());
                }
                current_qn = Some(qn);
            }
            // 未来字段：忽略，保持向前兼容。
            _ => {}
        }
    }
    if !ended {
        return Err("视频请求缺少结束标记".to_owned());
    }

    let request = Request {
        url: url.ok_or_else(|| "视频请求缺少 url".to_owned())?,
        referer: referer.ok_or_else(|| "视频请求缺少 referer".to_owned())?,
        origin: origin.ok_or_else(|| "视频请求缺少 origin".to_owned())?,
        title,
        volume,
        paused,
        buffer_ms,
        theme,
        qualities,
        current_qn,
    };
    request.validate()?;
    Ok(request)
}

fn set_once(slot: &mut Option<String>, value: &str, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("视频请求重复 {name}"));
    }
    *slot = Some(value.to_owned());
    Ok(())
}

/// 从 stdin 读取请求；读到 `end` 即返回，后续行留给 `quit` 线程。
pub fn read_request(reader: &mut impl BufRead) -> Result<Request, String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("读取视频请求失败：{error}"))?;
        if read == 0 {
            return Err("视频请求不完整".to_owned());
        }
        if line.len() > MAX_REQUEST_LINE_BYTES {
            return Err("视频请求字段过长".to_owned());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        lines.push(trimmed.to_owned());
        if trimmed == "end" {
            break;
        }
        if lines.len() > MAX_REQUEST_LINES {
            return Err("视频请求字段过多".to_owned());
        }
    }
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    parse_request(borrowed)
}

/// np-video 回给 TUI 的状态行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Ready,
    Closed,
    Error(String),
    /// 控制条本地暂停 / 继续；TUI 只更新状态，不回发 pause。
    Paused(bool),
    /// 控制条本地音量；TUI 更新自身音量态，不回发 volume。
    Volume(u8),
    /// 控制条刷新：TUI 用当前 qn 重新取流并发回 `load`。
    Refresh,
    /// 控制条选了清晰度：TUI 用该 qn 重新取流并发回 `load`。
    Quality(u64),
}

impl Event {
    pub fn line(&self) -> String {
        match self {
            Self::Ready => "ready".to_owned(),
            Self::Closed => "closed".to_owned(),
            Self::Error(message) => format!("error {}", sanitize_message(message)),
            Self::Paused(paused) => format!("control paused {}", u8::from(*paused)),
            Self::Volume(volume) => format!("volume {}", (*volume).min(100)),
            Self::Refresh => "control refresh".to_owned(),
            Self::Quality(qn) => format!("control quality {qn}"),
        }
    }
}

fn sanitize_message(message: &str) -> String {
    let mut cleaned: String = message
        .chars()
        .map(|character| match character {
            '\r' | '\n' | '\0' => ' ',
            other => other,
        })
        .take(MAX_EVENT_MESSAGE_CHARS)
        .collect();
    if cleaned.trim().is_empty() {
        cleaned = "视频窗口异常".to_owned();
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    const URL: &str = "https://example.invalid/live.flv?token=secret";
    const REFERER: &str = "https://live.bilibili.com/";
    const ORIGIN: &str = "https://live.bilibili.com";

    fn request_lines() -> Vec<String> {
        vec![
            "v2".to_owned(),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
            "title BLAST 直播".to_owned(),
            "volume 65".to_owned(),
            "paused 1".to_owned(),
            "end".to_owned(),
        ]
    }

    #[test]
    fn parses_a_complete_request() {
        let lines = request_lines();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("valid request");
        assert_eq!(request.url, URL);
        assert_eq!(request.referer, REFERER);
        assert_eq!(request.origin, ORIGIN);
        assert_eq!(request.title, "BLAST 直播");
        assert_eq!(request.volume, 65);
        assert!(request.paused);
        assert_eq!(request.buffer_ms, DEFAULT_BUFFER_MS);
        assert_eq!(request.display_title(), "BLAST 直播");
    }

    #[test]
    fn buffer_target_defaults_to_900_and_explicit_values_win() {
        // 旧 TUI 不发 buffer_ms：缺省 900，和 `live.buffer_ms` 默认一致。
        let lines = request_lines();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(parse_request(refs).expect("default buffer").buffer_ms, 900);

        let mut lines = request_lines();
        lines.insert(6, "buffer_ms 1200".to_owned());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(
            parse_request(refs).expect("explicit buffer").buffer_ms,
            1_200
        );

        // 与 live.buffer_ms 的 500–2000 设置范围一致；越界直接拒绝。
        for bad in ["0", "499", "2001", "abc"] {
            let mut lines = request_lines();
            lines.insert(6, format!("buffer_ms {bad}"));
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            assert!(
                parse_request(refs).is_err(),
                "buffer_ms {bad} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_crlf_and_extra_unknown_fields() {
        let mut lines = request_lines();
        lines.insert(4, "future x".to_owned());
        let crlf: Vec<String> = lines.iter().map(|line| format!("{line}\r")).collect();
        let refs: Vec<&str> = crlf.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("crlf request");
        assert_eq!(request.title, "BLAST 直播");
    }

    #[test]
    fn empty_title_falls_back_to_live() {
        let lines = [
            "v2".to_owned(),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
            "title ".to_owned(),
            "end".to_owned(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("empty title");
        assert_eq!(request.display_title(), "直播");
    }

    #[test]
    fn rejects_wrong_version_and_missing_fields() {
        let mut lines = request_lines();
        lines[0] = "v3".to_owned();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("版本"));

        let lines = ["v2".to_owned(), "end".to_owned()];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("url"));
    }

    #[test]
    fn rejects_missing_end_and_duplicate_fields() {
        let lines = [
            "v2".to_owned(),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("结束标记"));

        let lines = [
            "v2".to_owned(),
            format!("url {URL}"),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
            "end".to_owned(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("重复"));
    }

    #[test]
    fn parses_quality_list_and_current_qn() {
        let mut lines = request_lines();
        lines.insert(7, "quality 10000 原画".to_owned());
        lines.insert(8, "quality 400 蓝光".to_owned());
        lines.insert(9, "quality 250 超清".to_owned());
        lines.insert(10, "current_qn 250".to_owned());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("quality request");
        assert_eq!(
            request.qualities,
            vec![
                (10000, "原画".to_owned()),
                (400, "蓝光".to_owned()),
                (250, "超清".to_owned())
            ]
        );
        assert_eq!(request.current_qn, Some(250));
    }

    #[test]
    fn old_requests_without_quality_fields_stay_compatible() {
        let lines = request_lines();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("legacy request");
        assert!(request.qualities.is_empty());
        assert_eq!(request.current_qn, None);
        // 旧 TUI 不发 theme：宿主回退默认暗色，不报错。
        assert_eq!(request.theme, None);
    }

    #[test]
    fn theme_line_round_trips_with_nine_hex_colors() {
        let theme = Theme::default();
        let line = theme.line();
        assert!(line.starts_with("theme "));
        assert_eq!(line.split_whitespace().count(), 10);
        assert_eq!(
            Theme::parse(line.trim_start_matches("theme ")).expect("parse"),
            theme
        );
        assert_eq!(
            line,
            "theme #ffb454 #14171c #1a1e24 #e9e4da #a7a49e #7d7c78 #1e2023 #ff8b7d #0d0f12"
        );
    }

    #[test]
    fn malformed_theme_lines_are_rejected_without_panicking() {
        for bad in [
            "",
            "#ffffff",
            "theme #ffffff",
            "#fff #14171c #1a1e24 #e9e4da #a7a49e #7d7c78 #1e2023 #ff8b7d #0d0f12",
            "#gggggg #14171c #1a1e24 #e9e4da #a7a49e #7d7c78 #1e2023 #ff8b7d #0d0f12",
            "#ff0000 #00ff00 #0000ff #ffffff #000000 #111111 #222222 #333333 #444444 #555555",
        ] {
            assert!(Theme::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn request_theme_is_optional_and_invalid_lines_keep_the_previous_theme() {
        let theme = Theme::default();
        let mut lines = request_lines();
        lines.insert(4, theme.line());
        // 再插一行非法 theme：整行拒绝，保留上一份合法主题。
        lines.insert(5, "theme #ff0000".to_owned());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let request = parse_request(refs).expect("request with theme");
        assert_eq!(request.theme, Some(theme));
    }

    #[test]
    fn rejects_bad_quality_fields_without_echoing_labels() {
        for bad in [
            "quality 10000",
            "quality abc 原画",
            "quality 10000 ",
            "quality 0 原画",
            "quality 10000 原\n画",
            "current_qn abc",
            "current_qn 0",
        ] {
            let mut lines = request_lines();
            lines.insert(7, bad.to_owned());
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            let error = parse_request(refs).expect_err(bad);
            assert!(
                !error.contains("原画") && !error.contains("secret"),
                "{error} must not echo labels or urls"
            );
        }

        // 重复档位直接拒绝。
        let mut lines = request_lines();
        lines.insert(7, "quality 400 蓝光".to_owned());
        lines.insert(8, "quality 400 超清".to_owned());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("重复"));
    }

    #[test]
    fn rejects_unsafe_values_without_echoing_the_url() {
        for url in ["ftp://example.invalid/x", "#evil", "https://e.invalid/\nX"] {
            let request = Request {
                url: url.to_owned(),
                referer: REFERER.to_owned(),
                origin: ORIGIN.to_owned(),
                title: "x".to_owned(),
                volume: 100,
                paused: false,
                buffer_ms: DEFAULT_BUFFER_MS,
                theme: None,
                qualities: Vec::new(),
                current_qn: None,
            };
            let error = request.validate().unwrap_err();
            assert!(
                !error.contains("secret")
                    && !error.contains("evil")
                    && !error.contains("e.invalid"),
                "{error}"
            );
        }

        let lines = [
            "v2".to_owned(),
            format!("url {URL}"),
            "referer https://live.bilibili.com/,X-Evil: 1".to_owned(),
            format!("origin {ORIGIN}"),
            "end".to_owned(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("逗号"));
        assert!(validate_stream_url("https://example.invalid/live.flv").is_ok());
        assert!(validate_stream_url("https://e.invalid/\nX").is_err());
    }

    #[test]
    fn read_request_stops_at_end_and_leaves_quit_unread() {
        let mut input = BufReader::new(std::io::Cursor::new(format!(
            "v2\nurl {URL}\nreferer {REFERER}\norigin {ORIGIN}\ntitle 房间\nend\nquit\n"
        )));
        let request = read_request(&mut input).expect("request");
        assert_eq!(request.title, "房间");
        let mut rest = String::new();
        input.read_line(&mut rest).expect("read rest");
        assert_eq!(rest.trim(), "quit");
    }

    #[test]
    fn event_lines_are_single_line_and_sanitized() {
        assert_eq!(Event::Ready.line(), "ready");
        assert_eq!(Event::Closed.line(), "closed");
        assert_eq!(Event::Paused(true).line(), "control paused 1");
        assert_eq!(Event::Paused(false).line(), "control paused 0");
        assert_eq!(Event::Volume(65).line(), "volume 65");
        assert_eq!(Event::Volume(250).line(), "volume 100");
        assert_eq!(Event::Refresh.line(), "control refresh");
        assert_eq!(Event::Quality(400).line(), "control quality 400");
        let error = Event::Error("坏\n消息".to_owned()).line();
        assert_eq!(error, "error 坏 消息");
        assert!(!error.contains('\n'));
        let empty = Event::Error("   ".to_owned()).line();
        assert_eq!(empty, "error 视频窗口异常");
    }
}
