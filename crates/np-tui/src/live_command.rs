//! TUI 内部的 `live <room>` 入口；不注册到 `np-core` 命令表。

use crate::audio::TuiAudio;

/// 解析并启动 `live <room>`；返回要写入状态栏的文本。
pub(crate) fn start_live_command(input: &str, player: &TuiAudio) -> Option<String> {
    if let Some(status) = crate::mpv_command::run(input) {
        return Some(status);
    }
    let result = parse_live_command(input)?;
    Some(match result {
        Ok(room) => start_live(player, room),
        Err(message) => message,
    })
}

/// 统一开播入口：`/live` 命令与 Browse 直播页都走这里，只调播放器。
pub(crate) fn start_live(player: &TuiAudio, room: u64) -> String {
    player.start_live(room);
    format!("直播准备中：房间 {room}")
}

/// 解析 `live <room>` / `:live <room>`。
///
/// 只接受一个数字房间号；`search` / `play` 等命令返回 `None`。
pub(crate) fn parse_live_command(input: &str) -> Option<Result<u64, String>> {
    let trimmed = input.trim();
    let command = trimmed
        .strip_prefix(':')
        .or_else(|| trimmed.strip_prefix('/'))
        .unwrap_or(trimmed);
    let mut parts = command.split_whitespace();
    if parts.next() != Some("live") {
        return None;
    }
    let (Some(room), None) = (parts.next(), parts.next()) else {
        return Some(Err("用法：live <房间号>".to_owned()));
    };
    match room.parse::<u64>() {
        Ok(room) if room > 0 => Some(Ok(room)),
        _ => Some(Err(format!("房间号无效：{room}"))),
    }
}
