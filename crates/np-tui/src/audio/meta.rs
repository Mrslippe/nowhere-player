//! Live 房间资料（主播 / 分区 / 同接数）的拉取与 3s 刷新。
//!
//! 拆出 `audio.rs` 只是为了让主文件保持在 1800 行卫生上限内；线程与状态仍由
//! [`TuiAudio`] 持有，本模块只做 best-effort 拉取和定时触发。
//!
//! 同接人数不再取 `room_info.online`，而是公开的
//! `queryContributionRank?type=online_rank` 的 `data.count`；`ruid` 优先用
//! `room_info.uid`，缺失时回退 `room_init.uid`。

use std::time::{Duration, Instant};

use super::TuiAudio;

/// 同接数刷新间隔（T-0101）：3 秒，单飞行请求。
pub(crate) const LIVE_META_REFRESH: Duration = Duration::from_secs(3);

/// Live 页展示用的房间资料：主播名 + 分区 + 同接人数。
///
/// 由独立的 `np-live-meta` 线程 best-effort 拉取；准备线程只负责声音，
/// 资料迟到或失败都不影响直播播放。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveMeta {
    pub(crate) anchor: String,
    pub(crate) area: String,
    pub(crate) live_started_at: Option<u64>,
    /// 同接人数；接口失败或缺失时 `None`，浮层显示 ` -- `。
    pub(crate) online: Option<u64>,
}

impl TuiAudio {
    /// 浮层右上角的同接人数；资料未到时 `None`（显示 ` -- `）。
    pub(crate) fn live_online(&self) -> Option<u64> {
        self.live_meta
            .borrow()
            .as_ref()
            .and_then(|meta| meta.online)
    }

    /// 启动一次后台房间资料拉取；已有请求在飞时直接返回（单飞行、防重入）。
    /// 旧 receiver 被替换后旧结果自然丢弃。
    pub(super) fn spawn_live_meta(&self, generation: u64, room_id: u64) {
        if self.live_meta_rx.borrow().is_some() {
            return;
        }
        let (meta_tx, meta_rx) = std::sync::mpsc::channel();
        *self.live_meta_rx.borrow_mut() = Some(meta_rx);
        let spawn = std::thread::Builder::new()
            .name("np-live-meta".to_owned())
            .spawn(move || {
                let Ok(transport) = np_sources::ReqwestTransport::new() else {
                    return;
                };
                let _ = meta_tx.send((generation, fetch_live_meta(&transport, room_id)));
            });
        if spawn.is_err() {
            *self.live_meta_rx.borrow_mut() = None;
        }
    }

    /// 每 3s 刷新同接数 / 分区；只在真实直播（由 `start_live` 设置过
    /// 刷新时刻）时生效，测试里的 `live_for_test` 不会触网。
    pub(super) fn refresh_live_meta_if_due(&self) {
        if !self.is_live() {
            self.live_meta_refresh_at.set(None);
            return;
        }
        let Some(due) = self.live_meta_refresh_at.get() else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        // 单飞行：上一次请求还没回传时，把刷新顺延到下一次 tick 再判断。
        if self.live_meta_rx.borrow().is_some() {
            return;
        }
        self.live_meta_refresh_at
            .set(Some(Instant::now() + LIVE_META_REFRESH));
        let generation = self.live_generation.get();
        let room = self.live_room.get();
        if room != 0 {
            self.spawn_live_meta(generation, room);
        }
    }

    /// 收下后台线程拉到的资料；generation 过期就丢掉，并在单次回传后
    /// 释放 receiver（下一条刷新才能再发请求）。
    pub(super) fn drain_live_meta(&self) -> bool {
        let mut changed = false;
        loop {
            let (received, done) = {
                let receiver = self.live_meta_rx.borrow();
                match receiver.as_ref() {
                    None => (None, true),
                    Some(receiver) => match receiver.try_recv() {
                        Ok(value) => (Some(value), true),
                        Err(std::sync::mpsc::TryRecvError::Empty) => (None, false),
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => (None, true),
                    },
                }
            };
            if done {
                *self.live_meta_rx.borrow_mut() = None;
            }
            let Some((generation, mut meta)) = received else {
                break;
            };
            if generation != self.live_generation.get() {
                continue;
            }
            // 主播名 / 分区是慢变信息：单次请求失败时保留上一次的值；
            // 同接数按本次结果覆盖（失败即 ` -- `），不显示过期数字。
            if let Some(old) = self.live_meta.borrow().as_ref() {
                if meta.anchor.is_empty() {
                    meta.anchor.clone_from(&old.anchor);
                }
                if meta.area.is_empty() {
                    meta.area.clone_from(&old.area);
                }
                if meta.live_started_at.is_none() {
                    meta.live_started_at = old.live_started_at;
                }
            }
            if self.live_meta.borrow().as_ref() != Some(&meta) {
                *self.live_meta.borrow_mut() = Some(meta);
                changed = true;
            }
        }
        changed
    }
}

/// 拉主播名、分区与同接数；所有请求都 best-effort。
pub(super) fn fetch_live_meta(transport: &dyn np_sources::Transport, room_id: u64) -> LiveMeta {
    let room = np_live::api::fetch_room_info(transport, room_id).ok();
    let (area, live_started_at, uid) = room.as_ref().map_or_else(
        || (String::new(), None, None),
        |room| {
            let area = if room.area_name.trim().is_empty() {
                room.parent_area_name.clone()
            } else {
                room.area_name.clone()
            };
            (area, room.live_started_at, room.uid)
        },
    );
    let anchor = np_live::api::fetch_anchor_name(transport, room_id).unwrap_or_default();
    // `room_info.uid` 优先；旧响应缺 uid 时用 `room_init` 兜底。
    let ruid = uid.or_else(|| {
        np_live::api::fetch_room_init(transport, room_id)
            .ok()
            .and_then(|init| init.uid)
    });
    let online =
        ruid.and_then(|ruid| np_live::api::fetch_online_count(transport, ruid, room_id).ok());
    LiveMeta {
        anchor,
        area,
        live_started_at,
        online,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: &[u8] = include_bytes!("../../../np-live/fixtures/room_info.json");
    const ONLINE: &[u8] = br#"{"code":0,"message":"0","data":{"count":12345}}"#;

    /// 按 URL 路由夹具：room_info / 同接 / 其它（主播名请求失败）。
    struct FixtureTransport;

    impl np_sources::Transport for FixtureTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            if url.contains("queryContributionRank") {
                Ok(ONLINE.to_vec())
            } else if url.contains("get_info") {
                Ok(ROOM.to_vec())
            } else {
                Ok(b"{}".to_vec())
            }
        }
    }

    /// 旧 room_info 响应没有 uid：必须回退 `room_init.uid` 才能查到同接。
    struct NoUidTransport;

    impl np_sources::Transport for NoUidTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            if url.contains("queryContributionRank") {
                Ok(ONLINE.to_vec())
            } else if url.contains("room_init") {
                Ok(br#"{"code":0,"data":{"room_id":21622811,"uid":474595627}}"#.to_vec())
            } else if url.contains("get_info") {
                Ok(
                    r#"{"code":0,"data":{"room_id":21622811,"area_name":"游戏赛事"}}"#
                        .as_bytes()
                        .to_vec(),
                )
            } else {
                Ok(b"{}".to_vec())
            }
        }
    }

    struct BrokenOnlineTransport;

    impl np_sources::Transport for BrokenOnlineTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            if url.contains("queryContributionRank") {
                Ok(br#"{"code":-400,"message":"bad"}"#.to_vec())
            } else if url.contains("get_info") {
                Ok(ROOM.to_vec())
            } else {
                Ok(b"{}".to_vec())
            }
        }
    }

    #[test]
    fn refresh_interval_is_three_seconds() {
        assert_eq!(LIVE_META_REFRESH, Duration::from_secs(3));
    }

    #[test]
    fn an_inflight_request_blocks_a_second_spawn() {
        let audio = TuiAudio::empty();
        let (tx, rx) = std::sync::mpsc::channel();
        *audio.live_meta_rx.borrow_mut() = Some(rx);
        // 已有请求在飞：spawn 必须直接返回，不能替换 receiver。
        audio.spawn_live_meta(0, 21622811);
        tx.send((
            0u64,
            LiveMeta {
                online: Some(7),
                ..LiveMeta::default()
            },
        ))
        .expect("send through the original receiver");
        assert!(audio.drain_live_meta());
        assert_eq!(audio.live_online(), Some(7));
        assert!(audio.live_meta_rx.borrow().is_none(), "回传后释放飞行槽");
    }

    #[test]
    fn live_meta_uses_online_rank_count_not_room_info_online() {
        let meta = fetch_live_meta(&FixtureTransport, 21622811);
        assert_eq!(meta.online, Some(12345), "同接走 online_rank.count");
        assert_eq!(meta.area, "游戏赛事");
        assert_eq!(meta.anchor, "", "anchor 请求失败时只影响主播名");
    }

    #[test]
    fn missing_room_info_uid_falls_back_to_room_init() {
        let meta = fetch_live_meta(&NoUidTransport, 21622811);
        assert_eq!(meta.area, "游戏赛事");
        assert_eq!(meta.online, Some(12345), "room_init.uid 兜底后仍能查同接");
    }

    #[test]
    fn online_failure_becomes_none_for_the_dash_display() {
        let meta = fetch_live_meta(&BrokenOnlineTransport, 21622811);
        assert_eq!(meta.online, None);
        assert_eq!(meta.area, "游戏赛事");
    }
}
