//! TUI 内部 `/mpv` 入口：下载应用托管的 mpv。

/// 解析 `mpv` / `/mpv`；返回状态栏文本。
pub(crate) fn run(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let command = trimmed
        .strip_prefix(':')
        .or_else(|| trimmed.strip_prefix('/'))
        .unwrap_or(trimmed);
    if command.split_whitespace().next() != Some("mpv") {
        return None;
    }
    if command.split_whitespace().nth(1).is_some() {
        return Some("用法：mpv".to_owned());
    }
    Some(match crate::mpv_install::start() {
        Ok(()) => "mpv 下载中…（完成后按 v 打开视频）".to_owned(),
        Err(error) => error,
    })
}
