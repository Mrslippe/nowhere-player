//! IPC 命令层（开发方案 §4.3）。
//!
//! 这里只做参数转换与转发：不碰 `Player`、不做阻塞逻辑、不 panic。
//! 成功返回完整 [`PlaybackState`]；失败返回 `{ code, message }`（[`PlayerError`]）。
//!
//! 通道名带点（如 `player.snapshot`）；Tauri 命令参数默认 camelCase，
//! 因此 `position_ms` 在前端写作 `positionMs`。

use np_core::{PlaybackState, PlayerError};
use tauri::State;

use crate::player_actor::PlayerHandle;

/// `player.snapshot`：取首帧状态。
#[tauri::command(rename = "player.snapshot")]
pub(crate) fn player_snapshot(
    handle: State<'_, PlayerHandle>,
) -> Result<PlaybackState, PlayerError> {
    handle.snapshot()
}

/// `player.play`：带 `path` 打开并播放；不带则继续当前曲目。
#[tauri::command(rename = "player.play")]
pub(crate) fn player_play(
    handle: State<'_, PlayerHandle>,
    path: Option<String>,
) -> Result<PlaybackState, PlayerError> {
    handle.play(path)
}

/// `player.pause`：暂停。
#[tauri::command(rename = "player.pause")]
pub(crate) fn player_pause(handle: State<'_, PlayerHandle>) -> Result<PlaybackState, PlayerError> {
    handle.pause()
}

/// `player.next`：无队列，固定返回 `E_UNSUPPORTED`。
#[tauri::command(rename = "player.next")]
pub(crate) fn player_next(handle: State<'_, PlayerHandle>) -> Result<PlaybackState, PlayerError> {
    handle.next()
}

/// `player.prev`：无队列，固定返回 `E_UNSUPPORTED`。
#[tauri::command(rename = "player.prev")]
pub(crate) fn player_prev(handle: State<'_, PlayerHandle>) -> Result<PlaybackState, PlayerError> {
    handle.prev()
}

/// `player.seek`：跳转到 `positionMs`。
#[tauri::command(rename = "player.seek")]
pub(crate) fn player_seek(
    handle: State<'_, PlayerHandle>,
    position_ms: u64,
) -> Result<PlaybackState, PlayerError> {
    handle.seek(position_ms)
}

/// `player.volume`：设置音量（0.0–1.0）。
#[tauri::command(rename = "player.volume")]
pub(crate) fn player_volume(
    handle: State<'_, PlayerHandle>,
    volume: f32,
) -> Result<PlaybackState, PlayerError> {
    handle.volume(volume)
}
