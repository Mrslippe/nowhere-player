//! IPC 命令层（开发方案 §4.3）。
//!
//! 这里只做参数转换与转发：不碰 `Player`、不做阻塞逻辑、不 panic。
//! 成功返回完整 [`PlaybackState`]（或队列命令的 [`QueueView`]）；
//! 失败返回 `{ code, message }`（[`PlayerError`]）。
//!
//! 通道名带点（如 `player.snapshot`）；Tauri 命令参数默认 camelCase，
//! 因此 `position_ms` 在前端写作 `positionMs`。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use np_core::theme::{
    BUILTIN_THEMES, BuiltinTheme, E_THEME_ID, E_THEME_IO, E_THEME_JSON, E_THEME_NOT_FOUND,
    RawTheme, ThemeApplyReply, ThemeError, ThemeListEntry, ThemeSource, ThemeSpec,
};
use np_core::{PlaybackMode, PlaybackState, PlayerError, QueueView};
use np_library::{Stats, Track};
use serde::de::DeserializeOwned;
use tauri::{AppHandle, Manager, State};

use crate::command_exec::{
    CmdErrorPayload, LibraryApi, LibraryHandle, LibraryObserver, MAX_SEARCH_RESULTS,
    NoopLibraryObserver, ScanAcceptedReply, emit_block, event_observer_for_app, scan_started_block,
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
            event_observer_for_app(app.clone(), block_id.clone(), "library scan".to_owned())
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

/// `theme.list`：列出内置 + 用户主题；同名用户主题覆盖内置。
#[tauri::command(rename = "theme.list")]
pub(crate) fn theme_list(app: AppHandle) -> Result<Vec<ThemeListEntry>, ThemeError> {
    let dir = themes_dir(&app)?;
    Ok(list_themes(&dir))
}

/// `theme.apply`：按 id 取主题；用户文件优先，其次内置。
#[tauri::command(rename = "theme.apply")]
pub(crate) fn theme_apply(app: AppHandle, id: String) -> Result<ThemeApplyReply, ThemeError> {
    let dir = themes_dir(&app)?;
    apply_theme(&dir, &id)
}

/// `theme.initial`：启动时从 settings 读回 `theme.id`；主题文件已失效则清掉并回落默认。
#[tauri::command(rename = "theme.initial")]
pub(crate) fn theme_initial(
    app: AppHandle,
    library: State<'_, LibraryHandle>,
) -> Result<Option<String>, CmdErrorPayload> {
    let Some(id) = library.settings().theme_id() else {
        return Ok(None);
    };
    let dir = themes_dir(&app).map_err(|error| CmdErrorPayload {
        code: error.code,
        message: error.message,
        hint: error.hint,
    })?;
    match apply_theme(&dir, &id) {
        Ok(_) => Ok(Some(id)),
        Err(_) => {
            let _ = library.settings().unset_and_save("theme.id");
            Ok(None)
        }
    }
}

/// `theme.exportBuiltin`：把内置主题导出成 `<appdata>/themes/<id>.json` 样例。
///
/// 已存在同名文件时不覆盖（用户可能正在改它），返回 `created: false`。
#[tauri::command(rename = "theme.exportBuiltin")]
pub(crate) fn theme_export_builtin(
    app: AppHandle,
    id: String,
) -> Result<ThemeExportReply, ThemeError> {
    let dir = themes_dir(&app)?;
    export_builtin(&dir, &id)
}

/// `theme.exportBuiltin` 的回执。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeExportReply {
    /// 导出文件的绝对路径。
    pub(crate) path: String,
    /// `true` 表示本次新写入；`false` 表示文件已存在、未覆盖。
    pub(crate) created: bool,
}

/// 用户主题目录：`<appdata>/themes`（不存在时按需创建，不在这里建）。
const THEMES_DIR: &str = "themes";

pub(crate) fn themes_dir(app: &AppHandle) -> Result<PathBuf, ThemeError> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| ThemeError::new(E_THEME_IO, format!("无法定位应用数据目录：{error}")))?;
    Ok(base.join(THEMES_DIR))
}

/// 用 Tauri 公开的响应反序列化完成 `&str -> T`。
///
/// np-core 不引入 `serde_json`（依赖红线：不得新增第三方依赖，且 np-core 的
/// Cargo.toml 不在本卡授权路径内），而 Tauri 的 `InvokeResponseBody::deserialize`
/// 正好是公开 API、内部就是 `serde_json::from_str`。这里借它解析用户主题文件。
fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    tauri::ipc::InvokeResponseBody::Json(text.to_owned())
        .deserialize::<T>()
        .map_err(|error| error.to_string())
}

/// 列出内置 + 用户主题；同名时用户主题覆盖内置（便于就地改内置主题）。
pub(crate) fn list_themes(dir: &Path) -> Vec<ThemeListEntry> {
    let mut entries: BTreeMap<String, ThemeListEntry> = BTreeMap::new();

    for theme in BUILTIN_THEMES.iter() {
        entries.insert(theme.id.to_owned(), theme.entry());
    }
    for entry in user_theme_entries(dir) {
        entries.insert(entry.id.clone(), entry);
    }

    entries.into_values().collect()
}

fn user_theme_entries(dir: &Path) -> Vec<ThemeListEntry> {
    let mut entries = Vec::new();

    for path in theme_files(dir) {
        match load_user_theme(&path) {
            Ok(spec) => entries.push(ThemeListEntry {
                id: spec.id,
                name: spec.name,
                skin: spec.skin,
                source: ThemeSource::User,
            }),
            // 单个坏文件不影响其它主题：跳过并留一条 stderr，供排查。
            Err(error) => eprintln!("[theme] 跳过 {}：{error}", path.display()),
        }
    }

    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

pub(crate) fn apply_theme(dir: &Path, id: &str) -> Result<ThemeApplyReply, ThemeError> {
    let id = id.trim();
    // 先校验再拼路径：id 里有 `..` / 分隔符时连文件都不碰。
    np_core::theme::validate_theme_id(id)?;

    let user_path = dir.join(format!("{id}.json"));
    if user_path.is_file() {
        let spec = load_user_theme(&user_path)?;
        return Ok(ThemeApplyReply::from_spec(spec, ThemeSource::User));
    }

    if let Some(theme) = BuiltinTheme::find(id) {
        return Ok(ThemeApplyReply::from_builtin(theme));
    }

    Err(
        ThemeError::new(E_THEME_NOT_FOUND, format!("主题不存在：{id}"))
            .with_hint("用 theme.list 查看可用主题，或把 <id>.json 放进 themes 目录"),
    )
}

pub(crate) fn export_builtin(dir: &Path, id: &str) -> Result<ThemeExportReply, ThemeError> {
    let id = id.trim();
    let theme = BuiltinTheme::find(id).ok_or_else(|| {
        ThemeError::new(E_THEME_NOT_FOUND, format!("没有内置主题：{id}"))
            .with_hint("可用内置主题：graphite / amber / ice / phosphor")
    })?;

    fs::create_dir_all(dir)
        .map_err(|error| ThemeError::new(E_THEME_IO, format!("无法创建主题目录：{error}")))?;

    let path = dir.join(format!("{}.json", theme.id));
    let path_text = path.to_string_lossy().into_owned();

    if path.is_file() {
        return Ok(ThemeExportReply {
            path: path_text,
            created: false,
        });
    }

    fs::write(&path, theme.to_spec().to_json_pretty())
        .map_err(|error| ThemeError::new(E_THEME_IO, format!("写入主题文件失败：{error}")))?;

    Ok(ThemeExportReply {
        path: path_text,
        created: true,
    })
}

/// 读出并校验一个用户主题文件；文件名必须是 `<id>.json`。
fn load_user_theme(path: &Path) -> Result<ThemeSpec, ThemeError> {
    let text = fs::read_to_string(path)
        .map_err(|error| ThemeError::new(E_THEME_IO, format!("读取主题文件失败：{error}")))?;
    let raw: RawTheme = parse_json(&text)
        .map_err(|error| ThemeError::new(E_THEME_JSON, format!("主题 JSON 解析失败：{error}")))?;
    let spec = ThemeSpec::from_raw(raw)?;

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if stem != spec.id {
        return Err(ThemeError::new(
            E_THEME_ID,
            format!("文件名 {stem}.json 与主题 id {} 不一致", spec.id),
        )
        .with_hint("文件名必须是 <id>.json，id 以文件内 id 为准"));
    }

    Ok(spec)
}

/// 主题目录下的 `*.json` 文件（按路径排序，保证 list 稳定）。
fn theme_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = read_dir
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    files.sort();
    files
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
    use std::fs;
    use std::path::PathBuf;

    use np_core::theme::{BuiltinTheme, E_THEME_ID, E_THEME_NOT_FOUND, ThemeSource, ThemeTokens};

    use super::{apply_theme, cover_file_name, export_builtin, list_themes};

    /// 每个用例一个独立临时目录（同名会互相踩，用 tag 区分）。
    fn temp_theme_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("np-theme-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时主题目录");
        dir
    }

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
        assert_eq!(__tauri_command_name_theme_list!(), "theme.list");
        assert_eq!(__tauri_command_name_theme_apply!(), "theme.apply");
        assert_eq!(
            __tauri_command_name_theme_export_builtin!(),
            "theme.exportBuiltin"
        );
    }

    #[test]
    fn apply_theme_returns_builtin_tokens() {
        let dir = temp_theme_dir("builtin");

        let reply = apply_theme(&dir, "phosphor").expect("内置磷光主题");

        assert_eq!(reply.source, ThemeSource::Builtin);
        assert_eq!(reply.skin.as_str(), "phosphor");
        assert_eq!(reply.tokens.fg, "#ffb454");
        assert_eq!(reply.tokens.bg, "#0a0806");
        assert!(reply.warnings.is_empty());
    }

    #[test]
    fn apply_theme_rejects_unknown_and_traversal_ids() {
        let dir = temp_theme_dir("reject");

        let missing = apply_theme(&dir, "nope").expect_err("未知主题");
        assert_eq!(missing.code, E_THEME_NOT_FOUND);

        for id in ["../escape", "a/b", ".."] {
            let error = apply_theme(&dir, id).expect_err(id);
            assert_eq!(error.code, E_THEME_ID, "id 被接受：{id:?}");
        }

        // 确认没有在主题目录之外留下任何探测文件。
        assert!(!dir.join("../escape.json").exists());
    }

    #[test]
    fn export_builtin_writes_a_reloadable_sample() {
        let dir = temp_theme_dir("export");

        let first = export_builtin(&dir, "amber").expect("导出琥珀");

        assert!(first.created, "首次导出应写入文件");
        assert!(first.path.ends_with("amber.json"));
        let text = fs::read_to_string(&first.path).expect("读回导出的主题");
        assert!(text.contains("\"id\": \"amber\""), "导出内容：{text}");

        // 导出后该 id 变成用户主题（用户文件优先），token 与内置一致。
        let applied = apply_theme(&dir, "amber").expect("应用导出的主题");
        assert_eq!(applied.source, ThemeSource::User);
        assert_eq!(
            applied.tokens,
            BuiltinTheme::find("amber")
                .expect("内置琥珀")
                .to_spec()
                .tokens
        );

        let second = export_builtin(&dir, "amber").expect("再次导出");
        assert!(!second.created, "已存在的文件不能被覆盖");
        assert_eq!(second.path, first.path);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_themes_merges_builtin_and_user_files() {
        let dir = temp_theme_dir("list");
        fs::write(
            dir.join("graphite.json"),
            r##"{"id":"graphite","name":"我的石墨","contract":1,"skin":"phosphor","tokens":{"bg":"#000000"}}"##,
        )
        .expect("写用户主题");
        fs::write(dir.join("broken.json"), "{ 这不是 JSON").expect("写坏文件");

        let entries = list_themes(&dir);

        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "amber",
                "graphite",
                "ice",
                "phosphor",
                "tui-block",
                "tui-paper",
                "tui-wire"
            ]
        );

        let graphite = entries
            .iter()
            .find(|entry| entry.id == "graphite")
            .expect("graphite");
        assert_eq!(graphite.source, ThemeSource::User);
        assert_eq!(graphite.name, "我的石墨");
        assert_eq!(graphite.skin.as_str(), "phosphor");
        // 坏文件被跳过，不破坏列表。
        assert!(entries.iter().all(|entry| entry.id != "broken"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_theme_missing_tokens_are_completed_with_graphite() {
        let dir = temp_theme_dir("partial");
        fs::write(
            dir.join("mine.json"),
            r##"{"id":"mine","contract":1,"tokens":{"bg":"#101010","line":"not-a-color"}}"##,
        )
        .expect("写用户主题");

        let reply = apply_theme(&dir, "mine").expect("应用用户主题");

        assert_eq!(reply.source, ThemeSource::User);
        assert_eq!(reply.tokens.bg, "#101010");
        assert_eq!(reply.tokens.line, ThemeTokens::default().line);
        assert_eq!(reply.tokens.fg, ThemeTokens::default().fg);
        assert!(
            reply
                .warnings
                .iter()
                .any(|warning| warning.contains("line"))
        );

        let _ = fs::remove_dir_all(&dir);
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
