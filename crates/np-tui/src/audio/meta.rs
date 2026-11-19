//! Live 房间资料与同接人数的两路独立刷新。
//!
//! 拆出 `audio.rs` 只是为了让主文件保持在 1800 行卫生上限内；线程与状态仍由
//! [`TuiAudio`] 持有，本模块只做 best-effort 拉取和定时触发。
//!
//! 房间资料（主播名 / 分区 / 开播时间 / 主播 uid）**30s** 刷新一次；同接人数
//! （`queryContributionRank?type=online_rank` 的 `data.count`）**3s** 刷新一次。
//! 两路各有独立的 channel、计时与单飞行，一个失败或迟回不影响另一个。
//! 同接优先复用资料里的 `room_info.uid`，缺失时才按需回退 `room_init.uid`。

use std::cell::Cell;
use std::time::{Duration, Instant};

use super::TuiAudio;

/// 基础资料（主播 / 分区 / 开播时间 / 主播 uid）刷新间隔：30 秒。
pub(crate) const LIVE_META_REFRESH: Duration = Duration::from_secs(30);

/// 同接人数刷新间隔：3 秒。
pub(crate) const LIVE_ONLINE_REFRESH: Duration = Duration::from_secs(3);

/// Live 页展示用的房间资料：主播名 + 分区 + 开播时间 + 主播 uid。
///
/// 由独立的 `np-live-meta` 线程 best-effort 拉取；准备线程只负责声音，
/// 资料迟到或失败都不影响直播播放，失败时保留上一次的主播 / 分区。
/// 同接人数走另一条 3s 通道（[`TuiAudio::live_online`]），不在这里。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveMeta {
    pub(crate) anchor: String,
    pub(crate) area: String,
    pub(crate) live_started_at: Option<u64>,
    /// 主播 uid；同接查询优先用它，缺失时同接路按需回退 `room_init`。
    pub(crate) ruid: Option<u64>,
}

impl TuiAudio {
    /// 浮层右上角的同接人数；资料未到或接口失败时 `None`（显示 ` -- `）。
    pub(crate) fn live_online(&self) -> Option<u64> {
        self.live_online.get()
    }

    /// 一轮直播开始：两路立即各拉一次，并把计时分别顺延到各自周期。
    ///
    /// `start_live`、重连、F5 都会走到这里；旧 receiver 先丢掉，再各自
    /// 递增 generation，保证迟到的旧结果不会串到新房。
    pub(super) fn begin_live_refresh(&self, room_id: u64) {
        *self.live_meta_rx.borrow_mut() = None;
        *self.live_online_rx.borrow_mut() = None;
        self.live_online.set(None);
        let meta_generation = self.live_meta_generation.get().wrapping_add(1);
        self.live_meta_generation.set(meta_generation);
        let online_generation = self.live_online_generation.get().wrapping_add(1);
        self.live_online_generation.set(online_generation);
        let now = Instant::now();
        self.live_meta_refresh_at.set(Some(now + LIVE_META_REFRESH));
        self.live_online_refresh_at
            .set(Some(now + LIVE_ONLINE_REFRESH));
        self.spawn_live_meta(meta_generation, room_id);
        self.spawn_live_online(online_generation, room_id);
    }

    /// stop / 换房：两路 generation 各递增一次，清掉在飞槽位、计时与同接数。
    pub(super) fn reset_live_refresh(&self) {
        self.live_meta_generation
            .set(self.live_meta_generation.get().wrapping_add(1));
        self.live_online_generation
            .set(self.live_online_generation.get().wrapping_add(1));
        *self.live_meta_rx.borrow_mut() = None;
        *self.live_online_rx.borrow_mut() = None;
        self.live_meta_refresh_at.set(None);
        self.live_online_refresh_at.set(None);
        self.live_online.set(None);
    }

    /// 到点判定：未直播时清掉计时；未到点或上一次请求还在飞都返回 `false`。
    ///
    /// 两路各传自己的 `Cell` 和在飞标志，因此互不阻塞、互不顺延。
    fn refresh_due(&self, due_at: &Cell<Option<Instant>>, in_flight: bool) -> bool {
        if !self.is_live() {
            due_at.set(None);
            return false;
        }
        due_at
            .get()
            .is_some_and(|due| !in_flight && Instant::now() >= due)
    }

    /// 启动一次后台资料拉取；已有请求在飞时直接返回（单飞行、防重入）。
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

    /// 启动一次后台同接拉取；`ruid` 取资料里已拿到的 uid，缺失时线程内才回退。
    pub(super) fn spawn_live_online(&self, generation: u64, room_id: u64) {
        if self.live_online_rx.borrow().is_some() {
            return;
        }
        let ruid = self.live_meta.borrow().as_ref().and_then(|meta| meta.ruid);
        let (online_tx, online_rx) = std::sync::mpsc::channel();
        *self.live_online_rx.borrow_mut() = Some(online_rx);
        let spawn = std::thread::Builder::new()
            .name("np-live-online".to_owned())
            .spawn(move || {
                let Ok(transport) = np_sources::ReqwestTransport::new() else {
                    return;
                };
                let online = fetch_live_online(&transport, room_id, ruid);
                let _ = online_tx.send((generation, online));
            });
        if spawn.is_err() {
            *self.live_online_rx.borrow_mut() = None;
        }
    }

    /// 每 30s 刷新主播 / 分区 / 开播时间；只在真实直播（由 `start_live`
    /// 设置过刷新时刻）时生效，测试里的 `live_for_test` 不会触网。
    pub(super) fn refresh_live_meta_if_due(&self) {
        if !self.refresh_due(
            &self.live_meta_refresh_at,
            self.live_meta_rx.borrow().is_some(),
        ) {
            return;
        }
        self.live_meta_refresh_at
            .set(Some(Instant::now() + LIVE_META_REFRESH));
        let room = self.live_room.get();
        if room != 0 {
            self.spawn_live_meta(self.live_meta_generation.get(), room);
        }
    }

    /// 每 3s 刷新同接人数；优先用资料里的 `ruid`，缺失时由线程回退 `room_init`。
    pub(super) fn refresh_live_online_if_due(&self) {
        if !self.refresh_due(
            &self.live_online_refresh_at,
            self.live_online_rx.borrow().is_some(),
        ) {
            return;
        }
        self.live_online_refresh_at
            .set(Some(Instant::now() + LIVE_ONLINE_REFRESH));
        let room = self.live_room.get();
        if room != 0 {
            self.spawn_live_online(self.live_online_generation.get(), room);
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
            if generation != self.live_meta_generation.get() {
                continue;
            }
            // 主播名 / 分区 / 开播时间 / uid 都是慢变信息：单次请求失败时
            // 保留上一次的值。同接数不在这里，失败由 3s 通道单独处理。
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
                if meta.ruid.is_none() {
                    meta.ruid = old.ruid;
                }
            }
            if self.live_meta.borrow().as_ref() != Some(&meta) {
                *self.live_meta.borrow_mut() = Some(meta);
                changed = true;
            }
        }
        changed
    }

    /// 收下同接结果；generation 过期就丢掉。失败按 `None` 覆盖，显示 ` -- `，
    /// 不保留上一次的数字。
    pub(super) fn drain_live_online(&self) -> bool {
        let mut changed = false;
        loop {
            let (received, done) = {
                let receiver = self.live_online_rx.borrow();
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
                *self.live_online_rx.borrow_mut() = None;
            }
            let Some((generation, online)) = received else {
                break;
            };
            if generation != self.live_online_generation.get() {
                continue;
            }
            if self.live_online.get() != online {
                self.live_online.set(online);
                changed = true;
            }
        }
        changed
    }
}

/// 拉主播名、分区、开播时间与主播 uid；不查同接，也不碰 `room_init`。
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
    LiveMeta {
        anchor,
        area,
        live_started_at,
        ruid: uid,
    }
}

/// 拉同接人数：`ruid` 优先用资料里的 uid，缺失时才回退 `room_init.uid`。
///
/// 回退是「按需」的：meta 每次成功都不额外查 `room_init`，只有同接线程
/// 拿不到 uid 的那一轮才补一次（旧 room_info 响应可能没有 `uid`）。
pub(super) fn fetch_live_online(
    transport: &dyn np_sources::Transport,
    room_id: u64,
    ruid: Option<u64>,
) -> Option<u64> {
    let ruid = ruid.or_else(|| {
        np_live::api::fetch_room_init(transport, room_id)
            .ok()
            .and_then(|init| init.uid)
    });
    ruid.and_then(|ruid| np_live::api::fetch_online_count(transport, ruid, room_id).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;

    const ROOM: &[u8] = include_bytes!("../../../np-live/fixtures/room_info.json");
    const NO_UID_ROOM: &[u8] =
        br#"{"code":0,"data":{"room_id":21622811,"area_name":"\u6e38\u620f\u8d5b\u4e8b"}}"#;
    const ROOM_INIT: &[u8] = br#"{"code":0,"data":{"room_id":21622811,"uid":474595627}}"#;
    const ONLINE: &[u8] = br#"{"code":0,"message":"0","data":{"count":12345}}"#;

    /// 按 URL 路由夹具；`room_info` 可换，`room_init` 调用次数可断言。
    struct FixtureTransport {
        room_info: Vec<u8>,
        room_init_calls: AtomicU32,
    }

    impl FixtureTransport {
        fn new(room_info: &[u8]) -> Self {
            Self {
                room_info: room_info.to_vec(),
                room_init_calls: AtomicU32::new(0),
            }
        }

        fn room_init_calls(&self) -> u32 {
            self.room_init_calls.load(Ordering::SeqCst)
        }
    }

    impl np_sources::Transport for FixtureTransport {
        fn get(&self, url: &str) -> np_sources::Result<Vec<u8>> {
            if url.contains("queryContributionRank") {
                Ok(ONLINE.to_vec())
            } else if url.contains("room_init") {
                self.room_init_calls.fetch_add(1, Ordering::SeqCst);
                Ok(ROOM_INIT.to_vec())
            } else if url.contains("get_info") {
                Ok(self.room_info.clone())
            } else {
                // 主播名等其它请求失败：让 anchor 保持为空。
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

    fn live_audio() -> TuiAudio {
        let audio = TuiAudio::empty();
        audio.live_for_test(21622811, "BLAST");
        audio
    }

    #[test]
    fn refresh_intervals_split_thirty_and_three_seconds() {
        assert_eq!(LIVE_META_REFRESH, Duration::from_secs(30));
        assert_eq!(LIVE_ONLINE_REFRESH, Duration::from_secs(3));
    }

    #[test]
    fn meta_and_online_deadlines_are_independent() {
        let audio = live_audio();
        let now = Instant::now();
        let past = now.checked_sub(Duration::from_millis(1)).expect("past");
        let future = now + Duration::from_secs(2);

        audio.live_meta_refresh_at.set(Some(past));
        audio.live_online_refresh_at.set(Some(future));
        assert!(
            audio.refresh_due(&audio.live_meta_refresh_at, false),
            "meta due"
        );
        assert!(
            !audio.refresh_due(&audio.live_online_refresh_at, false),
            "online not due"
        );

        audio.live_meta_refresh_at.set(Some(future));
        audio.live_online_refresh_at.set(Some(past));
        assert!(
            !audio.refresh_due(&audio.live_meta_refresh_at, false),
            "meta not due"
        );
        assert!(
            audio.refresh_due(&audio.live_online_refresh_at, false),
            "online due"
        );
    }

    #[test]
    fn in_flight_blocks_only_its_own_channel() {
        let audio = live_audio();
        let past = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("past");
        audio.live_meta_refresh_at.set(Some(past));
        audio.live_online_refresh_at.set(Some(past));

        let (meta_tx, meta_rx) = mpsc::channel();
        let (_online_tx, online_rx) = mpsc::channel();
        *audio.live_meta_rx.borrow_mut() = Some(meta_rx);
        *audio.live_online_rx.borrow_mut() = Some(online_rx);
        assert!(
            !audio.refresh_due(
                &audio.live_meta_refresh_at,
                audio.live_meta_rx.borrow().is_some()
            ),
            "在飞的 meta 必须挡住本轮"
        );
        assert!(
            !audio.refresh_due(
                &audio.live_online_refresh_at,
                audio.live_online_rx.borrow().is_some()
            ),
            "在飞的 online 必须挡住本轮"
        );

        // 只结束 meta 的飞行：meta 到点，online 仍被自己的飞行挡着。
        meta_tx.send((0, LiveMeta::default())).expect("send meta");
        assert!(audio.drain_live_meta());
        assert!(
            audio.refresh_due(
                &audio.live_meta_refresh_at,
                audio.live_meta_rx.borrow().is_some()
            ),
            "meta 回传后重新到点"
        );
        assert!(
            !audio.refresh_due(
                &audio.live_online_refresh_at,
                audio.live_online_rx.borrow().is_some()
            ),
            "online 的飞行槽不受 meta 影响"
        );
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
                anchor: "测试主播".to_owned(),
                ..LiveMeta::default()
            },
        ))
        .expect("send through the original receiver");
        assert!(audio.drain_live_meta());
        assert_eq!(audio.live_anchor(), "测试主播");
        assert!(audio.live_meta_rx.borrow().is_none(), "回传后释放飞行槽");
    }

    #[test]
    fn stale_generations_of_both_channels_are_dropped() {
        let audio = live_audio();
        audio.live_meta_for_test("旧主播", "旧分区");
        audio.live_online.set(Some(2000));

        let meta_stale = audio.live_meta_generation.get().wrapping_sub(1);
        let (meta_tx, meta_rx) = mpsc::channel();
        *audio.live_meta_rx.borrow_mut() = Some(meta_rx);
        meta_tx
            .send((
                meta_stale,
                LiveMeta {
                    anchor: "新主播".to_owned(),
                    area: "新分区".to_owned(),
                    ..LiveMeta::default()
                },
            ))
            .expect("send stale meta");
        assert!(!audio.drain_live_meta(), "旧 generation 不产生变更");
        assert_eq!(audio.live_anchor(), "旧主播");
        assert_eq!(audio.live_area(), "旧分区");

        let online_stale = audio.live_online_generation.get().wrapping_sub(1);
        let (online_tx, online_rx) = mpsc::channel();
        *audio.live_online_rx.borrow_mut() = Some(online_rx);
        online_tx
            .send((online_stale, Some(9999)))
            .expect("send stale online");
        assert!(!audio.drain_live_online());
        assert_eq!(audio.live_online(), Some(2000));
        assert!(
            audio.live_meta_rx.borrow().is_none(),
            "单次回传释放 meta 槽"
        );
        assert!(
            audio.live_online_rx.borrow().is_none(),
            "单次回传释放 online 槽"
        );
    }

    #[test]
    fn meta_failure_keeps_the_previous_anchor_and_area() {
        let audio = live_audio();
        audio.live_meta_for_test("旧主播", "旧分区");

        let generation = audio.live_meta_generation.get();
        let (partial_tx, partial_rx) = mpsc::channel();
        *audio.live_meta_rx.borrow_mut() = Some(partial_rx);
        partial_tx
            .send((
                generation,
                LiveMeta {
                    anchor: "新主播".to_owned(),
                    ..LiveMeta::default()
                },
            ))
            .expect("send partial meta");
        assert!(audio.drain_live_meta());
        assert_eq!(audio.live_anchor(), "新主播");
        assert_eq!(audio.live_area(), "旧分区", "空分区沿用旧值");

        let (empty_tx, empty_rx) = mpsc::channel();
        *audio.live_meta_rx.borrow_mut() = Some(empty_rx);
        empty_tx
            .send((generation, LiveMeta::default()))
            .expect("send empty meta");
        assert!(!audio.drain_live_meta(), "全空结果不产生变更");
        assert_eq!(audio.live_anchor(), "新主播", "失败不覆盖主播名");
        assert_eq!(audio.live_area(), "旧分区", "失败不覆盖分区");
    }

    #[test]
    fn online_failure_clears_the_old_count_for_the_dash() {
        let audio = live_audio();
        audio.live_online.set(Some(12345));
        let online_generation = audio.live_online_generation.get();
        let (tx, rx) = mpsc::channel();
        *audio.live_online_rx.borrow_mut() = Some(rx);
        tx.send((online_generation, None)).expect("send failure");
        assert!(audio.drain_live_online());
        assert_eq!(audio.live_online(), None, "失败显示 --，不保留旧数");
    }

    #[test]
    fn stop_live_resets_both_channels_and_bumps_generations() {
        let audio = live_audio();
        audio.live_online.set(Some(2000));
        let meta_before = audio.live_meta_generation.get();
        let online_before = audio.live_online_generation.get();

        assert!(audio.stop_live());
        assert_eq!(audio.live_meta_generation.get(), meta_before + 1);
        assert_eq!(audio.live_online_generation.get(), online_before + 1);
        assert_eq!(audio.live_online(), None);
        assert!(audio.live_meta_refresh_at.get().is_none());
        assert!(audio.live_online_refresh_at.get().is_none());
    }

    #[test]
    fn live_meta_fetches_anchor_area_uid_without_online_or_room_init() {
        let transport = FixtureTransport::new(ROOM);
        let meta = fetch_live_meta(&transport, 21622811);
        assert_eq!(meta.area, "游戏赛事");
        assert_eq!(meta.ruid, Some(474_595_627));
        assert_eq!(meta.anchor, "", "anchor 请求失败时只影响主播名");
        assert_eq!(transport.room_init_calls(), 0, "meta 不查 room_init");
    }

    #[test]
    fn online_uses_meta_ruid_without_room_init() {
        let transport = FixtureTransport::new(ROOM);
        assert_eq!(
            fetch_live_online(&transport, 21622811, Some(474_595_627)),
            Some(12_345)
        );
        assert_eq!(transport.room_init_calls(), 0, "有 uid 时不查 room_init");
    }

    #[test]
    fn missing_ruid_falls_back_to_room_init_on_demand() {
        let transport = FixtureTransport::new(NO_UID_ROOM);
        let meta = fetch_live_meta(&transport, 21622811);
        assert_eq!(meta.ruid, None, "旧 room_info 没有 uid");
        assert_eq!(transport.room_init_calls(), 0, "meta 阶段仍不查 room_init");

        assert_eq!(
            fetch_live_online(&transport, 21622811, meta.ruid),
            Some(12_345),
            "room_init.uid 兜底后仍能查同接"
        );
        assert_eq!(transport.room_init_calls(), 1);
    }

    #[test]
    fn online_failure_becomes_none_for_the_dash_display() {
        assert_eq!(
            fetch_live_online(&BrokenOnlineTransport, 21622811, Some(474_595_627)),
            None
        );
    }
}
