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

/// `library.coverUrl`：把封面 key 换成前端可加载的绝对路径。
///
/// 只接受 16 位**小写**十六进制。含 `/`、`\`、`..`、空串、大写或长度不符
/// 一律在拼路径之前拒绝，避免路径穿越（T-0025 §6.1）。
#[tauri::command(rename = "library.coverUrl")]
pub(crate) fn library_cover_url(
    library: State<'_, LibraryHandle>,
    key: String,
) -> Result<String, CmdErrorPayload> {
    let file = cover_file_name(&key)?;
    library
        .covers_dir()
        .join(file)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CmdErrorPayload {
            code: "E_LIB_IO".to_owned(),
            message: "封面路径不是有效的 UTF-8".to_owned(),
            hint: None,
        })
}

/// `library.trackByPath`：播放器只持文件路径，经它回库取元数据（含 `coverKey`）。
#[tauri::command(rename = "library.trackByPath")]
pub(crate) fn library_track_by_path(
    library: State<'_, LibraryHandle>,
    path: String,
) -> Result<Option<Track>, CmdErrorPayload> {
    if path.is_empty() {
        return Err(CmdErrorPayload {
            code: "E_CMD_ARG".to_owned(),
            message: "曲目路径不能为空".to_owned(),
            hint: None,
        });
    }
    library.track_by_path(&path).map_err(CmdErrorPayload::from)
}

/// `cover_key` 的长度：16 位小写十六进制（ARCH-003 §3）。
const COVER_KEY_LEN: usize = 16;

/// 把封面 key 变成文件名；不合法时返回 `E_CMD_ARG`，且**不算出任何路径**。
fn cover_file_name(key: &str) -> Result<String, CmdErrorPayload> {
    let valid = key.len() == COVER_KEY_LEN
        && key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());

    if !valid {
        return Err(CmdErrorPayload {
            code: "E_CMD_ARG".to_owned(),
            message: "封面 key 非法".to_owned(),
            hint: Some("封面 key 必须是 16 位小写十六进制".to_owned()),
        });
    }

    Ok(format!("{key}.jpg"))
}

#[cfg(test)]
mod tests {
    use super::cover_file_name;

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
        assert_eq!(
            __tauri_command_name_library_cover_url!(),
            "library.coverUrl"
        );
        assert_eq!(
            __tauri_command_name_library_track_by_path!(),
            "library.trackByPath"
        );
    }

    #[test]
    fn cover_file_name_accepts_a_normalized_key() {
        assert_eq!(
            cover_file_name("0123456789abcdef").expect("valid key"),
            "0123456789abcdef.jpg"
        );
    }

    #[test]
    fn cover_file_name_rejects_traversal_and_odd_keys() {
        for key in [
            "",
            "..",
            "../../covers/x",
            "0123456789abcde/",
            "0123456789abcde\\",
            "0123456789abcde.",
            "0123456789ABCDEF",
            "0123456789abcdeF",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789abcde ",
            "0123456789abcdég",
        ] {
            let error = cover_file_name(key).expect_err(key);
            assert_eq!(error.code, "E_CMD_ARG", "key accepted: {key:?}");
        }
    }
}
