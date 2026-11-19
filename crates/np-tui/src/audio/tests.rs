use np_app::PlayerApi;
use np_core::{PlaybackMode, PlaybackStatus};

use crate::event_loop::DanmakuLink;

use super::live::DanmakuSession;
use super::meta::LiveMeta;
use super::{LiveState, TuiAudio, reconnect_delay};

#[cfg(test)]
impl TuiAudio {
    /// 只设置直播展示状态，不打开设备 / 网络；供 TUI 状态与渲染测试使用。
    pub(crate) fn live_for_test(&self, room_id: u64, title: &str) {
        self.live_state.set(LiveState::Playing);
        self.live_room.set(room_id);
        self.live_elapsed_override.set(Some(4402));
        self.duration_ms.set(None);
        *self.title.borrow_mut() = title.to_owned();
        *self.artist.borrow_mut() = "B站直播".to_owned();
        *self.live_metrics.borrow_mut() = Some(np_live::LiveMetrics::default());
    }

    /// 测试用：切换 Live 页的暂停 / 继续展示状态。
    pub(crate) fn set_live_paused_for_test(&self, paused: bool) {
        self.live_state.set(if paused {
            LiveState::Paused
        } else {
            LiveState::Playing
        });
    }

    /// 测试用：直接写入主播名 / 分区；同接数用卡面示例 `2000`。
    pub(crate) fn live_meta_for_test(&self, anchor: &str, area: &str) {
        *self.live_meta.borrow_mut() = Some(LiveMeta {
            anchor: anchor.to_owned(),
            area: area.to_owned(),
            live_started_at: None,
            ruid: None,
        });
        self.live_online.set(Some(2000));
    }

    /// 测试用：只改同接数，不动主播名 / 分区。
    pub(crate) fn live_online_for_test(&self, online: Option<u64>) {
        self.live_online.set(online);
    }

    /// 测试用：直接设置登录态，避免读写真实 auth 文件。
    pub(crate) fn bili_auth_for_test(&self, auth: Option<np_live::BiliAuth>) {
        *self.bili_auth.borrow_mut() = auth;
        self.auth_loaded.set(true);
    }

    /// 测试用：下一条弹幕发送直接回传固定结果，不创建网络线程。
    pub(crate) fn danmaku_result_for_test(&self, result: Result<(), String>) {
        *self.chat_test_result.borrow_mut() = Some(result);
    }

    /// 测试用：到目前为止真正进入发送路径的次数。
    pub(crate) fn danmaku_calls_for_test(&self) -> u32 {
        self.chat_test_calls.get()
    }

    /// 测试用：直接设置弹幕连接状态，不建立真实连接。
    pub(crate) fn danmaku_link_for_test(&self, link: DanmakuLink) {
        self.danmaku_link.set(link);
    }

    /// 测试用：建立一条假弹幕通道，模拟后台线程的消息与状态回传。
    pub(crate) fn danmaku_channel_for_test(
        &self,
    ) -> (
        std::sync::mpsc::Sender<np_live::DanmakuMessage>,
        std::sync::mpsc::Sender<DanmakuLink>,
    ) {
        self.stop_danmaku();
        let generation = self.danmaku_generation.get().wrapping_add(1);
        self.danmaku_generation.set(generation);
        let (message_tx, message_rx) = std::sync::mpsc::channel();
        let (link_tx, link_rx) = std::sync::mpsc::channel();
        *self.danmaku_session.borrow_mut() = Some(DanmakuSession {
            generation,
            messages: message_rx,
            links: link_rx,
        });
        self.danmaku_link.set(DanmakuLink::Connecting);
        (message_tx, link_tx)
    }
}

#[test]
fn local_play_opens_a_file_instead_of_returning_unsupported() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
    if !path.is_file() {
        return;
    }
    let audio = TuiAudio::empty();
    let state = audio
        .play(Some(path.to_string_lossy().into_owned()))
        .expect("local play should open");
    assert!(matches!(state.status, PlaybackStatus::Playing));
    assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
}

#[test]
fn tone_440_meets_the_four_playback_criteria() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
    if !path.is_file() {
        return;
    }
    let audio = TuiAudio::empty();
    let state = audio
        .play(Some(path.to_string_lossy().into_owned()))
        .expect("no E_AUDIO");
    assert!(matches!(state.status, PlaybackStatus::Playing));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(
        audio.position_ms() >= 800,
        "position {}",
        audio.position_ms()
    );
    assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
}

#[test]
fn queue_steps_and_volume_are_real() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
    if !path.is_file() {
        return;
    }
    let audio = TuiAudio::empty();
    audio
        .queue_add(vec![path.to_string_lossy().into_owned()])
        .unwrap();
    audio.queue_jump(0).unwrap();
    PlayerApi::volume(&audio, 0.5).unwrap();
    assert!((audio.snapshot().unwrap().volume - 0.5).abs() < 0.001);
    let mode = PlaybackMode {
        repeat: np_core::RepeatMode::Off,
        shuffle: true,
    };
    PlayerApi::mode(&audio, mode).unwrap();
    audio.next().unwrap();
    assert!(audio.is_playing());
    let state = audio.snapshot().unwrap();
    assert!(
        (state.volume - 0.5).abs() < 0.001,
        "volume must survive track change"
    );
    assert_eq!(state.mode, mode, "mode must survive track change");
}

#[test]
fn prepend_duplicate_and_next_follow_unique_queue() {
    let first = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
    let second =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
    if !first.is_file() || !second.is_file() {
        return;
    }
    let audio = TuiAudio::empty();
    audio
        .play(Some(first.to_string_lossy().into_owned()))
        .unwrap();
    audio
        .play(Some(second.to_string_lossy().into_owned()))
        .unwrap();
    assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
    audio
        .play(Some(second.to_string_lossy().into_owned()))
        .unwrap();
    assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
    audio.next().unwrap();
    assert_eq!(audio.source_path().as_deref(), Some(first.as_path()));
}
#[test]
fn mode_cycles_through_four_playback_modes() {
    let audio = TuiAudio::empty();
    let list = audio.cycle_mode();
    assert_eq!(list.repeat, np_core::RepeatMode::All);
    assert!(!list.shuffle);
    let one = audio.cycle_mode();
    assert_eq!(one.repeat, np_core::RepeatMode::One);
    assert!(!one.shuffle);
    let shuffle = audio.cycle_mode();
    assert!(shuffle.shuffle);
    let sequential = audio.cycle_mode();
    assert_eq!(sequential.repeat, np_core::RepeatMode::Off);
    assert!(!sequential.shuffle);
}
#[test]
fn reconnect_backoff_steps_and_state() {
    assert_eq!(reconnect_delay(1).as_secs(), 1);
    assert_eq!(reconnect_delay(2).as_secs(), 2);
    assert_eq!(reconnect_delay(3).as_secs(), 5);
    assert_eq!(reconnect_delay(4).as_secs(), 10);
    assert_eq!(reconnect_delay(5).as_secs(), 20);
    assert_eq!(reconnect_delay(6).as_secs(), 30);
    assert_eq!(reconnect_delay(99).as_secs(), 30);

    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    audio.begin_reconnect(Some("测试断流".to_owned()));
    assert_eq!(audio.live_state(), LiveState::Reconnecting);
    assert_eq!(audio.reconnect_attempt.get(), 1);
    assert_eq!(
        audio.live_status_label().as_deref(),
        Some("LIVE RECONNECTING 1/6")
    );
    assert!(audio.stop_live());
    assert_eq!(audio.live_state(), LiveState::Idle);
}

#[test]
fn live_state_text_is_chinese_and_flips_on_pause() {
    let audio = TuiAudio::empty();
    audio.live_for_test(21622811, "BLAST");
    assert_eq!(audio.live_state_text(), "直播");
    audio.live_state.set(LiveState::Paused);
    assert_eq!(audio.live_state_text(), "暂停");
}

#[test]
fn live_anchor_prefers_meta_and_falls_back_to_the_room() {
    let audio = TuiAudio::empty();
    assert_eq!(audio.live_anchor(), "直播");
    assert_eq!(audio.live_area(), "");

    audio.live_for_test(21622811, "BLAST");
    assert_eq!(audio.live_anchor(), "房间 21622811");
    assert_eq!(audio.live_area(), "");

    audio.live_meta_for_test("测试主播", "游戏赛事");
    assert_eq!(audio.live_anchor(), "测试主播");
    assert_eq!(audio.live_area(), "游戏赛事");

    assert!(audio.stop_live());
    assert_eq!(audio.live_anchor(), "直播");
    assert_eq!(audio.live_area(), "");
}

#[test]
fn live_buffer_target_clamps_to_a_sane_range() {
    let audio = TuiAudio::empty();
    assert_eq!(audio.live_buffer_target_ms(), 900);
    audio.set_live_buffer_target(1_200);
    assert_eq!(audio.live_buffer_target_ms(), 1_200);
    audio.set_live_buffer_target(0);
    assert_eq!(
        audio.live_buffer_target_ms(),
        np_core::settings::LIVE_BUFFER_MS_MIN
    );
    audio.set_live_buffer_target(u64::MAX);
    assert_eq!(
        audio.live_buffer_target_ms(),
        np_core::settings::LIVE_BUFFER_MS_MAX
    );
}

#[test]
fn seek_moves_playback_position() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
    if !path.is_file() {
        return;
    }
    let audio = TuiAudio::empty();
    audio
        .play(Some(path.to_string_lossy().into_owned()))
        .expect("open tone");
    let target = 1_000;
    PlayerApi::seek(&audio, target).expect("seek");
    std::thread::sleep(std::time::Duration::from_millis(100));
    let position = audio.position_ms();
    assert!(
        position >= 900,
        "seek did not move position: target={target}, position={position}"
    );
}
