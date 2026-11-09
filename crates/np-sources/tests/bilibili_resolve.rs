//! B 站取流（view → playurl）的离线测试：`Transport` 注入夹具，测试零网络。

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use np_sources::{AudioSource, BilibiliSource, SourceError, SourceId, SourceTrack, Transport};

const VIEW: &[u8] = include_bytes!("../fixtures/bilibili/view.json");
const PLAYURL: &[u8] = include_bytes!("../fixtures/bilibili/playurl.json");
const PLAYURL_DURL: &[u8] = include_bytes!("../fixtures/bilibili/playurl_durl.json");
const DEADLINE: u64 = 1_794_234_249;

#[derive(Clone, Debug)]
enum Reply {
    Body(Vec<u8>),
    Error(Kind),
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Network,
}

impl Kind {
    fn into_error(self) -> SourceError {
        match self {
            Self::Network => SourceError::Network("连接失败（夹具）".to_string()),
        }
    }
}

struct FakeTransport {
    view: Reply,
    playurl: Reply,
    calls: Mutex<Vec<String>>,
}

impl FakeTransport {
    fn new(view: Reply, playurl: Reply) -> Arc<Self> {
        Arc::new(Self {
            view,
            playurl,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl Transport for FakeTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(url.to_string());
        let reply = if url.contains("/x/player/playurl") {
            &self.playurl
        } else {
            &self.view
        };
        match reply {
            Reply::Body(body) => Ok(body.clone()),
            Reply::Error(kind) => Err(kind.into_error()),
        }
    }
}

fn source(view: Reply, playurl: Reply) -> (BilibiliSource, Arc<FakeTransport>) {
    let transport = FakeTransport::new(view, playurl);
    (BilibiliSource::new(transport.clone()), transport)
}

fn source_with_fixtures() -> (BilibiliSource, Arc<FakeTransport>) {
    source(Reply::Body(VIEW.to_vec()), Reply::Body(PLAYURL.to_vec()))
}

fn body(json: &str) -> Reply {
    Reply::Body(json.as_bytes().to_vec())
}

fn track(source_id: &str) -> SourceTrack {
    SourceTrack {
        source: SourceId::Bilibili,
        source_id: source_id.to_string(),
        title: "测试曲目".to_string(),
        artist: "测试作者".to_string(),
        duration_ms: 225_000,
        album: None,
        cover_url: None,
    }
}

#[test]
fn resolve_picks_highest_bandwidth_audio_mp4_and_reads_deadline() {
    let (source, transport) = source_with_fixtures();

    let stream = source.resolve(&track("BV1xx411c7mD")).expect("resolve");

    // 999999 的 flac 被跳过，最高码率的 audio/mp4 是 319102。
    assert_eq!(
        stream.url,
        "https://example.invalid/dash/audio-319102.m4s?deadline=1794234249&token=synthetic"
    );
    assert_eq!(stream.mime, "audio/mp4");
    assert_eq!(
        stream.expires_at,
        Some(UNIX_EPOCH + Duration::from_secs(DEADLINE))
    );
    assert!(
        stream
            .headers
            .iter()
            .any(|(name, value)| name == "Referer" && value == "https://www.bilibili.com"),
        "B 站下载必须带 Referer: {:?}",
        stream.headers
    );
    assert!(
        stream
            .headers
            .iter()
            .any(|(name, value)| name == "Origin" && value == "https://www.bilibili.com"),
        "B 站下载必须带 Origin: {:?}",
        stream.headers
    );

    let calls = transport.calls();
    assert_eq!(calls.len(), 2, "view + playurl");
    assert!(
        calls[0].contains("/x/web-interface/view?bvid=BV1xx411c7mD"),
        "{}",
        calls[0]
    );
    assert!(
        calls[1].contains("/x/player/playurl?bvid=BV1xx411c7mD&cid=508529777&fnval=16&fourk=1"),
        "{}",
        calls[1]
    );
}

#[test]
fn resolve_uses_pages_cid_when_top_level_cid_is_missing() {
    let view =
        body(r#"{"code":0,"message":"OK","data":{"bvid":"BV1xx411c7mD","pages":[{"cid":42}]}}"#);
    let (source, transport) = source(view, Reply::Body(PLAYURL.to_vec()));

    source.resolve(&track("BV1xx411c7mD")).expect("resolve");

    let calls = transport.calls();
    assert!(calls[1].contains("cid=42"), "{}", calls[1]);
}

#[test]
fn resolve_falls_back_to_durl_without_dash() {
    let (source, _transport) = source(
        Reply::Body(VIEW.to_vec()),
        Reply::Body(PLAYURL_DURL.to_vec()),
    );

    let stream = source.resolve(&track("BV1xx411c7mD")).expect("resolve");

    assert_eq!(
        stream.url,
        "https://example.invalid/durl/video-508529777.mp4"
    );
    assert_eq!(stream.mime, "video/mp4");
    assert_eq!(stream.expires_at, None, "durl 没有 deadline");
}

#[test]
fn resolve_rejects_flv_durl() {
    let playurl = body(
        r#"{"code":0,"data":{"format":"flv720","durl":[{"url":"https://example.invalid/video.flv"}]}}"#,
    );
    let (source, _transport) = source(Reply::Body(VIEW.to_vec()), playurl);

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("FLV 应报错");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("FLV"));
}

#[test]
fn resolve_rejects_dash_without_audio_mp4() {
    let playurl = body(
        r#"{"code":0,"data":{"dash":{"audio":[{"baseUrl":"https://example.invalid/a.flac","bandwidth":9,"mimeType":"audio/flac"}]}}}"#,
    );
    let (source, _transport) = source(Reply::Body(VIEW.to_vec()), playurl);

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("没有 mp4 应报错");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("audio/mp4"));
}

#[test]
fn resolve_without_deadline_has_no_expiry() {
    let playurl = body(
        r#"{"code":0,"data":{"dash":{"audio":[{"baseUrl":"https://example.invalid/a.m4s","bandwidth":1,"mimeType":"audio/mp4"}]}}}"#,
    );
    let (source, _transport) = source(Reply::Body(VIEW.to_vec()), playurl);

    let stream = source.resolve(&track("BV1xx411c7mD")).expect("resolve");

    assert_eq!(stream.expires_at, None);
}

#[test]
fn resolve_accepts_aid_source_id() {
    let (source, transport) = source_with_fixtures();

    source.resolve(&track("1001")).expect("resolve");

    let calls = transport.calls();
    assert!(
        calls[0].contains("/x/web-interface/view?aid=1001"),
        "{}",
        calls[0]
    );
    assert!(calls[1].contains("bvid=BV1xx411c7mD"), "{}", calls[1]);
}

#[test]
fn resolve_rejects_invalid_source_id_without_network() {
    let (source, transport) = source_with_fixtures();

    let error = source
        .resolve(&track("not-an-id"))
        .expect_err("非法 id 应报错");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(transport.calls().is_empty());
}

#[test]
fn resolve_view_api_error_is_unavailable() {
    let view = body(r#"{"code":-404,"message":"啥都木有","data":null}"#);
    let (source, transport) = source(view, Reply::Body(PLAYURL.to_vec()));

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("view 错误");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 1, "view 失败后不应再调 playurl");
}

#[test]
fn resolve_playurl_api_error_is_unavailable() {
    let playurl = body(r#"{"code":-404,"message":"啥都木有","data":null}"#);
    let (source, transport) = source(Reply::Body(VIEW.to_vec()), playurl);

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("playurl 错误");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 2);
}

#[test]
fn resolve_view_network_error_propagates() {
    let (source, _transport) = source(Reply::Error(Kind::Network), Reply::Body(PLAYURL.to_vec()));

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("网络错误");

    assert!(matches!(error, SourceError::Network(_)), "{error:?}");
}

#[test]
fn resolve_invalid_view_json_is_parse_error() {
    let (source, _transport) = source(body("{not json"), Reply::Body(PLAYURL.to_vec()));

    let error = source
        .resolve(&track("BV1xx411c7mD"))
        .expect_err("非法 JSON");

    assert!(matches!(error, SourceError::Parse(_)), "{error:?}");
}

#[test]
fn resolve_view_without_cid_is_parse_error() {
    let view = body(r#"{"code":0,"data":{"bvid":"BV1xx411c7mD"}}"#);
    let (source, _transport) = source(view, Reply::Body(PLAYURL.to_vec()));

    let error = source.resolve(&track("BV1xx411c7mD")).expect_err("缺 cid");

    assert!(matches!(error, SourceError::Parse(_)), "{error:?}");
}

#[test]
fn resolve_rejects_overlong_track_without_network() {
    let (source, transport) = source_with_fixtures();
    let mut track = track("BV1xx411c7mD");
    track.duration_ms = np_sources::MAX_TRACK_DURATION_MS + 1;

    let error = source.resolve(&track).expect_err("超长曲目应拒绝");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("30 分钟"), "{error}");
    assert!(transport.calls().is_empty(), "不应触网");
}
