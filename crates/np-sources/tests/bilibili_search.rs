//! B 站搜索的离线测试：`Transport` 注入夹具，测试代码不含任何网络调用。

use std::sync::{Arc, Mutex};

use np_sources::{AudioSource, BilibiliSource, SourceError, SourceId, SourceTrack, Transport};

const NAV: &[u8] = include_bytes!("../fixtures/bilibili/nav.json");
const SEARCH_OK: &[u8] = include_bytes!("../fixtures/bilibili/search_ok.json");
const SEARCH_EMPTY: &[u8] = include_bytes!("../fixtures/bilibili/search_empty.json");
const SEARCH_DENIED: &[u8] = include_bytes!("../fixtures/bilibili/search_denied.json");

#[derive(Clone, Debug)]
enum Reply {
    Body(Vec<u8>),
    Error(Kind),
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Http(u16),
    Network,
    Unavailable,
}

impl Kind {
    fn into_error(self) -> SourceError {
        match self {
            Self::Http(status) => SourceError::Http(status),
            Self::Network => SourceError::Network("连接被拒绝（夹具）".to_string()),
            Self::Unavailable => SourceError::Unavailable("风控（夹具）".to_string()),
        }
    }
}

struct FakeTransport {
    nav: Reply,
    search: Reply,
    calls: Mutex<Vec<String>>,
}

impl FakeTransport {
    fn new(nav: Reply, search: Reply) -> Arc<Self> {
        Arc::new(Self {
            nav,
            search,
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
        let reply = if url.contains("/x/web-interface/nav") {
            &self.nav
        } else {
            &self.search
        };
        match reply {
            Reply::Body(body) => Ok(body.clone()),
            Reply::Error(kind) => Err(kind.into_error()),
        }
    }
}

fn source_ok() -> (BilibiliSource, Arc<FakeTransport>) {
    let transport = FakeTransport::new(Reply::Body(NAV.to_vec()), Reply::Body(SEARCH_OK.to_vec()));
    (BilibiliSource::new(transport.clone()), transport)
}

fn source_with(search: Reply) -> (BilibiliSource, Arc<FakeTransport>) {
    let transport = FakeTransport::new(Reply::Body(NAV.to_vec()), search);
    (BilibiliSource::new(transport.clone()), transport)
}

fn sample_track() -> SourceTrack {
    SourceTrack {
        source: SourceId::Bilibili,
        source_id: "BV1xx411c7mD".to_string(),
        title: "测试曲目".to_string(),
        artist: "测试作者".to_string(),
        duration_ms: 225_000,
        album: None,
        cover_url: None,
    }
}

#[test]
fn search_ok_maps_fields_and_filters_unplayable_items() {
    let (source, transport) = source_ok();
    let tracks = source.search("测试", 10).expect("夹具应能解析");

    // 5 条里 2 条非法（时长 LIVE / id 为空），只保留 3 条。
    assert_eq!(tracks.len(), 3);

    let first = &tracks[0];
    assert_eq!(first.source, SourceId::Bilibili);
    assert_eq!(first.source_id, "BV1xx411c7mD");
    assert_eq!(first.title, "测试搜索视频 & 标题");
    assert_eq!(first.artist, "测试作者A");
    assert_eq!(first.duration_ms, 225_000);
    assert_eq!(first.album, None);
    assert_eq!(
        first.cover_url.as_deref(),
        Some("https://i0.hdslb.com/bfs/archive/example-cover-a.jpg")
    );

    assert_eq!(tracks[1].duration_ms, 57_729_000);
    assert_eq!(tracks[1].cover_url, None);

    assert_eq!(tracks[2].title, "带实体 '引号' 与 <尖括号>");
    assert_eq!(tracks[2].cover_url, None);

    // nav + search 各一次。
    assert_eq!(transport.calls().len(), 2);
}

#[test]
fn search_url_contains_wbi_signature_and_sorted_params() {
    let (source, transport) = source_ok();
    source.search("周杰伦", 2).expect("夹具应能解析");

    let calls = transport.calls();
    assert!(calls[0].ends_with("/x/web-interface/nav"), "{}", calls[0]);
    let url = &calls[1];
    assert!(url.starts_with("https://api.bilibili.com/x/web-interface/wbi/search/type?"));
    assert!(url.contains("keyword=%E5%91%A8%E6%9D%B0%E4%BC%A6"));
    assert!(url.contains("search_type=video"));
    assert!(url.contains("page=1"));
    assert!(url.contains("page_size=2"));
    assert!(url.contains("&wts="));

    // wbi 要求参数按 key 升序，w_rid 追加在最后。
    let keyword = url.find("keyword=").expect("keyword");
    let page = url.find("page=").expect("page");
    let page_size = url.find("page_size=").expect("page_size");
    let search_type = url.find("search_type=").expect("search_type");
    let wts = url.find("&wts=").expect("wts");
    let w_rid = url.find("&w_rid=").expect("w_rid");
    assert!(keyword < page && page < page_size && page_size < search_type);
    assert!(search_type < wts && wts < w_rid);

    let rid = &url[w_rid + "&w_rid=".len()..];
    assert_eq!(rid.len(), 32);
    assert!(rid.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn search_empty_result_null_returns_empty_vec() {
    let (source, _transport) = source_with(Reply::Body(SEARCH_EMPTY.to_vec()));
    let tracks = source.search("没有结果", 5).expect("空结果不是错误");
    assert!(tracks.is_empty());
}

#[test]
fn search_empty_result_array_returns_empty_vec() {
    let (source, _transport) = source_with(Reply::Body(
        r#"{"code":0,"message":"OK","data":{"result":[]}}"#.as_bytes().to_vec(),
    ));
    let tracks = source.search("没有结果", 5).expect("空结果不是错误");
    assert!(tracks.is_empty());
}

#[test]
fn search_denied_412_is_unavailable() {
    let (source, _transport) = source_with(Reply::Body(SEARCH_DENIED.to_vec()));
    let error = source.search("测试", 5).expect_err("风控应报错");
    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("-412"));
    assert_eq!(error.code(), "E_SRC_UNAVAILABLE");
}

#[test]
fn search_api_error_code_403_is_unavailable() {
    let (source, _transport) = source_with(Reply::Body(
        r#"{"code":-403,"message":"访问权限不足","data":null}"#
            .as_bytes()
            .to_vec(),
    ));
    let error = source.search("测试", 5).expect_err("签名错应报错");
    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert!(error.to_string().contains("-403"));
}

#[test]
fn search_http_500_maps_to_http_variant() {
    let (source, _transport) = source_with(Reply::Error(Kind::Http(500)));
    let error = source.search("测试", 5).expect_err("500 应报错");
    assert!(matches!(error, SourceError::Http(500)), "{error:?}");
}

#[test]
fn search_connection_failure_maps_to_network() {
    let (source, _transport) = source_with(Reply::Error(Kind::Network));
    let error = source.search("测试", 5).expect_err("连接失败应报错");
    assert!(matches!(error, SourceError::Network(_)), "{error:?}");
}

#[test]
fn search_invalid_json_maps_to_parse() {
    let (source, _transport) = source_with(Reply::Body(b"{not json".to_vec()));
    let error = source.search("测试", 5).expect_err("非法 JSON 应报错");
    assert!(matches!(error, SourceError::Parse(_)), "{error:?}");
}

#[test]
fn search_limit_truncates_results() {
    let (source, transport) = source_ok();
    let tracks = source.search("测试", 2).expect("夹具应能解析");
    assert_eq!(tracks.len(), 2);
    assert!(transport.calls()[1].contains("page_size=2"));
}

#[test]
fn search_limit_zero_makes_no_request() {
    let (source, transport) = source_ok();
    let tracks = source.search("测试", 0).expect("limit=0 直接返回");
    assert!(tracks.is_empty());
    assert!(transport.calls().is_empty());
}

#[test]
fn search_limit_is_clamped_to_max_page_size() {
    let (source, transport) = source_ok();
    source.search("测试", 999).expect("夹具应能解析");
    assert!(transport.calls()[1].contains("page_size=50"));
}

#[test]
fn resolve_returns_not_implemented_without_network() {
    let (source, transport) = source_ok();
    let error = source
        .resolve(&sample_track())
        .expect_err("本卡只返回占位错误");
    assert!(matches!(error, SourceError::NotImplemented(_)), "{error:?}");
    assert_eq!(error.code(), "E_SRC_NOT_IMPLEMENTED");
    assert!(transport.calls().is_empty());
}

#[test]
fn nav_missing_wbi_img_is_unavailable() {
    let transport = FakeTransport::new(
        Reply::Body(r#"{"code":-101,"message":"账号未登录","data":{}}"#.as_bytes().to_vec()),
        Reply::Error(Kind::Unavailable),
    );
    let source = BilibiliSource::new(transport.clone());
    let error = source.search("测试", 5).expect_err("取不到 key 应报错");
    assert!(matches!(error, SourceError::Unavailable(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn nav_failure_propagates() {
    let transport =
        FakeTransport::new(Reply::Error(Kind::Network), Reply::Body(SEARCH_OK.to_vec()));
    let source = BilibiliSource::new(transport.clone());
    let error = source.search("测试", 5).expect_err("nav 失败应报错");
    assert!(matches!(error, SourceError::Network(_)), "{error:?}");
    assert_eq!(transport.calls().len(), 1);
}

#[test]
fn nav_keys_are_cached_between_searches() {
    let (source, transport) = source_ok();
    source.search("测试", 1).expect("第一次搜索");
    source.search("测试", 1).expect("第二次搜索");

    let calls = transport.calls();
    assert_eq!(calls.len(), 3, "nav + search + search");
    assert_eq!(
        calls
            .iter()
            .filter(|url| url.contains("/x/web-interface/nav"))
            .count(),
        1
    );
}
