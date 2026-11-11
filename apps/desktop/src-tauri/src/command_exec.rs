//! Tauri 薄适配：把 AppHandle / PlayerHandle / LibraryHandle 接到 `np-app` 的端口上。
//!
//! 所有命令语义、搜索、队列、歌单、主题逻辑都在 `np_app::command`；这里只做
//! Tauri 事件、窗口/路径与应用状态的适配。

use std::path::PathBuf;
use std::sync::Arc;

use np_app::{
    CmdError, CmdExecReply, EventObserver, EventSink, Execution, PlaybackTarget, PlayerApi,
    Services, ThemeApi, ThemeExportTarget, complete_for, execute_with_services, normalize_cmd_dots,
    scan_full_flag,
};
use np_core::{Block, Completion, PlaybackState, ThemeApplyReply, ThemeError, ThemeListEntry};

// 让 lib.rs / ipc.rs 继续从 `command_exec` 取库句柄与共享类型。
pub use np_app::{
    CmdErrorPayload, LibraryApi, LibraryHandle, LibraryObserver, MAX_SEARCH_RESULTS,
    NoopLibraryObserver, ScanAcceptedReply, scan_started_block,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::player_actor::PlayerHandle;

/// `PlayerHandle` 的播放能力适配。
pub(crate) struct HandleApi<'a> {
    handle: &'a PlayerHandle,
}

impl<'a> HandleApi<'a> {
    pub(crate) fn new(handle: &'a PlayerHandle) -> Self {
        Self { handle }
    }
}

impl PlayerApi for HandleApi<'_> {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        self.handle.snapshot().map_err(CmdError::from_player)
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        self.handle.play(path).map_err(CmdError::from_player)
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        self.handle.pause().map_err(CmdError::from_player)
    }

    fn resume(&self) -> Result<PlaybackState, CmdError> {
        self.handle.play(None).map_err(CmdError::from_player)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.handle.next().map_err(CmdError::from_player)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.handle.prev().map_err(CmdError::from_player)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        self.handle.seek(position_ms).map_err(CmdError::from_player)
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        self.handle.volume(volume).map_err(CmdError::from_player)
    }

    fn mode(&self, mode: np_core::PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.handle.mode(mode).map_err(CmdError::from_player)
    }

    fn queue_list(&self) -> Result<np_core::QueueView, CmdError> {
        self.handle.queue_list().map_err(CmdError::from_player)
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<np_core::QueueView, CmdError> {
        self.handle.queue_add(paths).map_err(CmdError::from_player)
    }

    fn queue_remove(&self, index: usize) -> Result<np_core::QueueView, CmdError> {
        self.handle
            .queue_remove(index)
            .map_err(CmdError::from_player)
    }

    fn queue_clear(&self) -> Result<np_core::QueueView, CmdError> {
        self.handle.queue_clear().map_err(CmdError::from_player)
    }

    fn queue_jump(&self, index: usize) -> Result<np_core::QueueView, CmdError> {
        self.handle.queue_jump(index).map_err(CmdError::from_player)
    }
}

impl PlaybackTarget for PlayerHandle {
    fn play_path(&self, path: &str) -> Result<PlaybackState, CmdError> {
        self.play(Some(path.to_owned()))
            .map_err(CmdError::from_player)
    }
}

/// Tauri 事件出口。
struct TauriEventSink {
    app: AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Err(error) = self.app.emit(event, payload) {
            eprintln!("[{event}] emit failed: {error}");
        }
    }
}

fn theme_error(error: ThemeError) -> CmdError {
    normalize_cmd_dots(CmdError {
        code: error.code,
        message: error.message,
        hint: error.hint,
    })
}

/// Tauri 主题服务：目录来自 AppHandle，广播走 EventSink。
struct ThemeHandle {
    dir: PathBuf,
    sink: Arc<dyn EventSink>,
}

impl ThemeHandle {
    fn for_app(app: AppHandle, sink: Arc<dyn EventSink>) -> Result<Self, CmdError> {
        let dir = crate::ipc::themes_dir(&app).map_err(theme_error)?;
        Ok(Self { dir, sink })
    }
}

impl ThemeApi for ThemeHandle {
    fn list(&self) -> Result<Vec<ThemeListEntry>, CmdError> {
        Ok(crate::ipc::list_themes(&self.dir))
    }

    fn apply(&self, id: &str) -> Result<ThemeApplyReply, CmdError> {
        crate::ipc::apply_theme(&self.dir, id).map_err(theme_error)
    }

    fn export(&self, id: &str) -> Result<ThemeExportTarget, CmdError> {
        crate::ipc::export_builtin(&self.dir, id)
            .map(|reply| ThemeExportTarget {
                path: reply.path,
                created: reply.created,
            })
            .map_err(theme_error)
    }

    fn broadcast(&self, id: &str) {
        self.sink.theme_changed(id);
    }
}

/// 在线命令的快速分派：保持原行为，只把 AppHandle 留在适配层。
fn maybe_start_online(
    app: &AppHandle,
    _handle: &PlayerHandle,
    library: &LibraryHandle,
    input: &str,
    block_id: &str,
    sink: &Arc<dyn EventSink>,
) -> Option<CmdExecReply> {
    let parsed = match np_core::command::parse(input) {
        Ok(Some(parsed)) => parsed,
        _ => return None,
    };

    if parsed.spec.name == "search" {
        let Ok(options) = np_app::SearchOptions::parse(&parsed) else {
            return None;
        };
        if options.local {
            return None;
        }

        if options.up.is_some() || options.mid.is_some() {
            let target = options
                .up
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("mid={}", options.mid.unwrap_or(0)));
            let template = np_app::up_started_block(block_id, input, &target);
            sink.terminal_block(&template);

            let app = app.clone();
            let online = library.online();
            let sink = Arc::clone(sink);
            std::thread::spawn(move || {
                let library = app.state::<LibraryHandle>();
                let emit = move |block: &Block| sink.terminal_block(block);
                np_app::run_up_search(
                    &options,
                    &*online,
                    &*library,
                    &emit,
                    &template,
                    &np_app::now_millis,
                );
            });
            return Some(CmdExecReply {
                block_id: block_id.to_owned(),
                ok: true,
            });
        }

        let template = np_app::search_started_block(block_id, input, &options.keyword);
        sink.terminal_block(&template);

        let app = app.clone();
        let online = library.online();
        let sink = Arc::clone(sink);
        std::thread::spawn(move || {
            let library = app.state::<LibraryHandle>();
            let emit = move |block: &Block| sink.terminal_block(block);
            np_app::run_online_search_with_options(
                &options,
                &*online,
                &*library,
                &emit,
                &template,
                &np_app::now_millis,
            );
        });
        return Some(CmdExecReply {
            block_id: block_id.to_owned(),
            ok: true,
        });
    }

    if parsed.spec.name == "play"
        && parsed
            .positional
            .first()
            .is_some_and(|value| np_app::is_unsigned_integer(value))
    {
        let hits = LibraryApi::search_hits(library);
        let listing = LibraryApi::active_listing(library);
        if let Some((index, track)) = np_app::online_play_hit(&parsed, listing.as_ref(), &hits) {
            let template = np_app::online_play_started_block(block_id, input, index, &track);
            sink.terminal_block(&template);

            let app = app.clone();
            let online = library.online();
            let sink = Arc::clone(sink);
            std::thread::spawn(move || {
                let handle = app.state::<PlayerHandle>();
                let emit = move |block: &Block| sink.terminal_block(block);
                np_app::run_online_play(
                    &track,
                    index,
                    &*online,
                    &*handle,
                    &emit,
                    &template,
                    &np_app::now_millis,
                );
            });
            return Some(CmdExecReply {
                block_id: block_id.to_owned(),
                ok: true,
            });
        }
    }

    None
}

#[tauri::command(rename = "cmd.exec")]
pub(crate) fn cmd_exec(
    app: AppHandle,
    handle: State<'_, PlayerHandle>,
    library: State<'_, LibraryHandle>,
    input: String,
    block_id: String,
) -> CmdExecReply {
    let sink: Arc<dyn EventSink> = Arc::new(TauriEventSink { app: app.clone() });
    if let Some(reply) = maybe_start_online(&app, &handle, &library, &input, &block_id, &sink) {
        return reply;
    }

    let api = HandleApi::new(&handle);
    let online = library.online();
    let theme = ThemeHandle::for_app(app.clone(), Arc::clone(&sink)).ok();
    let services = Services {
        settings: Some(library.settings()),
        theme: theme.as_ref().map(|theme| theme as &dyn ThemeApi),
        online: Some(&*online),
    };

    let full = scan_full_flag(&input);
    if let Some(full) = full {
        sink.terminal_block(&scan_started_block(&block_id, &input, full));
    }
    let observer: Arc<dyn LibraryObserver> = if full.is_some() {
        let sink = Arc::clone(&sink);
        let emit = Arc::new(move |block: &Block| sink.terminal_block(block));
        Arc::new(EventObserver::with_port(
            emit,
            Arc::new(np_app::now_millis),
            block_id.clone(),
            input.clone(),
            np_app::now_millis(),
        ))
    } else {
        Arc::new(NoopLibraryObserver)
    };

    match execute_with_services(&api, &*library, services, &block_id, &input, observer) {
        Execution::Ignored => CmdExecReply { block_id, ok: true },
        Execution::Blocks { running, done } => {
            sink.terminal_block(&running);
            let ok = done.status != np_core::BlockStatus::Error;
            sink.terminal_block(&done);
            CmdExecReply { block_id, ok }
        }
        Execution::Accepted { running } => CmdExecReply {
            block_id: running.id,
            ok: true,
        },
    }
}

/// `cmd.complete` 的薄适配。
#[tauri::command(rename = "cmd.complete")]
pub(crate) fn cmd_complete(
    handle: State<'_, PlayerHandle>,
    input: String,
    cursor: usize,
) -> Completion {
    complete_for(&HandleApi::new(&handle), &input, cursor)
}

/// 旧 IPC 的扫描入口仍通过这个 helper 发块。
pub(crate) fn emit_block(app: &AppHandle, block: &Block) {
    TauriEventSink { app: app.clone() }.terminal_block(block);
}

/// 旧 IPC 的扫描观察者构造。
pub(crate) fn event_observer_for_app(
    app: AppHandle,
    block_id: String,
    input: String,
) -> Arc<dyn LibraryObserver> {
    let sink: Arc<dyn EventSink> = Arc::new(TauriEventSink { app });
    let emit = Arc::new(move |block: &Block| sink.terminal_block(block));
    Arc::new(EventObserver::with_port(
        emit,
        Arc::new(np_app::now_millis),
        block_id,
        input,
        np_app::now_millis(),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cmd_exec_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_exec!(), "cmd.exec");
    }

    #[test]
    fn cmd_complete_command_name_keeps_the_dot() {
        assert_eq!(__tauri_command_name_cmd_complete!(), "cmd.complete");
    }
}
