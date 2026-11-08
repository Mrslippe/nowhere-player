//! 播放领域模型与状态机（T-0013）。
//!
//! 这里只有纯数据与纯状态转换：不依赖 `np-audio`、不碰线程、不碰 IPC。
//! `apps/desktop` 的 `PlayerActor` 负责把 [`PlayerCommand`] 喂给
//! `np_audio::Player`，再把这里产出的 [`PlayerEvent`] 广播给前端。
//!
//! 序列化形状即 IPC 契约（`camelCase` 字段名），前端镜像见 T-0014。

use serde::{Deserialize, Serialize};

/// 位置事件周期（毫秒）：4 Hz（ARCH-006 §5）。
pub const POSITION_TICK_MS: u64 = 250;

/// 播放器状态。`Idle` 表示尚未打开任何曲目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    /// 没有打开的曲目。
    Idle,
    /// 正在播放。
    Playing,
    /// 已暂停（位置保留）。
    Paused,
    /// 已自然播到文件末尾（边沿只上报一次）。
    Ended,
    /// 解码/设备错误，需重新 `play` 才能恢复。
    Error,
}

/// 循环模式；T-0013 没有队列，固定为 `Off`，字段先按契约保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    /// 不循环。
    #[default]
    Off,
    /// 单曲循环。
    One,
    /// 列表循环。
    All,
}

/// 播放模式（开发方案 §4.3 的“模式”字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackMode {
    /// 循环模式。
    pub repeat: RepeatMode,
    /// 是否随机。
    pub shuffle: bool,
}

/// 播放状态快照（`evt.player` 的载荷，ARCH-006 §5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackState {
    /// 当前状态。
    pub status: PlaybackStatus,
    /// 当前曲目标识；本地文件在 T-0013 用绝对路径字符串充当。
    pub track_id: Option<String>,
    /// 当前位置（毫秒）。
    pub position_ms: u64,
    /// 总时长（毫秒）；容器未给出时为 `None`。
    pub duration_ms: Option<u64>,
    /// 用户音量（0.0–1.0，感知曲线由 `np-audio` 负责）。
    pub volume: f32,
    /// 播放模式；T-0013 恒为默认值，T-0015 队列接线后才会变化。
    pub mode: PlaybackMode,
    /// 最近一次异步错误；成功 `play` 或打开新曲目后清空。
    #[serde(default)]
    pub error: Option<PlayerError>,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Idle,
            track_id: None,
            position_ms: 0,
            duration_ms: None,
            volume: 1.0,
            mode: PlaybackMode::default(),
            error: None,
        }
    }
}

/// IPC 错误码闭集（映射到 ARCH-005 风格；T-0013 §7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerErrorCode {
    /// 解码 / IO / 重采样 / 参数非法。
    #[serde(rename = "E_DECODE")]
    Decode,
    /// 无可用设备 / 设备丢失。
    #[serde(rename = "E_DEVICE_LOST")]
    DeviceLost,
    /// 不支持的格式，或当前操作尚无实现（`next`/`prev`）。
    #[serde(rename = "E_UNSUPPORTED")]
    Unsupported,
    /// 命令通道已满或播放器线程不可用。
    #[serde(rename = "E_BUSY")]
    Busy,
}

/// IPC 错误载荷：`{ code, message }`，可直接返回给前端。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerError {
    /// 稳定错误码。
    pub code: PlayerErrorCode,
    /// 一句话原因。
    pub message: String,
}

/// 音频错误的三个归类桶。
///
/// `np-core` 不依赖 `np-audio`，所以由桌面层把 `AudioError` 归入这三类，
/// 再由 [`PlayerError::from_audio`] 映射成 IPC 错误码；映射本身可在这里单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFailure {
    /// 解码、IO、重采样、参数非法。
    Decode,
    /// 无可用设备或设备丢失。
    DeviceLost,
    /// 不支持的格式。
    Unsupported,
}

impl PlayerError {
    /// 用错误码和原因构造错误。
    pub fn new(code: PlayerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// `E_DECODE`。
    pub fn decode(message: impl Into<String>) -> Self {
        Self::new(PlayerErrorCode::Decode, message)
    }

    /// `E_DEVICE_LOST`。
    pub fn device_lost(message: impl Into<String>) -> Self {
        Self::new(PlayerErrorCode::DeviceLost, message)
    }

    /// `E_UNSUPPORTED`。
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(PlayerErrorCode::Unsupported, message)
    }

    /// `E_BUSY`。
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(PlayerErrorCode::Busy, message)
    }

    /// 按 T-0013 §7 的错误码表把音频失败归类成 IPC 错误。
    pub fn from_audio(failure: AudioFailure, message: impl Into<String>) -> Self {
        let code = match failure {
            AudioFailure::Decode => PlayerErrorCode::Decode,
            AudioFailure::DeviceLost => PlayerErrorCode::DeviceLost,
            AudioFailure::Unsupported => PlayerErrorCode::Unsupported,
        };
        Self::new(code, message)
    }
}

/// 播放器命令（actor 的输入，也是 IPC 命令的领域镜像）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerCommand {
    /// 取当前完整快照。
    Snapshot,
    /// `path = Some` 打开新曲目并播放；`None` 继续当前曲目。
    Play {
        /// 本地文件路径。
        path: Option<String>,
    },
    /// 暂停。
    Pause,
    /// 下一首；队列未实现，固定 `E_UNSUPPORTED`。
    Next,
    /// 上一首；队列未实现，固定 `E_UNSUPPORTED`。
    Prev,
    /// 跳转到 `position_ms`。
    Seek {
        /// 目标位置（毫秒）。
        #[serde(rename = "positionMs")]
        position_ms: u64,
    },
    /// 设置音量（0.0–1.0，非法值会被夹取）。
    SetVolume {
        /// 目标音量。
        volume: f32,
    },
    /// 请求 actor 退出（由控制通道使用）。
    Shutdown,
}

/// 4 Hz 位置事件（`evt.position` 的载荷）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionTick {
    /// 当前位置（毫秒）。
    pub position_ms: u64,
}

/// actor 对外广播的事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    /// 完整状态快照 → `evt.player`。
    StateChanged(PlaybackState),
    /// 4 Hz 位置 → `evt.position`。
    PositionTick(PositionTick),
}

/// 纯状态机：actor 只负责调用 `np-audio`，所有状态转换规则都在这里。
///
/// `ended_reported` 是 `TrackEnded` 的边沿去重标记：切歌、重新 `play`
/// 或 `seek` 时复位（T-0013 §7）。
#[derive(Debug, Clone)]
pub struct PlaybackMachine {
    state: PlaybackState,
    ended_reported: bool,
}

impl Default for PlaybackMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackMachine {
    /// 初始为 `Idle`，音量 1.0。
    pub fn new() -> Self {
        Self {
            state: PlaybackState::default(),
            ended_reported: false,
        }
    }

    /// 当前快照。
    pub fn snapshot(&self) -> PlaybackState {
        self.state.clone()
    }

    /// 是否处于 `Playing`（决定是否发 4 Hz 位置事件）。
    pub fn is_playing(&self) -> bool {
        self.state.status == PlaybackStatus::Playing
    }

    /// 打开新曲目：回到 `Paused` + 位置 0，清空错误并复位边沿标记。
    pub fn open(&mut self, track_id: impl Into<String>, duration_ms: Option<u64>) -> PlayerEvent {
        self.ended_reported = false;
        self.state.status = PlaybackStatus::Paused;
        self.state.track_id = Some(track_id.into());
        self.state.position_ms = 0;
        self.state.duration_ms = duration_ms;
        self.state.error = None;
        PlayerEvent::StateChanged(self.snapshot())
    }

    /// 开始/继续播放；清空上一次错误并复位边沿标记。
    pub fn play(&mut self, position_ms: u64) -> PlayerEvent {
        self.ended_reported = false;
        self.state.status = PlaybackStatus::Playing;
        self.state.position_ms = position_ms;
        self.state.error = None;
        PlayerEvent::StateChanged(self.snapshot())
    }

    /// 暂停并保留当前位置。
    pub fn pause(&mut self, position_ms: u64) -> PlayerEvent {
        self.state.status = PlaybackStatus::Paused;
        self.state.position_ms = position_ms;
        PlayerEvent::StateChanged(self.snapshot())
    }

    /// 跳转位置；从 `Ended` 跳转后回到 `Paused`，并复位边沿标记。
    pub fn seek(&mut self, position_ms: u64) -> PlayerEvent {
        self.ended_reported = false;
        if self.state.status == PlaybackStatus::Ended {
            self.state.status = PlaybackStatus::Paused;
        }
        self.state.position_ms = position_ms;
        PlayerEvent::StateChanged(self.snapshot())
    }

    /// 设置音量；越界与 NaN 被夹取到 0.0–1.0。
    pub fn set_volume(&mut self, volume: f32) -> PlayerEvent {
        self.state.volume = clamp_volume(volume);
        PlayerEvent::StateChanged(self.snapshot())
    }

    /// 只同步位置（`snapshot` 命令用），不产生事件。
    pub fn sync_position(&mut self, position_ms: u64) {
        self.state.position_ms = position_ms;
    }

    /// 异步失败：错误状态粘滞，直到下一次成功 `play` 或打开新曲目。
    pub fn fail(&mut self, error: PlayerError, position_ms: u64) -> Option<PlayerEvent> {
        if self.state.status == PlaybackStatus::Error {
            return None;
        }
        self.ended_reported = true;
        self.state.status = PlaybackStatus::Error;
        self.state.position_ms = position_ms;
        self.state.error = Some(error);
        Some(PlayerEvent::StateChanged(self.snapshot()))
    }

    /// 4 Hz tick。只有 `Playing` 时产出事件：
    ///
    /// - 播到末尾 → 一次 `StateChanged(Ended)`（边沿去重）；
    /// - 否则 → `PositionTick`。
    pub fn tick(&mut self, position_ms: u64, ended: bool) -> Option<PlayerEvent> {
        if self.state.status != PlaybackStatus::Playing {
            return None;
        }
        self.state.position_ms = position_ms;
        if ended {
            if self.ended_reported {
                return None;
            }
            self.ended_reported = true;
            self.state.status = PlaybackStatus::Ended;
            if let Some(duration) = self.state.duration_ms {
                self.state.position_ms = duration;
            }
            return Some(PlayerEvent::StateChanged(self.snapshot()));
        }
        Some(PlayerEvent::PositionTick(PositionTick { position_ms }))
    }
}

/// 与 `np_audio::clamp_volume` 相同的语义，避免 `np-core` 反向依赖音频层。
fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn playing_machine() -> PlaybackMachine {
        let mut machine = PlaybackMachine::new();
        machine.open("C:/music/a.flac", Some(180_000));
        machine.play(0);
        machine
    }

    #[test]
    fn default_state_is_idle_with_full_volume() {
        let state = PlaybackState::default();
        assert_eq!(state.status, PlaybackStatus::Idle);
        assert!(state.track_id.is_none());
        assert_eq!(state.position_ms, 0);
        assert!(state.duration_ms.is_none());
        assert_eq!(state.volume, 1.0);
        assert_eq!(state.mode, PlaybackMode::default());
        assert!(state.error.is_none());
    }

    #[test]
    fn open_play_pause_seek_keep_snapshot_consistent() {
        let mut machine = PlaybackMachine::new();
        let opened = machine.open("C:/music/a.flac", Some(180_000));
        match opened {
            PlayerEvent::StateChanged(state) => {
                assert_eq!(state.status, PlaybackStatus::Paused);
                assert_eq!(state.duration_ms, Some(180_000));
                assert_eq!(state.track_id.as_deref(), Some("C:/music/a.flac"));
            }
            PlayerEvent::PositionTick(_) => panic!("open must emit a state snapshot"),
        }

        machine.play(0);
        assert!(machine.is_playing());
        let tick = machine.tick(250, false);
        assert_eq!(
            tick,
            Some(PlayerEvent::PositionTick(PositionTick { position_ms: 250 }))
        );

        machine.pause(250);
        assert_eq!(machine.snapshot().status, PlaybackStatus::Paused);
        assert_eq!(machine.snapshot().position_ms, 250);

        machine.seek(90_000);
        let state = machine.snapshot();
        assert_eq!(state.status, PlaybackStatus::Paused);
        assert_eq!(state.position_ms, 90_000);
    }

    #[test]
    fn ended_is_reported_once_until_play_resets_it() {
        let mut machine = playing_machine();
        let first = machine.tick(180_000, true);
        match first {
            Some(PlayerEvent::StateChanged(state)) => {
                assert_eq!(state.status, PlaybackStatus::Ended);
                assert_eq!(state.position_ms, 180_000);
            }
            _ => panic!("ending must emit exactly one state snapshot"),
        }
        assert!(machine.tick(180_000, true).is_none(), "ended edge repeated");

        machine.play(0);
        assert!(
            machine.tick(180_000, true).is_some(),
            "play must reset the ended edge"
        );
    }

    #[test]
    fn seek_after_end_returns_to_paused() {
        let mut machine = playing_machine();
        let _ = machine.tick(180_000, true);
        assert_eq!(machine.snapshot().status, PlaybackStatus::Ended);

        machine.seek(10_000);
        let state = machine.snapshot();
        assert_eq!(state.status, PlaybackStatus::Paused);
        assert_eq!(state.position_ms, 10_000);
        assert!(
            machine.tick(10_000, false).is_none(),
            "paused emits no ticks"
        );
    }

    #[test]
    fn ticks_are_suppressed_outside_playing() {
        let mut machine = PlaybackMachine::new();
        assert!(machine.tick(0, false).is_none(), "idle emits no ticks");
        machine.open("a", None);
        assert!(machine.tick(0, false).is_none(), "paused emits no ticks");
        machine.play(0);
        assert!(machine.tick(0, false).is_some());
        let _ = machine.fail(PlayerError::decode("boom"), 10);
        assert!(machine.tick(10, false).is_none(), "error emits no ticks");
    }

    #[test]
    fn volume_is_clamped_to_unit_range() {
        let mut machine = PlaybackMachine::new();
        machine.set_volume(-1.0);
        assert_eq!(machine.snapshot().volume, 0.0);
        machine.set_volume(2.0);
        assert_eq!(machine.snapshot().volume, 1.0);
        machine.set_volume(f32::NAN);
        assert_eq!(machine.snapshot().volume, 0.0);
        machine.set_volume(0.25);
        assert_eq!(machine.snapshot().volume, 0.25);
    }

    #[test]
    fn audio_failures_map_to_the_closed_error_set() {
        assert_eq!(
            PlayerError::from_audio(AudioFailure::Decode, "x").code,
            PlayerErrorCode::Decode
        );
        assert_eq!(
            PlayerError::from_audio(AudioFailure::DeviceLost, "x").code,
            PlayerErrorCode::DeviceLost
        );
        assert_eq!(
            PlayerError::from_audio(AudioFailure::Unsupported, "x").code,
            PlayerErrorCode::Unsupported
        );
        assert_eq!(PlayerError::busy("x").code, PlayerErrorCode::Busy);
    }

    #[test]
    fn error_status_is_sticky_until_play_recovers() {
        let mut machine = playing_machine();
        let error = PlayerError::device_lost("unplugged");
        assert!(machine.fail(error.clone(), 5_000).is_some());
        assert_eq!(machine.snapshot().status, PlaybackStatus::Error);
        assert_eq!(machine.snapshot().error, Some(error));
        assert!(
            machine.fail(PlayerError::decode("again"), 5_000).is_none(),
            "error must not re-emit"
        );

        machine.play(5_000);
        let state = machine.snapshot();
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert!(state.error.is_none());
    }

    #[test]
    fn state_and_events_serialize_with_camel_case_fields() {
        let state = PlaybackState {
            status: PlaybackStatus::Playing,
            track_id: Some("C:/music/a.flac".into()),
            position_ms: 42,
            duration_ms: Some(100),
            volume: 0.5,
            mode: PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: true,
            },
            error: Some(PlayerError::decode("boom")),
        };

        let value: Value = serde_json::to_value(&state).expect("state serializes");
        assert_eq!(value["status"], json!("playing"));
        assert_eq!(value["trackId"], json!("C:/music/a.flac"));
        assert_eq!(value["positionMs"], json!(42));
        assert_eq!(value["durationMs"], json!(100));
        assert_eq!(value["volume"], json!(0.5));
        assert_eq!(value["mode"]["repeat"], json!("all"));
        assert_eq!(value["mode"]["shuffle"], json!(true));
        assert_eq!(value["error"]["code"], json!("E_DECODE"));

        let tick = serde_json::to_value(PlayerEvent::PositionTick(PositionTick { position_ms: 7 }))
            .expect("tick serializes");
        assert_eq!(tick["type"], json!("position_tick"));
        assert_eq!(tick["positionMs"], json!(7));

        let changed =
            serde_json::to_value(PlayerEvent::StateChanged(state)).expect("state event serializes");
        assert_eq!(changed["type"], json!("state_changed"));
        assert_eq!(changed["status"], json!("playing"));
    }

    #[test]
    fn commands_serialize_with_their_domain_payload() {
        let seek = serde_json::to_value(PlayerCommand::Seek { position_ms: 1_500 })
            .expect("seek serializes");
        assert_eq!(seek["type"], json!("seek"));
        assert_eq!(seek["positionMs"], json!(1_500));

        let play =
            serde_json::to_value(PlayerCommand::Play { path: None }).expect("play serializes");
        assert_eq!(play["type"], json!("play"));
        assert_eq!(play["path"], Value::Null);

        let volume = serde_json::to_value(PlayerCommand::SetVolume { volume: 0.75 })
            .expect("volume serializes");
        assert_eq!(volume["volume"], json!(0.75));
    }
}
