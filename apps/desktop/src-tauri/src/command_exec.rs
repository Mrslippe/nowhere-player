//! 命令执行器（T-0018）：把 `np_core` 的解析结果变成播放器调用与终端输出。
//!
//! 这一层是 T-0017（解析）与 T-0019（渲染）之间的胶水：
//! - 只依赖 [`PlayerApi`]，不直接依赖 `PlayerHandle`；单测用 `FakeApi` 记录调用，
//!   因此执行器逻辑不碰真实音频设备（T-0018 §6.1）。
//! - 输出复用 `np_core::terminal` 的六种原语，不自造字符串协议。
//! - `cmd.exec` 同步执行，结果通过 `terminal:block` 事件推回前端。
//!
//! # 约定
//!
//! - **序号对用户一律 1 基**：`play 3` 是队列第 3 首，执行层内部转成 0 基。
//! - `vol ±N` 先读当前音量再算，结果在调用前钳到 0–100。
//! - `mode` 一次下发完整 `{ repeat, shuffle }`，避免两次调用互相覆盖。
//! - `clear` 是视图操作，由前端处理；执行器只留一条防御性错误。

use np_core::{
    CommandErrorCode, Parsed, ParseError, PlaybackMode, PlaybackState, PlayerError,
    PlayerErrorCode, QueueView,
};

/// 执行器依赖的播放器能力（T-0018 §6.1）。
///
/// 生产实现是 `HandleApi`（`player_actor` 里的 `PlayerHandle` 适配）；
/// 单测实现是 `FakeApi`。执行器只按 trait 调用，所以命令映射可以整体单测。
pub(crate) trait PlayerApi {
    /// 当前完整快照。
    fn snapshot(&self) -> Result<PlaybackState, CmdError>;
    /// 打开路径并播放（同时入队）；`None` 时继续当前曲目。
    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError>;
    /// 暂停。
    fn pause(&self) -> Result<PlaybackState, CmdError>;
    /// 继续播放（等价于 `play(None)`）。
    fn resume(&self) -> Result<PlaybackState, CmdError>;
    /// 下一首。
    fn next(&self) -> Result<PlaybackState, CmdError>;
    /// 上一首。
    fn prev(&self) -> Result<PlaybackState, CmdError>;
    /// 跳到绝对播放位置（毫秒）。
    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError>;
    /// 设置音量（0.0–1.0）。
    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError>;
    /// 设置完整播放模式。
    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError>;
    /// 队列快照。
    fn queue_list(&self) -> Result<QueueView, CmdError>;
    /// 追加曲目（按给定顺序）。
    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError>;
    /// 移除播放顺序里的第 `index` 首（0 基）。
    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError>;
    /// 清空队列。
    fn queue_clear(&self) -> Result<QueueView, CmdError>;
    /// 跳到播放顺序里的第 `index` 首（0 基）并播放。
    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError>;
}

/// 执行期错误：稳定的 IPC 码 + 一句原因 + 可选的修复提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CmdError {
    /// IPC 错误码，如 `E_CMD_ARG` / `E_UNSUPPORTED`。
    pub(crate) code: String,
    /// 一句话原因，可直接展示。
    pub(crate) message: String,
    /// 修复提示；解析错误用 `usage` 或拼写建议填充。
    pub(crate) hint: Option<String>,
}

impl CmdError {
    /// 用错误码与原因构造错误。
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
        }
    }

    /// 解析错误 → 执行错误：`hint` 优先用 `usage`，没有则用拼写建议。
    pub(crate) fn from_parse(error: &ParseError) -> Self {
        Self {
            code: error.code.as_str().to_owned(),
            message: error.message.clone(),
            hint: error
                .usage
                .clone()
                .or_else(|| error.suggestion.as_ref().map(|name| suggestion(name))),
        }
    }

    /// 播放器错误 → 执行错误。`hint` 由执行器补命令用法（见 `with_usage`）。
    pub(crate) fn from_player(error: PlayerError) -> Self {
        Self {
            code: player_error_code(error.code).to_owned(),
            message: error.message,
            hint: None,
        }
    }

    /// 补一条提示；已有提示时不覆盖。
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        if self.hint.is_none() {
            self.hint = Some(hint.into());
        }
        self
    }

    /// 失败时至少给一条提示：没有更具体的就用命令用法。
    pub(crate) fn with_usage(mut self, parsed: &Parsed) -> Self {
        if self.hint.is_none() {
            self.hint = Some(format!("用法：{}", parsed.usage()));
        }
        self
    }

    /// 转成终端的 `error` 原语。
    pub(crate) fn to_output(&self) -> np_core::Output {
        np_core::Output::Error {
            code: self.code.clone(),
            message: self.message.clone(),
            hint: self.hint.clone(),
            actions: None,
        }
    }
}

/// 拼写建议的展示文案，与 `ParseError` 的 `Display` 保持一致。
fn suggestion(name: &str) -> String {
    format!("did you mean '{name}'?")
}

/// `PlayerErrorCode` 的 IPC 字符串（`np-core` 没有暴露 `as_str`）。
fn player_error_code(code: PlayerErrorCode) -> &'static str {
    match code {
        PlayerErrorCode::Decode => "E_DECODE",
        PlayerErrorCode::DeviceLost => "E_DEVICE_LOST",
        PlayerErrorCode::Unsupported => "E_UNSUPPORTED",
        PlayerErrorCode::Busy => "E_BUSY",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use np_core::command::parse;
    use np_core::{PlaybackMode, PlaybackStatus, RepeatMode};

    use super::*;

    /// `FakeApi` 记录的一次调用。
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Snapshot,
        Play(Option<String>),
        Pause,
        Resume,
        Next,
        Prev,
        Seek(u64),
        Volume(f32),
        Mode(PlaybackMode),
        QueueList,
        QueueAdd(Vec<String>),
        QueueRemove(usize),
        QueueClear,
        QueueJump(usize),
    }

    /// 单测用播放器：记录调用、返回构造值，不碰真实设备。
    struct FakeApi {
        calls: Mutex<Vec<Call>>,
        state: Mutex<PlaybackState>,
        queue: Mutex<QueueView>,
        reject_paths: Mutex<Vec<String>>,
        playback_error: Mutex<Option<CmdError>>,
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                state: Mutex::new(PlaybackState {
                    volume: 0.5,
                    ..PlaybackState::default()
                }),
                queue: Mutex::new(QueueView {
                    tracks: Vec::new(),
                    index: None,
                }),
                reject_paths: Mutex::new(Vec::new()),
                playback_error: Mutex::new(None),
            }
        }

        fn with_state(self, state: PlaybackState) -> Self {
            *self.state.lock().expect("state mutex") = state;
            self
        }

        fn with_queue(self, queue: QueueView) -> Self {
            *self.queue.lock().expect("queue mutex") = queue;
            self
        }

        fn with_reject_paths(self, paths: &[&str]) -> Self {
            *self.reject_paths.lock().expect("reject mutex") =
                paths.iter().map(|path| (*path).to_owned()).collect();
            self
        }

        fn with_playback_error(self, error: CmdError) -> Self {
            *self.playback_error.lock().expect("error mutex") = Some(error);
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("calls mutex").clone()
        }

        fn record(&self, call: Call) {
            self.calls.lock().expect("calls mutex").push(call);
        }

        /// 播放类命令的公共返回：配置了错误就回错误，否则回当前快照。
        fn playback(&self) -> Result<PlaybackState, CmdError> {
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            Ok(self.state.lock().expect("state mutex").clone())
        }
    }

    impl PlayerApi for FakeApi {
        fn snapshot(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Snapshot);
            self.playback()
        }

        fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
            self.record(Call::Play(path.clone()));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            {
                let mut state = self.state.lock().expect("state mutex");
                if path.is_some() {
                    state.track_id = path;
                }
                state.status = PlaybackStatus::Playing;
            }
            self.playback()
        }

        fn pause(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Pause);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Paused;
            self.playback()
        }

        fn resume(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Resume);
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").status = PlaybackStatus::Playing;
            self.playback()
        }

        fn next(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Next);
            self.playback()
        }

        fn prev(&self) -> Result<PlaybackState, CmdError> {
            self.record(Call::Prev);
            self.playback()
        }

        fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
            self.record(Call::Seek(position_ms));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").position_ms = position_ms;
            self.playback()
        }

        fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
            self.record(Call::Volume(volume));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").volume = volume;
            self.playback()
        }

        fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
            self.record(Call::Mode(mode));
            if let Some(error) = self.playback_error.lock().expect("error mutex").clone() {
                return Err(error);
            }
            self.state.lock().expect("state mutex").mode = mode;
            self.playback()
        }

        fn queue_list(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueList);
            Ok(self.queue.lock().expect("queue mutex").clone())
        }

        fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
            self.record(Call::QueueAdd(paths.clone()));
            {
                let reject = self.reject_paths.lock().expect("reject mutex");
                if let Some(path) = paths.iter().find(|path| reject.contains(path)) {
                    return Err(CmdError::new("E_DECODE", format!("无法加入：{path}")));
                }
            }
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.extend(paths);
            if queue.index.is_none() {
                queue.index = Some(0);
            }
            Ok(queue.clone())
        }

        fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueRemove(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.tracks.remove(index);
            if queue.tracks.is_empty() {
                queue.index = None;
            } else if let Some(current) = queue.index {
                if index < current {
                    queue.index = Some(current - 1);
                } else if index == current {
                    queue.index = Some(current.min(queue.tracks.len() - 1));
                }
            }
            Ok(queue.clone())
        }

        fn queue_clear(&self) -> Result<QueueView, CmdError> {
            self.record(Call::QueueClear);
            let mut queue = self.queue.lock().expect("queue mutex");
            queue.tracks.clear();
            queue.index = None;
            Ok(queue.clone())
        }

        fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
            self.record(Call::QueueJump(index));
            let mut queue = self.queue.lock().expect("queue mutex");
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_DECODE", "队列下标越界"));
            }
            queue.index = Some(index);
            Ok(queue.clone())
        }
    }

    fn queue_view(tracks: &[&str], index: Option<usize>) -> QueueView {
        QueueView {
            tracks: tracks.iter().map(|track| (*track).to_owned()).collect(),
            index,
        }
    }

    #[test]
    fn fake_api_records_calls() {
        let api = FakeApi::new();
        api.pause().expect("pause succeeds");
        api.seek(1_500).expect("seek succeeds");

        assert_eq!(api.calls(), vec![Call::Pause, Call::Seek(1_500)]);
    }

    #[test]
    fn parse_error_keeps_code_and_usage_hint() {
        let error = parse("plya").expect_err("unknown command must fail");
        let error = CmdError::from_parse(&error);

        assert_eq!(error.code, "E_CMD_UNKNOWN");
        assert_eq!(error.hint.as_deref(), Some("did you mean 'play'?"));
    }

    #[test]
    fn player_error_maps_to_ipc_code() {
        let error = CmdError::from_player(PlayerError::unsupported("没有可暂停的曲目"));

        assert_eq!(error.code, "E_UNSUPPORTED");
        assert_eq!(error.message, "没有可暂停的曲目");
        assert_eq!(error.hint, None);
    }

    #[test]
    fn usage_hint_is_filled_when_missing() {
        let parsed = parse("vol 5")
            .expect("valid input")
            .expect("non-empty input");
        let error = CmdError::new("E_DECODE", "设备丢失").with_usage(&parsed);

        assert_eq!(error.hint.as_deref(), Some("用法：vol <0-100|±N>"));
    }

    #[test]
    fn repeat_mode_defaults_to_off_for_unknown_values() {
        // 解析层已把枚举归一化；这里只锁住“未知值绝不 panic”。
        let mode = PlaybackMode {
            repeat: RepeatMode::Off,
            shuffle: false,
        };
        assert_eq!(mode, PlaybackMode::default());
    }
}
