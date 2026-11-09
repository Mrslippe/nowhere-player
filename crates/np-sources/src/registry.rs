//! 多个在线源的注册表：串行搜索、单源失败隔离（ADR-0004 §6）。
//!
//! 注册表本身不做网络节流：节流在 [`crate::ReqwestTransport`]（每源 300ms），
//! 命令层按顺序调用 [`SourceRegistry::search_all`] 即可。

use std::sync::Arc;

use crate::AudioSource;
use crate::error::SourceError;
use crate::model::{SourceId, SourceTrack};

/// 一次跨源搜索的合并结果。
#[derive(Debug)]
pub struct SearchMerge {
    /// 成功的源按注册顺序拼接；没有做全局截断，调用方按需截断。
    pub tracks: Vec<SourceTrack>,
    /// 失败的源及其错误；一个源失败不影响其它源。
    pub failures: Vec<(SourceId, SourceError)>,
}

/// 持有多个 [`AudioSource`] 的注册表。
pub struct SourceRegistry {
    sources: Vec<Arc<dyn AudioSource>>,
}

impl SourceRegistry {
    /// 按给定顺序注册源；顺序就是搜索结果与来源标记的顺序。
    pub fn new(sources: Vec<Arc<dyn AudioSource>>) -> Self {
        Self { sources }
    }

    /// 已注册的源（按顺序）。
    pub fn sources(&self) -> &[Arc<dyn AudioSource>] {
        &self.sources
    }

    /// 按 [`SourceId`] 找源；命令层 resolve 时用。
    pub fn source(&self, id: SourceId) -> Option<Arc<dyn AudioSource>> {
        self.sources
            .iter()
            .find(|source| source.id() == id)
            .cloned()
    }

    /// 串行搜索所有源，单源失败只记录错误。
    ///
    /// `limit` 是**每个源**的上限；命令层再把合并结果截到总上限，
    /// 避免一个源占满全部序号。
    pub fn search_all(&self, query: &str, limit: usize) -> SearchMerge {
        let mut tracks = Vec::new();
        let mut failures = Vec::new();
        if limit == 0 {
            return SearchMerge { tracks, failures };
        }

        for source in &self.sources {
            match source.search(query, limit) {
                Ok(mut found) => tracks.append(&mut found),
                Err(error) => failures.push((source.id(), error)),
            }
        }
        SearchMerge { tracks, failures }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::error::Result;
    use crate::model::{ResolvedStream, SourceId, SourceTrack};

    #[derive(Clone, Copy)]
    enum Failure {
        None,
        Unavailable,
        Network,
    }

    struct FakeSource {
        id: SourceId,
        tracks: Vec<SourceTrack>,
        failure: Failure,
        calls: AtomicUsize,
    }

    impl FakeSource {
        fn new(id: SourceId, titles: &[&str], failure: Failure) -> Arc<Self> {
            let tracks = titles
                .iter()
                .enumerate()
                .map(|(index, title)| SourceTrack {
                    source: id,
                    source_id: format!("{}-{index}", id),
                    title: (*title).to_string(),
                    artist: "测试艺人".to_string(),
                    duration_ms: 1_000,
                    album: None,
                    cover_url: None,
                })
                .collect();
            Arc::new(Self {
                id,
                tracks,
                failure,
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl AudioSource for FakeSource {
        fn id(&self) -> SourceId {
            self.id
        }

        fn search(&self, _query: &str, limit: usize) -> Result<Vec<SourceTrack>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.failure {
                Failure::None => Ok(self.tracks.iter().take(limit).cloned().collect()),
                Failure::Unavailable => Err(SourceError::Unavailable("不可用".to_string())),
                Failure::Network => Err(SourceError::Network("连接失败".to_string())),
            }
        }

        fn resolve(&self, _track: &SourceTrack) -> Result<ResolvedStream> {
            Err(SourceError::NotImplemented("测试不 resolve".to_string()))
        }
    }

    #[test]
    fn merge_keeps_registration_order_and_source_ids() {
        let bili = FakeSource::new(SourceId::Bilibili, &["B1", "B2"], Failure::None);
        let netease = FakeSource::new(SourceId::Netease, &["N1"], Failure::None);
        let registry = SourceRegistry::new(vec![bili.clone(), netease.clone()]);

        let merged = registry.search_all("测试", 10);

        assert!(merged.failures.is_empty());
        let titles: Vec<&str> = merged
            .tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect();
        assert_eq!(titles, ["B1", "B2", "N1"]);
        assert_eq!(merged.tracks[0].source, SourceId::Bilibili);
        assert_eq!(merged.tracks[2].source, SourceId::Netease);
        assert_eq!((bili.calls(), netease.calls()), (1, 1));
    }

    #[test]
    fn one_source_failure_does_not_hide_other_results() {
        let bili = FakeSource::new(SourceId::Bilibili, &[], Failure::Unavailable);
        let netease = FakeSource::new(SourceId::Netease, &["N1"], Failure::None);
        let registry = SourceRegistry::new(vec![bili, netease]);

        let merged = registry.search_all("测试", 5);

        assert_eq!(merged.tracks.len(), 1);
        assert_eq!(merged.failures.len(), 1);
        assert_eq!(merged.failures[0].0, SourceId::Bilibili);
        assert!(matches!(merged.failures[0].1, SourceError::Unavailable(_)));
    }

    #[test]
    fn all_sources_failing_still_returns_empty_merge() {
        let bili = FakeSource::new(SourceId::Bilibili, &[], Failure::Network);
        let netease = FakeSource::new(SourceId::Netease, &[], Failure::Unavailable);
        let registry = SourceRegistry::new(vec![bili, netease]);

        let merged = registry.search_all("测试", 5);

        assert!(merged.tracks.is_empty());
        assert_eq!(merged.failures.len(), 2);
    }

    #[test]
    fn limit_is_applied_per_source() {
        let bili = FakeSource::new(SourceId::Bilibili, &["B1", "B2", "B3"], Failure::None);
        let netease = FakeSource::new(SourceId::Netease, &["N1"], Failure::None);
        let registry = SourceRegistry::new(vec![bili, netease]);

        let merged = registry.search_all("测试", 1);

        assert_eq!(merged.tracks.len(), 2);
        assert_eq!(merged.tracks[0].title, "B1");
        assert_eq!(merged.tracks[1].title, "N1");
    }

    #[test]
    fn zero_limit_does_not_query_any_source() {
        let bili = FakeSource::new(SourceId::Bilibili, &["B1"], Failure::None);
        let registry = SourceRegistry::new(vec![bili.clone()]);

        let merged = registry.search_all("测试", 0);

        assert!(merged.tracks.is_empty());
        assert_eq!(bili.calls(), 0);
    }

    #[test]
    fn source_lookup_matches_id() {
        let bili = FakeSource::new(SourceId::Bilibili, &[], Failure::None);
        let registry = SourceRegistry::new(vec![bili]);

        assert!(registry.source(SourceId::Bilibili).is_some());
        assert!(registry.source(SourceId::Netease).is_none());
        assert_eq!(registry.sources().len(), 1);
    }

    #[test]
    fn registry_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SourceRegistry>();
    }
}
