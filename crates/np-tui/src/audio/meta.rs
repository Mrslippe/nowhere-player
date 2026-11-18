//! Live 房间资料（主播 / 分区 / 同接数）的拉取与 30s 刷新。
//!
//! 拆出 `audio.rs` 只是为了让主文件保持在 1800 行卫生上限内；线程与状态仍由
//! [`TuiAudio`] 持有，本模块只做 best-effort 拉取和定时触发。

use std::time::{Duration, Instant};

use super::TuiAudio;

/// 同接数刷新间隔（T-0100b2）。
pub(crate) const LIVE_META_REFRESH: Duration = Duration::from_secs(30);

/// Live 页展示用的房间资料：主播名 + 分区。
///
/// 由独立的 `np-live-meta` 线程 best-effort 拉取；准备线程只负责声音，
/// 资料迟到或失败都不影响直播播放。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveMeta {
    pub(crate) anchor: String,
    pub(crate) area: String,
    pub(crate) live_started_at: Option<u64>,
    /// 同接人数；接口缺失时 `None`，浮层显示 ` -- `。
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

    /// 启动一次后台房间资料拉取；旧 receiver 被替换后旧结果自然丢弃。
    pub(super) fn spawn_live_meta(&self, generation: u64, room_id: u64) {
        let (meta_tx, meta_rx) = std::sync::mpsc::channel();
        *self.live_meta_rx.borrow_mut() = Some(meta_rx);
        let _ = std::thread::Builder::new()
            .name("np-live-meta".to_owned())
            .spawn(move || {
                let Ok(transport) = np_sources::ReqwestTransport::new() else {
                    return;
                };
                let _ = meta_tx.send((generation, fetch_live_meta(&transport, room_id)));
            });
    }

    /// 每 30s 刷新同接数 / 分区；只在真实直播（由 `start_live` 设置过
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
        self.live_meta_refresh_at
            .set(Some(Instant::now() + LIVE_META_REFRESH));
        let generation = self.live_generation.get();
        let room = self.live_room.get();
        if room != 0 {
            self.spawn_live_meta(generation, room);
        }
    }
}

/// 拉主播名、分区与同接数；两条请求都 best-effort。
pub(super) fn fetch_live_meta(transport: &dyn np_sources::Transport, room_id: u64) -> LiveMeta {
    let (area, live_started_at, online) = np_live::api::fetch_room_info(transport, room_id)
        .map(|room| {
            let area = if room.area_name.trim().is_empty() {
                room.parent_area_name
            } else {
                room.area_name
            };
            (area, room.live_started_at, room.online)
        })
        .unwrap_or_default();
    let anchor = np_live::api::fetch_anchor_name(transport, room_id).unwrap_or_default();
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

    struct FixtureTransport;

    impl np_sources::Transport for FixtureTransport {
        fn get(&self, _url: &str) -> np_sources::Result<Vec<u8>> {
            Ok(ROOM.to_vec())
        }
    }

    #[test]
    fn refresh_interval_is_thirty_seconds() {
        assert_eq!(LIVE_META_REFRESH, Duration::from_secs(30));
    }

    #[test]
    fn room_info_carries_online_into_live_meta() {
        let meta = fetch_live_meta(&FixtureTransport, 21622811);
        assert_eq!(meta.online, Some(8400));
        assert_eq!(meta.area, "游戏赛事");
        assert_eq!(meta.anchor, "", "anchor 请求失败时只影响主播名");
    }
}
