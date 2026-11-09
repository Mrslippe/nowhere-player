//! 网易云匿名搜索与取流的离线测试：`Transport` 注入夹具，测试零网络。

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use np_sources::{AudioSource, NeteaseSource, SourceError, SourceId, SourceTrack, Transport};

const SEARCH_OK: &[u8] = include_bytes!("../fixtures/netease/search_ok.json");
const SEARCH_EMPTY: &[u8] = include_bytes!("../fixtures/netease/search_empty.json");
const PLAYER_OK: &[u8] = include_bytes!("../fixtures/netease/player_url_ok.json");
const PLAYER_NULL: &[u8] = include_bytes!("../fixtures/netease/player_url_null.json");
const PLAYER_128: &[u8] = include_bytes!("../fixtures/netease/player_url_128.json");

#[derive(Clone, Debug)]
enum Reply {
    Body(Vec<u8>),
    Error(Kind),
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Network,
    Http(u16),
}

impl Kind {
    fn into_error(self) -> SourceError {
        match self {
            Self::Network => SourceError::Network("连接失败（夹具）".to_string()),
            Self::Http(status) => SourceError::Http(status),
        }
    }
}

struct FakeTransport {
    search: Reply,
    detail: Reply,
    players: Mutex<Vec<Reply>>,
    calls: Mutex<Vec<String>>,
}

impl FakeTransport {
    fn new(search: Reply, players: Vec<Reply>) -> Arc<Self> {
        Self::with_detail(search, players, body(r#"{"code":200,"songs":[]}"#))
    }

    fn with_detail(search: Reply, players: Vec<Reply>, detail: Reply) -> Arc<Self> {
        Arc::new(Self {
            search,
            detail,
            players: Mutex::new(players),
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

        let reply = if url.contains("/api/search/get") {
            self.search.clone()
        } else if url.contains("/api/song/detail") {
            self.detail.clone()
        } else {
            let mut players = self
                .players
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if players.is_empty() {
                return Err(SourceError::Network("测试没有准备取流响应".to_string()));
            }
            players.remove(0)
        };

        match reply {
            Reply::Body(body) => Ok(body),
            Reply::Error(kind) => Err(kind.into_error()),
        }
    }
}

fn source(search: Reply, players: Vec<Reply>) -> (NeteaseSource, Arc<FakeTransport>) {
    let transport = FakeTransport::new(search, players);
    (NeteaseSource::new(transport.clone()), transport)
}

fn source_with_detail(
    search: Reply,
    players: Vec<Reply>,
    detail: Reply,
) -> (NeteaseSource, Arc<FakeTransport>) {
    let transport = FakeTransport::with_detail(search, players, detail);
    (NeteaseSource::new(transport.clone()), transport)
}

fn body(json: &str) -> Reply {
    Reply::Body(json.as_bytes().to_vec())
}

fn track(source_id: &str, duration_ms: u64) -> SourceTrack {
    SourceTrack {
        source: SourceId::Netease,
        source_id: source_id.to_string(),
        title: "测试曲目".to_string(),
        artist: "测试歌手".to_string(),
        duration_ms,
        album: None,
        cover_url: None,
    }
}

#[test]
fn search_maps_old_and_new_fields_and_filters_overlong() {
    let (source, transport) = source(Reply::Body(SEARCH_OK.to_vec()), Vec::new());

    let tracks = source.search("测试", 10).expect("夹具应能解析");

    // 5 条里 2 条非法：10003 超过 30 分钟、id=0。
    assert_eq!(tracks.len(), 3);

    let first = &tracks[0];
    assert_eq!(first.source, SourceId::Netease);
    assert_eq!(first.source_id, "10001");
    assert_eq!(first.title, "测试歌曲一");
    assert_eq!(first.artist, "测试歌手A,测试歌手B");
    assert_eq!(first.duration_ms, 238_698);
    assert_eq!(first.album.as_deref(), Some("测试专辑"));
    assert_eq!(
        first.cover_url.as_deref(),
        Some("https://p1.music.126.net/example-cover.jpg")
    );

    let second = &tracks[1];
    assert_eq!(second.source_id, "10002");
    assert_eq!(second.artist, "测试歌手C");
    assert_eq!(second.duration_ms, 200_000);
    assert_eq!(
        second.cover_url.as_deref(),
        Some("https://p1.music.126.net/example-cover-2.jpg")
    );

    // 恰好 30:00 保留，严格大于才过滤。
    let boundary = &tracks[2];
    assert_eq!(boundary.source_id, "10004");
    assert_eq!(boundary.duration_ms, 1_800_000);
    assert_eq!(boundary.cover_url, None);

    assert!(!tracks.iter().any(|track| track.source_id == "10003"));
    let calls = transport.calls();
    // 搜索 + 为唯一缺封面的 10004 补一次 detail。
    assert_eq!(calls.len(), 2);
    assert!(
        calls[0].contains("/api/search/get?s=%E6%B5%8B%E8%AF%95&type=1&limit=10"),
        "{}",
        calls[0]
    );
    assert!(
        calls[1].contains("/api/song/detail?ids=%5B10004%5D"),
        "{}",
        calls[1]
    );
}

#[test]
fn search_empty_returns_empty_vec() {
    let (source, _transport) = source(Reply::Body(SEARCH_EMPTY.to_vec()), Vec::new());

    let tracks = source.search("没有结果", 5).expect("空结果不是错误");

    assert!(tracks.is_empty());
}

#[test]
fn search_limit_truncates_results() {
    let (source, transport) = source(Reply::Body(SEARCH_OK.to_vec()), Vec::new());

    let tracks = source.search("测试", 2).expect("夹具应能解析");

    assert_eq!(tracks.len(), 2);
    assert!(transport.calls()[0].contains("limit=2"));
}

#[test]
fn search_limit_zero_makes_no_request() {
    let (source, transport) = source(Reply::Body(SEARCH_OK.to_vec()), Vec::new());

    assert!(source.search("测试", 0).expect("limit=0").is_empty());
    assert!(transport.calls().is_empty());
}

#[test]
fn search_api_error_code_is_unavailable() {
    let (source, _transport) = source(body(r#"{"code":-460,"message":"Cheating"}"#), Vec::new());

    let error = source.search("测试", 5).expect_err("风控应报错");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("-460"));
}

#[test]
fn search_http_500_maps_to_http_variant() {
    let (source, _transport) = source(Reply::Error(Kind::Http(500)), Vec::new());

    let error = source.search("测试", 5).expect_err("500 应报错");

    assert!(matches!(error, SourceError::Http(500)), "{error:?}");
}

#[test]
fn search_invalid_json_maps_to_parse() {
    let (source, _transport) = source(body("{not json"), Vec::new());

    let error = source.search("测试", 5).expect_err("非法 JSON");

    assert!(matches!(error, SourceError::Parse(_)), "{error:?}");
}

#[test]
fn resolve_320k_maps_https_mime_and_expiry() {
    let before = SystemTime::now();
    let (source, transport) = source(
        Reply::Body(SEARCH_OK.to_vec()),
        vec![Reply::Body(PLAYER_OK.to_vec())],
    );

    let stream = source
        .resolve(&track("10001", 238_698))
        .expect("320k 应可用");
    let after = SystemTime::now();

    assert_eq!(stream.url, "https://example.invalid/netease/320.mp3");
    assert_eq!(stream.mime, "audio/mpeg");
    assert!(stream.headers.is_empty(), "网易云 CDN 不需要 Referer");
    let expires = stream.expires_at.expect("expi=1200 应给出过期时间");
    assert!(expires >= before + Duration::from_secs(1200));
    assert!(expires <= after + Duration::from_secs(1200));

    let calls = transport.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains("/api/song/enhance/player/url?ids=%5B10001%5D&br=320000"),
        "{}",
        calls[0]
    );
}

#[test]
fn resolve_falls_back_through_bitrates() {
    let (source, transport) = source(
        Reply::Body(SEARCH_OK.to_vec()),
        vec![
            Reply::Body(PLAYER_NULL.to_vec()),
            Reply::Body(PLAYER_NULL.to_vec()),
            Reply::Body(PLAYER_128.to_vec()),
        ],
    );

    let stream = source
        .resolve(&track("10001", 238_698))
        .expect("应降级到 128k");

    assert_eq!(stream.url, "https://example.invalid/netease/128.mp3");
    let calls = transport.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls[0].contains("br=320000"), "{}", calls[0]);
    assert!(calls[1].contains("br=192000"), "{}", calls[1]);
    assert!(calls[2].contains("br=128000"), "{}", calls[2]);
}

#[test]
fn resolve_unplayable_has_human_readable_message() {
    let (source, transport) = source(
        Reply::Body(SEARCH_OK.to_vec()),
        vec![
            Reply::Body(PLAYER_NULL.to_vec()),
            Reply::Body(PLAYER_NULL.to_vec()),
            Reply::Body(PLAYER_NULL.to_vec()),
        ],
    );

    let error = source
        .resolve(&track("10002", 200_000))
        .expect_err("版权受限应报错");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("该曲目在网易云不可播放"));
    assert!(error.to_string().contains("版权受限") || error.to_string().contains("会员"));
    assert_eq!(error.code(), "E_SRC_UNAVAILABLE");
    assert_eq!(transport.calls().len(), 3);
}

#[test]
fn resolve_api_error_does_not_fall_back() {
    let (source, transport) = source(
        Reply::Body(SEARCH_OK.to_vec()),
        vec![body(r#"{"code":-460,"message":"Cheating","data":null}"#)],
    );

    let error = source
        .resolve(&track("10001", 238_698))
        .expect_err("接口错误");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 1, "接口级错误不应再试低码率");
}

#[test]
fn resolve_overlong_track_rejected_without_network() {
    let (source, transport) = source(Reply::Body(SEARCH_OK.to_vec()), Vec::new());

    let error = source
        .resolve(&track("10001", np_sources::MAX_TRACK_DURATION_MS + 1))
        .expect_err("超长曲目应拒绝");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("30 分钟"));
    assert!(transport.calls().is_empty());
}

#[test]
fn resolve_invalid_song_id_without_network() {
    let (source, transport) = source(Reply::Body(SEARCH_OK.to_vec()), Vec::new());

    let error = source
        .resolve(&track("not-an-id", 238_698))
        .expect_err("非法 ID 应拒绝");

    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(transport.calls().is_empty());
}

#[test]
fn resolve_without_expi_has_no_expiry() {
    let player = body(
        r#"{"code":200,"data":[{"url":"http://example.invalid/netease/no-expi.mp3","code":200,"type":"mp3"}]}"#,
    );
    let (source, _transport) = source(Reply::Body(SEARCH_OK.to_vec()), vec![player]);

    let stream = source
        .resolve(&track("10001", 238_698))
        .expect("没有 expi 也应能播");

    assert_eq!(stream.expires_at, None);
    assert_eq!(stream.url, "https://example.invalid/netease/no-expi.mp3");
}

#[test]
fn resolve_network_error_propagates() {
    let (source, transport) = source(
        Reply::Body(SEARCH_OK.to_vec()),
        vec![Reply::Error(Kind::Network)],
    );

    let error = source
        .resolve(&track("10001", 238_698))
        .expect_err("网络错误");

    assert!(matches!(error, SourceError::Network(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn search_enriches_missing_cover_from_detail() {
    let detail = body(
        r#"{"code":200,"songs":[{"id":10004,"album":{"picUrl":"http://p1.music.126.net/boundary.jpg"}}]}"#,
    );
    let (source, transport) =
        source_with_detail(Reply::Body(SEARCH_OK.to_vec()), Vec::new(), detail);

    let tracks = source.search("测试", 10).expect("夹具应能解析");

    assert_eq!(
        tracks[2].cover_url.as_deref(),
        Some("https://p1.music.126.net/boundary.jpg")
    );
    assert!(
        transport
            .calls()
            .iter()
            .any(|url| url.contains("/api/song/detail?ids=%5B10004%5D"))
    );
}

#[test]
fn search_detail_failure_does_not_break_search() {
    let (source, _transport) = source_with_detail(
        Reply::Body(SEARCH_OK.to_vec()),
        Vec::new(),
        Reply::Error(Kind::Network),
    );

    let tracks = source.search("测试", 10).expect("封面失败不影响搜索");

    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[2].source_id, "10004");
    assert_eq!(tracks[2].cover_url, None);
}

#[test]
fn search_batches_detail_ids_by_twenty() {
    let songs: Vec<String> = (1..=25)
        .map(|id| {
            format!(r#"{{"id":{id},"name":"歌{id}","duration":120000,"artists":[],"album":{{}}}}"#)
        })
        .collect();
    let search = body(&format!(
        r#"{{"code":200,"result":{{"songs":[{}]}}}}"#,
        songs.join(",")
    ));
    let (source, transport) = source(search, Vec::new());

    let tracks = source.search("测试", 100).expect("夹具应能解析");

    assert_eq!(tracks.len(), 25);
    let calls = transport.calls();
    let details: Vec<&String> = calls
        .iter()
        .filter(|url| url.contains("/api/song/detail"))
        .collect();
    assert_eq!(details.len(), 2);
    // id 之间用编码后的 %2C：20 个 id 有 19 个，剩下 5 个有 4 个。
    assert_eq!(details[0].matches("%2C").count(), 19);
    assert_eq!(details[1].matches("%2C").count(), 4);
}
