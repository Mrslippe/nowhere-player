//! T-0049：UP 空间检索的离线测试。

use std::sync::{Arc, Mutex};

use np_sources::{BilibiliSource, SourceError, SourceId, Transport, UpUser, resolve_up_user};

const NAV: &[u8] = include_bytes!("../fixtures/bilibili/nav.json");
const USERS: &[u8] = include_bytes!("../fixtures/bilibili/bili_user.json");
const ARCHIVES: &[u8] = include_bytes!("../fixtures/bilibili/rec_archives.json");
const EMPTY: &[u8] = include_bytes!("../fixtures/bilibili/rec_archives_empty.json");
const DENIED: &[u8] = include_bytes!("../fixtures/bilibili/rec_archives_denied.json");

#[derive(Clone, Copy)]
enum Body {
    Users,
    Archives,
    Empty,
    Denied,
    Http412,
}

struct RouteTransport {
    body: Body,
    calls: Mutex<Vec<String>>,
}

impl RouteTransport {
    fn new(body: Body) -> Arc<Self> {
        Arc::new(Self {
            body,
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

impl Transport for RouteTransport {
    fn get(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(url.to_owned());
        if url.contains("/nav") {
            return Ok(NAV.to_vec());
        }
        if url.contains("search_type=bili_user") {
            return Ok(match self.body {
                Body::Users => USERS.to_vec(),
                _ => USERS.to_vec(),
            });
        }
        if url.contains("recArchivesByKeywords") {
            if matches!(self.body, Body::Http412) {
                return Err(SourceError::Http(412));
            }
            return Ok(match self.body {
                Body::Archives => ARCHIVES.to_vec(),
                Body::Empty => EMPTY.to_vec(),
                Body::Denied => DENIED.to_vec(),
                Body::Users => ARCHIVES.to_vec(),
                Body::Http412 => unreachable!(),
            });
        }
        Err(SourceError::Unavailable(format!("unexpected URL: {url}")))
    }
}

fn source(body: Body) -> (BilibiliSource, Arc<RouteTransport>) {
    let transport = RouteTransport::new(body);
    (BilibiliSource::new(transport.clone()), transport)
}

#[test]
fn user_search_parses_and_resolves_duplicate_names() {
    let (source, transport) = source(Body::Users);

    let users = source.search_users("明前奶绿").expect("users");
    assert_eq!(users.len(), 3);
    let resolved = resolve_up_user(&users, "明前奶绿").expect("exact");
    assert_eq!(resolved.user.mid, 2132180406);
    assert_eq!(resolved.user.fans, 310066);
    assert_eq!(resolved.same_name, 2);

    // 命中 1 小时缓存：第二次不再请求用户搜索。
    let before = transport.calls().len();
    let again = source.search_users("明前奶绿").expect("cached");
    assert_eq!(again, users);
    assert_eq!(transport.calls().len(), before);
}

#[test]
fn user_archives_returns_target_and_uses_short_cache() {
    let (source, transport) = source(Body::Archives);

    let tracks = source
        .user_archives(2132180406, "明前奶绿", "最长的电影", 30)
        .expect("archives");
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].source, SourceId::Bilibili);
    assert_eq!(tracks[0].source_id, "BV1rG411n7qp");
    assert_eq!(tracks[0].artist, "明前奶绿");
    assert_eq!(tracks[0].duration_ms, 236_000);
    assert!(transport.calls().iter().any(|url| {
        url.contains("keywords=%E6%9C%80%E9%95%BF%E7%9A%84%E7%94%B5%E5%BD%B1")
            && url.contains("ps=30&pn=1")
    }));

    let before = transport.calls().len();
    let again = source
        .user_archives(2132180406, "明前奶绿", "最长的电影", 30)
        .expect("cached");
    assert_eq!(again, tracks);
    assert_eq!(transport.calls().len(), before);
}

#[test]
fn empty_archives_is_empty_not_error() {
    let (source, _) = source(Body::Empty);
    assert!(
        source
            .user_archives(2132180406, "明前奶绿", "不存在", 30)
            .expect("empty")
            .is_empty()
    );
}

#[test]
fn denied_412_maps_to_human_rate_limit_without_retry() {
    let (source, transport) = source(Body::Denied);

    let error = source
        .user_archives(2132180406, "明前奶绿", "最长的电影", 30)
        .expect_err("412");
    assert!(error.to_string().contains("B 站限流"));
    assert!(error.to_string().contains("未自动重试"));
    // 只发一次空间请求，不自动重试。
    assert_eq!(
        transport
            .calls()
            .iter()
            .filter(|url| url.contains("recArchivesByKeywords"))
            .count(),
        1
    );
}

#[test]
fn http_412_maps_to_human_rate_limit_without_retry() {
    let (source, transport) = source(Body::Http412);

    let error = source
        .user_archives(2132180406, "明前奶绿", "最长的电影", 30)
        .expect_err("HTTP 412");
    assert!(error.to_string().contains("B 站限流"));
    assert!(
        transport
            .calls()
            .iter()
            .filter(|url| url.contains("recArchivesByKeywords"))
            .count()
            == 1
    );
}

#[test]
fn no_keyword_still_uses_one_space_request() {
    let (source, transport) = source(Body::Archives);

    source
        .user_archives(2132180406, "明前奶绿", "", 30)
        .expect("recent");
    let spaces = transport
        .calls()
        .iter()
        .filter(|url| url.contains("recArchivesByKeywords"))
        .count();
    assert_eq!(spaces, 1);
}

#[test]
fn resolved_up_helper_rejects_empty_users() {
    assert!(resolve_up_user(&[], "明前奶绿").is_none());
    let users = vec![UpUser {
        mid: 1,
        name: "别人".to_owned(),
        fans: 1,
    }];
    assert_eq!(
        resolve_up_user(&users, "明前奶绿")
            .expect("fallback")
            .user
            .mid,
        1
    );
}
