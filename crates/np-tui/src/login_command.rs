//! TUI-only `/login bili` / `/logout bili`。

use crate::audio::TuiAudio;

pub(crate) fn run(input: &str, player: &TuiAudio) -> Option<String> {
    let trimmed = input.trim();
    let command = trimmed
        .strip_prefix(':')
        .or_else(|| trimmed.strip_prefix('/'))
        .unwrap_or(trimmed);
    let mut parts = command.split_whitespace();
    let action = parts.next()?;
    let scope = parts.next();
    if parts.next().is_some() {
        return None;
    }
    match (action, scope) {
        ("login", Some("bili")) => {
            player.start_bili_login();
            Some("B站登录：正在生成二维码".to_owned())
        }
        ("logout", Some("bili")) => Some(player.start_bili_logout()),
        ("login", None) | ("logout", None) => Some("用法：login bili / logout bili".to_owned()),
        _ => None,
    }
}
