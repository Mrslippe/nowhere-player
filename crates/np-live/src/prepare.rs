//! 后台准备：API → open_stream → FLV 头 + AAC ASC → [`PreparedLive`]。
//!
//! 这一步可能阻塞（网络 + 首帧），只能在 prepare 线程里跑；主线程用
//! `Player::open_source_with_ring(source, 150)` 接成品。

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use np_audio::AudioFormat;
use np_audio::AudioSource;
use np_sources::Transport;

use crate::api;
use crate::auth::BiliAuth;
use crate::control::LiveControl;
use crate::error::LiveError;
use crate::metrics::LiveMetrics;
use crate::source::LiveAudioSource;

/// 已经连上 CDN、攒到启动缓冲的直播源。
pub struct PreparedLive {
    /// 接口返回的真实房间号。
    pub room_id: u64,
    /// 房间标题；取不到时为空串。
    pub title: String,
    /// AAC 解码后的格式（直播 TUI 用来显示）。
    pub format: AudioFormat,
    /// 可以直接交给 `Player::open_source_with_ring`。
    pub source: Box<dyn AudioSource>,
    /// 缓冲 / drop / underrun 计数；TUI 用它显示 LIVE 状态。
    pub metrics: LiveMetrics,
    /// 暂停/恢复控制；暂停保留连接，恢复先播缓存。
    pub control: LiveControl,
}

/// 准备一路直播音频。
///
/// - `cancel` 在 API / 连接 / 首帧之间轮询；置位后返回 [`LiveError::Cancelled`]；
/// - 不能在 UI 线程调用。
pub fn prepare(
    transport: &dyn Transport,
    room_id: u64,
    cancel: &AtomicBool,
) -> Result<PreparedLive, LiveError> {
    prepare_auth(transport, room_id, cancel, None)
}

/// 带可选 B站登录态准备直播；cookie 只用于 playurl API。
pub fn prepare_auth(
    transport: &dyn Transport,
    room_id: u64,
    cancel: &AtomicBool,
    auth: Option<&BiliAuth>,
) -> Result<PreparedLive, LiveError> {
    if room_id == 0 {
        return Err(LiveError::InvalidRoom("房间号必须是正整数".to_owned()));
    }
    ensure_not_cancelled(cancel)?;

    // 诊断：直接使用浏览器同一条 FLV URL，跳过 playurl 选择。
    if let Some(path) = std::env::var_os("NP_LIVE_STREAM_URL_FILE") {
        let stream_url = fs::read_to_string(&path)
            .map_err(LiveError::Io)?
            .trim()
            .to_owned();
        if stream_url.is_empty() {
            return Err(LiveError::BadStream(
                "NP_LIVE_STREAM_URL_FILE 为空".to_owned(),
            ));
        }
        ensure_not_cancelled(cancel)?;
        let title = api::fetch_room_info(transport, room_id)
            .map(|room| room.title)
            .unwrap_or_default();
        ensure_not_cancelled(cancel)?;
        let stream = transport.open_stream(&stream_url, api::stream_headers())?;
        ensure_not_cancelled(cancel)?;
        let control = LiveControl::default();
        let (source, metrics) = LiveAudioSource::open(stream, control.clone())?;
        ensure_not_cancelled(cancel)?;
        let format = source.format();
        return Ok(PreparedLive {
            room_id,
            title,
            format,
            source: Box::new(source),
            metrics,
            control,
        });
    }

    let play_info = api::fetch_play_info_with_auth(transport, room_id, auth)?;
    ensure_not_cancelled(cancel)?;
    if !play_info.is_live() {
        return Err(LiveError::Offline);
    }
    let stream_url = play_info.flv_url.ok_or(LiveError::NoStream)?;

    // 标题是 best-effort：get_info 失败不影响听声音。
    let title = api::fetch_room_info(transport, play_info.room_id)
        .map(|room| room.title)
        .unwrap_or_default();

    ensure_not_cancelled(cancel)?;
    let stream = transport.open_stream(&stream_url, api::stream_headers())?;
    ensure_not_cancelled(cancel)?;

    let control = LiveControl::default();
    let (source, metrics) = LiveAudioSource::open(stream, control.clone())?;
    ensure_not_cancelled(cancel)?;
    let format = source.format();
    Ok(PreparedLive {
        room_id: play_info.room_id,
        title,
        format,
        source: Box::new(source),
        metrics,
        control,
    })
}

/// 用生产 [`np_sources::ReqwestTransport`] 准备直播；TUI 只调这个入口。
pub fn prepare_default(room_id: u64, cancel: &AtomicBool) -> Result<PreparedLive, LiveError> {
    prepare_default_auth(room_id, cancel, None)
}

/// 生产入口；带可选登录态。
pub fn prepare_default_auth(
    room_id: u64,
    cancel: &AtomicBool,
    auth: Option<&BiliAuth>,
) -> Result<PreparedLive, LiveError> {
    let transport = np_sources::ReqwestTransport::new()?;
    prepare_auth(&transport, room_id, cancel, auth)
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<(), LiveError> {
    if cancel.load(Ordering::Acquire) {
        Err(LiveError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use np_sources::{Result as SourceResult, SourceError};
    use std::io::{Cursor, Read};
    use std::sync::Mutex;

    const LIVE: &str = include_str!("../fixtures/live_playinfo.json");
    const OFFLINE: &str = include_str!("../fixtures/offline_playinfo.json");
    const ERROR: &str = include_str!("../fixtures/error_playinfo.json");
    const ROOM: &str = include_str!("../fixtures/room_info.json");

    const ASC_48K_STEREO: [u8; 4] = [0xAF, 0x00, 0x11, 0x90];

    #[derive(Default)]
    struct Calls {
        urls: Vec<String>,
        streams: usize,
        referer: bool,
    }

    struct FixtureTransport {
        play: &'static str,
        room: &'static str,
        stream: Option<Vec<u8>>,
        calls: Mutex<Calls>,
    }

    impl FixtureTransport {
        fn live() -> Self {
            Self {
                play: LIVE,
                room: ROOM,
                stream: Some(synthetic_stream()),
                calls: Mutex::new(Calls::default()),
            }
        }
    }

    impl Transport for FixtureTransport {
        fn get(&self, url: &str) -> SourceResult<Vec<u8>> {
            let mut calls = self.calls.lock().unwrap();
            calls.urls.push(url.to_owned());
            if url.contains("getRoomPlayInfo") {
                assert!(
                    url.contains("only_audio=1"),
                    "URL missing only_audio: {url}"
                );
                assert!(url.contains("format=0"), "URL missing format: {url}");
                Ok(self.play.as_bytes().to_vec())
            } else {
                Ok(self.room.as_bytes().to_vec())
            }
        }

        fn open_stream(
            &self,
            _url: &str,
            headers: &[(&str, &str)],
        ) -> SourceResult<Box<dyn Read + Send>> {
            let mut calls = self.calls.lock().unwrap();
            calls.streams += 1;
            calls.referer = headers
                .iter()
                .any(|(name, value)| *name == "Referer" && *value == "https://live.bilibili.com/");
            let Some(stream) = &self.stream else {
                return Err(SourceError::Network("模拟连接失败".to_owned()));
            };
            Ok(Box::new(Cursor::new(stream.clone())))
        }
    }

    fn synthetic_stream() -> Vec<u8> {
        let mut bytes = b"FLV\x01".to_vec();
        bytes.push(4);
        bytes.extend_from_slice(&9u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&tag(18, b"\x00\x00\x00\x00\x00"));
        bytes.extend_from_slice(&tag(8, &ASC_48K_STEREO));
        for index in 0..4u8 {
            bytes.extend_from_slice(&tag(8, &[0xAF, 0x01, 0x21, index, 0x56, 0xE5, 0x00]));
        }
        bytes
    }

    fn tag(tag_type: u8, payload: &[u8]) -> Vec<u8> {
        let size = payload.len();
        let mut bytes = vec![
            tag_type,
            (size >> 16) as u8,
            (size >> 8) as u8,
            size as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&(size as u32 + 11).to_be_bytes());
        bytes
    }

    fn transport_with(play: &'static str, stream: Option<Vec<u8>>) -> FixtureTransport {
        FixtureTransport {
            play,
            room: ROOM,
            stream,
            calls: Mutex::new(Calls::default()),
        }
    }

    #[test]
    fn prepare_reads_only_audio_url_and_referer() {
        let transport = FixtureTransport::live();
        let cancel = AtomicBool::new(false);
        let prepared = prepare(&transport, 21622811, &cancel).expect("prepare live");
        assert_eq!(prepared.room_id, 21622811);
        assert!(!prepared.title.is_empty());
        assert_eq!(prepared.format.sample_rate, 48_000);
        assert_eq!(prepared.format.channels, 2);
        assert!(prepared.source.duration_ms().is_none());
        assert!(!prepared.source.is_seekable());

        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.streams, 1);
        assert!(calls.referer);
        assert!(calls.urls.iter().any(|url| url.contains("getRoomPlayInfo")));
    }

    #[test]
    fn prepare_maps_offline_fixture() {
        let transport = transport_with(OFFLINE, Some(synthetic_stream()));
        let cancel = AtomicBool::new(false);
        let error = prepare(&transport, 510, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::Offline));
        assert_eq!(transport.calls.lock().unwrap().streams, 0);
    }

    #[test]
    fn prepare_maps_api_error_fixture() {
        let transport = transport_with(ERROR, Some(synthetic_stream()));
        let cancel = AtomicBool::new(false);
        let error = prepare(&transport, 99999999, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::Api { code: 60004, .. }));
    }

    #[test]
    fn prepare_maps_stream_open_failure() {
        let transport = transport_with(LIVE, None);
        let cancel = AtomicBool::new(false);
        let error = prepare(&transport, 21622811, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::Source(SourceError::Network(_))));
    }

    #[test]
    fn prepare_is_cancelled_before_any_request() {
        let transport = FixtureTransport::live();
        let cancel = AtomicBool::new(true);
        let error = prepare(&transport, 21622811, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::Cancelled));
        let calls = transport.calls.lock().unwrap();
        assert!(calls.urls.is_empty());
        assert_eq!(calls.streams, 0);
    }

    #[test]
    fn prepare_rejects_zero_room() {
        let transport = FixtureTransport::live();
        let cancel = AtomicBool::new(false);
        let error = prepare(&transport, 0, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::InvalidRoom(_)));
        assert!(transport.calls.lock().unwrap().urls.is_empty());
    }

    #[test]
    fn prepare_rejects_invalid_flv_stream() {
        let transport = transport_with(LIVE, Some(vec![0u8; 16]));
        let cancel = AtomicBool::new(false);
        let error = prepare(&transport, 21622811, &cancel)
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::BadStream(_)));
    }
}
