//! IPC 命令层（开发方案 §4.3）。
//!
//! 这里只做参数转换与转发：不碰 `Player`、不做阻塞逻辑、不 panic。
//! 成功返回完整 [`PlaybackState`]（或队列命令的 [`QueueView`]）；
//! 失败返回 `{ code, message }`（[`PlayerError`]）。
//!
//! 通道名带点（如 `player.snapshot`）；Tauri 命令参数默认 camelCase，
//! 因此 `position_ms` 在前端写作 `positionMs`。

use std::sync::Arc;

use np_core::{PlaybackMode, PlaybackState, PlayerError, QueueView};
use np_library::{Stats, Track};
use tauri::{AppHandle, State};

use crate::command_exec::{
    CmdErrorPayload, EventObserver, LibraryApi, LibraryHandle, LibraryObserver, MAX_SEARCH_RESULTS,
    NoopLibraryObserver, ScanAcceptedReply, emit_block, scan_started_block,
};
use crate::player_actor::PlayerHandle;

/// `player.snapshot`：取首帧状态。
#[tauri::command(rename = "player.snapshot")]
pub(crate) fn player_snapshot(
    handle: State<'_, PlayerHandle>,
) -> Result<PlaybackState, PlayerError> {
    handle.snapshot()
}

/// `player.play`：带 `path` 打开并播放（同时入队）；不带则继续当前曲目。
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

/// `player.next`：下一首；队列为空时回 `E_UNSUPPORTED`。
#[tauri::command(rename = "player.next")]
pub(crate) fn player_next(handle: State<'_, PlayerHandle>) -> Result<PlaybackState, PlayerError> {
    handle.next()
}

/// `player.prev`：上一首；队列为空时回 `E_UNSUPPORTED`。
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

/// `player.mode`：设置 `repeat` / `shuffle`（`mode` 为 `{ repeat, shuffle }`）。
#[tauri::command(rename = "player.mode")]
pub(crate) fn player_mode(
    handle: State<'_, PlayerHandle>,
    mode: PlaybackMode,
) -> Result<PlaybackState, PlayerError> {
    handle.mode(mode)
}

/// `queue.list`：取队列快照（按播放顺序）。
#[tauri::command(rename = "queue.list")]
pub(crate) fn queue_list(handle: State<'_, PlayerHandle>) -> Result<QueueView, PlayerError> {
    handle.queue_list()
}

/// `queue.add`：按给定顺序追加本地文件路径。
#[tauri::command(rename = "queue.add")]
pub(crate) fn queue_add(
    handle: State<'_, PlayerHandle>,
    paths: Vec<String>,
) -> Result<QueueView, PlayerError> {
    handle.queue_add(paths)
}

/// `queue.remove`：移除播放顺序里的第 `index` 首。
#[tauri::command(rename = "queue.remove")]
pub(crate) fn queue_remove(
    handle: State<'_, PlayerHandle>,
    index: usize,
) -> Result<QueueView, PlayerError> {
    handle.queue_remove(index)
}

/// `queue.clear`：清空队列并回到 `idle`。
#[tauri::command(rename = "queue.clear")]
pub(crate) fn queue_clear(handle: State<'_, PlayerHandle>) -> Result<QueueView, PlayerError> {
    handle.queue_clear()
}

/// `queue.jump`：跳到播放顺序里的第 `index` 首并播放。
#[tauri::command(rename = "queue.jump")]
pub(crate) fn queue_jump(
    handle: State<'_, PlayerHandle>,
    index: usize,
) -> Result<QueueView, PlayerError> {
    handle.queue_jump(index)
}

/// `library.addRoot`：登记扫描根（绝对化后幂等写入），返回当前根数量。
#[tauri::command(rename = "library.addRoot")]
pub(crate) fn library_add_root(
    library: State<'_, LibraryHandle>,
    path: String,
) -> Result<usize, CmdErrorPayload> {
    library.add_root(&path).map_err(CmdErrorPayload::from)
}

/// `library.scan`：受理后台扫描；给了 `blockId` 时用 `terminal:block` 推进度。
#[tauri::command(rename = "library.scan")]
pub(crate) fn library_scan(
    app: AppHandle,
    library: State<'_, LibraryHandle>,
    block_id: Option<String>,
    full: Option<bool>,
) -> Result<ScanAcceptedReply, CmdErrorPayload> {
    let full = full.unwrap_or(false);
    let observer: Arc<dyn LibraryObserver> = match &block_id {
        Some(block_id) => {
            // 与 cmd.exec 一样，先推 running 再启动线程，避免 done 抢跑。
            emit_block(&app, &scan_started_block(block_id, "library scan", full));
            Arc::new(EventObserver::for_app(
                app.clone(),
                block_id.clone(),
                "library scan".to_owned(),
            ))
        }
        None => Arc::new(NoopLibraryObserver),
    };

    if let Err(error) = library.scan(full, Arc::clone(&observer)) {
        // 已推的 running 块不能永远停在 running：失败时用同一 id 收尾。
        observer.on_failed(&error);
        return Err(CmdErrorPayload::from(error));
    }
    Ok(ScanAcceptedReply { accepted: true })
}

/// `library.list`：最近入库的曲目，默认 20 条。
#[tauri::command(rename = "library.list")]
pub(crate) fn library_list(
    library: State<'_, LibraryHandle>,
    limit: Option<usize>,
) -> Result<Vec<Track>, CmdErrorPayload> {
    let limit = limit.unwrap_or(20).max(1);
    library.list_recent(limit).map_err(CmdErrorPayload::from)
}

/// `library.search`：库检索，结果同时存进会话上下文供 `play <n>` 使用。
#[tauri::command(rename = "library.search")]
pub(crate) fn library_search(
    library: State<'_, LibraryHandle>,
    keyword: String,
    limit: Option<usize>,
) -> Result<Vec<Track>, CmdErrorPayload> {
    if keyword.trim().is_empty() {
        return Err(CmdErrorPayload {
            code: "E_CMD_ARG".to_owned(),
            message: "搜索关键词不能为空".to_owned(),
            hint: None,
        });
    }
    let limit = limit
        .unwrap_or(MAX_SEARCH_RESULTS)
        .clamp(1, MAX_SEARCH_RESULTS);
    let tracks = library
        .search(&keyword, limit)
        .map_err(CmdErrorPayload::from)?;
    library.remember_search(tracks.clone());
    Ok(tracks)
}

/// `library.stats`：曲目 / 专辑 / 艺人 / 总时长 / 最近扫描。
#[tauri::command(rename = "library.stats")]
pub(crate) fn library_stats(library: State<'_, LibraryHandle>) -> Result<Stats, CmdErrorPayload> {
    library.stats().map_err(CmdErrorPayload::from)
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_names_keep_the_dotted_contract() {
        assert_eq!(__tauri_command_name_player_snapshot!(), "player.snapshot");
        assert_eq!(__tauri_command_name_player_play!(), "player.play");
        assert_eq!(__tauri_command_name_player_pause!(), "player.pause");
        assert_eq!(__tauri_command_name_player_next!(), "player.next");
        assert_eq!(__tauri_command_name_player_prev!(), "player.prev");
        assert_eq!(__tauri_command_name_player_seek!(), "player.seek");
        assert_eq!(__tauri_command_name_player_volume!(), "player.volume");
        assert_eq!(__tauri_command_name_player_mode!(), "player.mode");
        assert_eq!(__tauri_command_name_queue_list!(), "queue.list");
        assert_eq!(__tauri_command_name_queue_add!(), "queue.add");
        assert_eq!(__tauri_command_name_queue_remove!(), "queue.remove");
        assert_eq!(__tauri_command_name_queue_clear!(), "queue.clear");
        assert_eq!(__tauri_command_name_queue_jump!(), "queue.jump");
        assert_eq!(__tauri_command_name_library_add_root!(), "library.addRoot");
        assert_eq!(__tauri_command_name_library_scan!(), "library.scan");
        assert_eq!(__tauri_command_name_library_list!(), "library.list");
        assert_eq!(__tauri_command_name_library_search!(), "library.search");
        assert_eq!(__tauri_command_name_library_stats!(), "library.stats");
    }
}
