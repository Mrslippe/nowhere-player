//! TUI → np-video 的一次性启动请求，以及 np-video → TUI 的状态行。
//!
//! 请求走 stdin，URL / header 不进入命令行（进程列表里看不到）。格式是行协议，
//! 便于人工调试，也避免在 np-video 里再引入 serde：
//!
//! ```text
//! v1
//! url https://...
//! referer https://live.bilibili.com/
//! origin https://live.bilibili.com
//! title 房间标题
//! end
//! ```
//!
//! 之后的 stdin 只用于 `quit`：TUI 关闭视频时先发 `quit`，np-video 再杀 mpv。

use std::io::BufRead;

/// 请求版本；TUI 与 np-video 必须一致。
pub const VERSION: &str = "v1";
const MAX_REQUEST_LINES: usize = 64;
const MAX_REQUEST_LINE_BYTES: usize = 16 * 1024;
const MAX_EVENT_MESSAGE_CHARS: usize = 300;

/// 启动一次嵌入播放所需的全部信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub referer: String,
    pub origin: String,
    pub title: String,
}

impl Request {
    /// 校验来自 TUI 的字段；错误文本不回显字段值（URL 可能带 token）。
    pub fn validate(&self) -> Result<(), String> {
        validate_url(&self.url)?;
        validate_header(&self.referer, "Referer")?;
        validate_header(&self.origin, "Origin")?;
        validate_text(&self.title, "标题")?;
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
}

impl Event {
    pub fn line(&self) -> String {
        match self {
            Self::Ready => "ready".to_owned(),
            Self::Closed => "closed".to_owned(),
            Self::Error(message) => format!("error {}", sanitize_message(message)),
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
            "v1".to_owned(),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
            "title BLAST 直播".to_owned(),
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
        assert_eq!(request.display_title(), "BLAST 直播");
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
            "v1".to_owned(),
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
        lines[0] = "v2".to_owned();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("版本"));

        let lines = ["v1".to_owned(), "end".to_owned()];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("url"));
    }

    #[test]
    fn rejects_missing_end_and_duplicate_fields() {
        let lines = [
            "v1".to_owned(),
            format!("url {URL}"),
            format!("referer {REFERER}"),
            format!("origin {ORIGIN}"),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("结束标记"));

        let lines = [
            "v1".to_owned(),
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
    fn rejects_unsafe_values_without_echoing_the_url() {
        for url in ["ftp://example.invalid/x", "#evil", "https://e.invalid/\nX"] {
            let request = Request {
                url: url.to_owned(),
                referer: REFERER.to_owned(),
                origin: ORIGIN.to_owned(),
                title: "x".to_owned(),
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
            "v1".to_owned(),
            format!("url {URL}"),
            "referer https://live.bilibili.com/,X-Evil: 1".to_owned(),
            format!("origin {ORIGIN}"),
            "end".to_owned(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(parse_request(refs).unwrap_err().contains("逗号"));
    }

    #[test]
    fn read_request_stops_at_end_and_leaves_quit_unread() {
        let mut input = BufReader::new(std::io::Cursor::new(format!(
            "v1\nurl {URL}\nreferer {REFERER}\norigin {ORIGIN}\ntitle 房间\nend\nquit\n"
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
        let error = Event::Error("坏\n消息".to_owned()).line();
        assert_eq!(error, "error 坏 消息");
        assert!(!error.contains('\n'));
        let empty = Event::Error("   ".to_owned()).line();
        assert_eq!(empty, "error 视频窗口异常");
    }
}
