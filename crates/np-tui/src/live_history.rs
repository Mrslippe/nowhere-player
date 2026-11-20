//! Browse 直播页的房间历史：`<config>/live_history.json`。
//!
//! 只有这一处读写历史文件。格式是 JSON 数组，字段 `room_id` / `anchor` /
//! `last_used`；np-tui 没有 serde derive 依赖，这里用 `serde_json::Value` 手解析。
//! 写入走 `.tmp` + rename，坏文件只留一句 warning，不阻塞启动。

use std::fs;
use std::path::{Path, PathBuf};

use crate::status::now_millis;

/// 历史条数上限；超出时丢弃最久未用的。
pub(crate) const LIVE_HISTORY_LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveHistoryEntry {
    pub(crate) room_id: u64,
    pub(crate) anchor: String,
    pub(crate) last_used: u64,
}

impl LiveHistoryEntry {
    /// 历史行：主播名到位时 `主播名 · 房间号`，否则 `房间 房间号`。
    pub(crate) fn label(&self) -> String {
        if self.anchor.is_empty() {
            format!("房间 {}", self.room_id)
        } else {
            format!("{} · {}", self.anchor, self.room_id)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LiveHistory {
    pub(crate) path: Option<PathBuf>,
    pub(crate) entries: Vec<LiveHistoryEntry>,
    /// 载入失败时的一句话；`main` 启动后写状态栏。
    pub(crate) warning: Option<String>,
}

impl LiveHistory {
    /// 读历史文件；返回空历史 + warning 的情况都带 `Some(history.path)`。
    pub(crate) fn load(path: Option<PathBuf>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self::default(), None);
        };
        let mut history = Self {
            path: Some(path.clone()),
            entries: Vec::new(),
            warning: None,
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            // 首次运行没有文件是正常态，不提示。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (history, None),
            Err(error) => {
                let warning = format!("直播历史读取失败：{error}");
                history.warning = Some(warning.clone());
                return (history, Some(warning));
            }
        };
        match parse_entries(&text) {
            Ok(entries) => history.entries = entries,
            Err(error) => {
                let warning = format!("直播历史已忽略：{error}");
                history.warning = Some(warning.clone());
                return (history, Some(warning));
            }
        }
        (history, None)
    }

    /// 记一次使用：同房间先删再插到最前，最多保留 [`LIVE_HISTORY_LIMIT`] 条。
    pub(crate) fn record(&mut self, room_id: u64, anchor: &str) {
        // 重开已知房间时先保留旧主播名，meta 失败也不会把历史打回“房间 N”。
        let previous_anchor = self
            .entries
            .iter()
            .find(|entry| entry.room_id == room_id)
            .map(|entry| entry.anchor.clone())
            .unwrap_or_default();
        self.entries.retain(|entry| entry.room_id != room_id);
        // 同一毫秒内连续记录时也要保证“后记的在前”。
        let last_used = now_millis().max(
            self.entries
                .first()
                .map_or(0, |entry| entry.last_used.saturating_add(1)),
        );
        let anchor = anchor.trim();
        self.entries.insert(
            0,
            LiveHistoryEntry {
                room_id,
                anchor: if anchor.is_empty() {
                    previous_anchor
                } else {
                    anchor.to_owned()
                },
                last_used,
            },
        );
        self.entries.truncate(LIVE_HISTORY_LIMIT);
    }

    /// 主播名回填：只改名字，不动使用顺序；返回是否真的变了。
    pub(crate) fn update_anchor(&mut self, room_id: u64, anchor: &str) -> bool {
        let anchor = anchor.trim();
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.room_id == room_id)
        else {
            return false;
        };
        if entry.anchor == anchor {
            return false;
        }
        entry.anchor = anchor.to_owned();
        true
    }

    pub(crate) fn remove(&mut self, room_id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.room_id != room_id);
        self.entries.len() != before
    }

    /// 原子写：先写 `live_history.json.tmp`，再 rename；没有路径时直接成功。
    pub(crate) fn save(&self) -> Result<(), String> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("创建直播历史目录失败：{error}"))?;
        }
        let items: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "room_id": entry.room_id,
                    "anchor": entry.anchor,
                    "last_used": entry.last_used,
                })
            })
            .collect();
        let json = serde_json::to_string_pretty(&serde_json::Value::Array(items))
            .map_err(|error| format!("直播历史序列化失败：{error}"))?;
        let temporary = temporary_path(path);
        if let Err(error) = fs::write(&temporary, json) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("写入直播历史失败：{error}"));
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("替换直播历史失败：{error}"));
        }
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut file = path.as_os_str().to_owned();
    file.push(".tmp");
    PathBuf::from(file)
}

/// 手解析：坏行跳过、按房间号去重、`last_used` 降序、截断到上限。
fn parse_entries(text: &str) -> Result<Vec<LiveHistoryEntry>, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("JSON 解析失败：{error}"))?;
    let Some(items) = value.as_array() else {
        return Err("顶层不是数组".to_owned());
    };
    let mut entries: Vec<LiveHistoryEntry> = Vec::new();
    for item in items {
        let Some(room_id) = item.get("room_id").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        if room_id == 0 || entries.iter().any(|entry| entry.room_id == room_id) {
            continue;
        }
        let anchor = item
            .get("anchor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let last_used = item
            .get("last_used")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        entries.push(LiveHistoryEntry {
            room_id,
            anchor,
            last_used,
        });
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_used));
    entries.truncate(LIVE_HISTORY_LIMIT);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "np-tui-live-history-{}-{}-{sequence}",
                std::process::id(),
                now_millis()
            ))
            .join(format!("{name}/live_history.json"))
    }

    fn cleanup(path: &Path) {
        if let Some(root) = path.parent().and_then(Path::parent) {
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn roundtrip_keeps_order_anchor_and_path() {
        let path = temp_path("roundtrip");
        let (mut history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        assert!(history.entries.is_empty());
        history.record(21622811, "主播A");
        history.record(1234, "");
        history.save().expect("save");
        assert!(!temporary_path(&path).exists(), "临时文件必须被 rename 掉");

        let (reloaded, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(reloaded.entries.len(), 2);
        assert_eq!(reloaded.entries[0].room_id, 1234);
        assert_eq!(reloaded.entries[0].anchor, "");
        assert_eq!(reloaded.entries[1].room_id, 21622811);
        assert_eq!(reloaded.entries[1].anchor, "主播A");
        assert!(reloaded.entries[0].last_used >= reloaded.entries[1].last_used);
        assert_eq!(reloaded.path.as_deref(), Some(path.as_path()));
        cleanup(&path);
    }

    #[test]
    fn missing_file_loads_empty_without_warning() {
        let path = temp_path("missing");
        let (history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        assert!(history.entries.is_empty());
        cleanup(&path);
    }

    #[test]
    fn corrupt_file_returns_empty_history_and_warning() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "not json").expect("write");
        let (history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(history.entries.is_empty());
        assert!(warning.is_some(), "坏文件必须提示");
        // 坏文件不阻塞后续写入：保存后能正常读回。
        let mut history = history;
        history.record(1, "");
        history.save().expect("save over corrupt file");
        let (reloaded, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        assert_eq!(reloaded.entries.len(), 1);
        cleanup(&path);
    }

    #[test]
    fn non_array_and_bad_rows_are_tolerated() {
        let path = temp_path("rows");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, r#"{"room_id":1}"#).expect("write");
        let (history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(history.entries.is_empty());
        assert!(warning.is_some());

        fs::write(
            &path,
            r#"[{"room_id":7,"anchor":"A"},{"nope":1},{"room_id":0},{"room_id":"9"},{"room_id":8,"anchor":"B","last_used":5}]"#,
        )
        .expect("write");
        let (history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        let rooms: Vec<u64> = history.entries.iter().map(|entry| entry.room_id).collect();
        assert_eq!(rooms, vec![8, 7]);
        cleanup(&path);
    }

    #[test]
    fn load_sorts_by_last_used_descending() {
        let path = temp_path("sorted");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(
            &path,
            r#"[{"room_id":1,"last_used":10},{"room_id":3,"last_used":30},{"room_id":2,"last_used":20}]"#,
        )
        .expect("write");
        let (history, warning) = LiveHistory::load(Some(path.clone()));
        assert!(warning.is_none());
        let rooms: Vec<u64> = history.entries.iter().map(|entry| entry.room_id).collect();
        assert_eq!(rooms, vec![3, 2, 1]);
        cleanup(&path);
    }

    #[test]
    fn record_dedupes_by_room_and_keeps_latest_anchor_in_front() {
        let mut history = LiveHistory::default();
        history.record(100, "");
        history.record(200, "B");
        history.record(100, "A");
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].room_id, 100);
        assert_eq!(history.entries[0].anchor, "A");
        assert_eq!(history.entries[1].room_id, 200);
    }

    #[test]
    fn re_record_without_anchor_keeps_the_previous_name() {
        let mut history = LiveHistory::default();
        history.record(7, "主播X");
        history.record(8, "");
        history.record(7, "");
        assert_eq!(history.entries[0].room_id, 7);
        assert_eq!(history.entries[0].anchor, "主播X");
    }

    #[test]
    fn record_caps_at_twenty_and_drops_the_oldest() {
        let mut history = LiveHistory::default();
        for room in 1..=25u64 {
            history.record(room, "");
        }
        assert_eq!(history.entries.len(), LIVE_HISTORY_LIMIT);
        assert_eq!(history.entries[0].room_id, 25);
        assert_eq!(history.entries[LIVE_HISTORY_LIMIT - 1].room_id, 6);
        assert!(history.entries.iter().all(|entry| entry.room_id > 5));
    }

    #[test]
    fn update_anchor_only_touches_the_matching_room() {
        let path = temp_path("anchor");
        let (mut history, _) = LiveHistory::load(Some(path.clone()));
        history.record(7, "");
        assert!(history.update_anchor(7, "主播X"));
        assert!(!history.update_anchor(7, "主播X"), "同名不重复写");
        assert!(!history.update_anchor(8, "别人"));
        assert_eq!(history.entries[0].anchor, "主播X");
        history.save().expect("save");
        let (reloaded, _) = LiveHistory::load(Some(path.clone()));
        assert_eq!(reloaded.entries[0].anchor, "主播X");
        cleanup(&path);
    }

    #[test]
    fn remove_deletes_only_the_requested_room() {
        let mut history = LiveHistory::default();
        history.record(1, "");
        history.record(2, "");
        assert!(history.remove(1));
        assert!(!history.remove(1));
        let rooms: Vec<u64> = history.entries.iter().map(|entry| entry.room_id).collect();
        assert_eq!(rooms, vec![2]);
    }

    #[test]
    fn label_falls_back_to_room_when_anchor_is_missing() {
        let mut history = LiveHistory::default();
        history.record(21622811, "");
        history.record(1234, "主播B");
        assert_eq!(history.entries[0].label(), "主播B · 1234");
        assert_eq!(history.entries[1].label(), "房间 21622811");
    }
}
