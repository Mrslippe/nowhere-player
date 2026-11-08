//! 内存播放队列（T-0015）：纯数据结构 + 纯函数，不持有 `np_audio::Player`。
//!
//! 队列只存曲目 id（本地文件即绝对路径），不持久化（M3 再做）。
//! 播放顺序直接保存在 `entries` 里：`shuffle` 打开时把「即将播放的部分」
//! 打乱一次，之后一律按序播——而不是每次随机取一首（避免同一首连续重复）。
//!
//! 面板、actor、IPC 都只通过这里的语义操作队列；切歌时真正的
//! `Player::open` / drop 由 `apps/desktop` 的 actor 在本线程完成。

use serde::{Deserialize, Serialize};

use crate::player::{PlaybackMode, RepeatMode};

/// 队列快照（`queue.*` IPC 的返回载荷）。
///
/// `tracks` 已按**播放顺序**排列；`index` 是当前曲目在该顺序里的下标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueView {
    /// 按播放顺序排列的曲目 id。
    pub tracks: Vec<String>,
    /// 当前指针；空队列为 `None`。
    pub index: Option<usize>,
}

/// 一次切歌的决策结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStep {
    /// 打开并播放该下标（可能与当前曲目相同，例如单曲循环）。
    Open(usize),
    /// 没有下一首：保持当前曲目（`repeat = off` 且已在最后一首）。
    Exhausted,
}

/// 队列里的一条曲目。
#[derive(Debug, Clone)]
struct Entry {
    /// 曲目 id。
    id: String,
    /// 入队序号；关闭 shuffle 时用它还原插入顺序。
    seq: u64,
}

/// 内存播放队列。
///
/// 不变式：`cursor.is_none()` ⇔ `entries.is_empty()`；否则
/// `cursor` 一定落在 `entries` 的下标范围内。
#[derive(Debug, Clone)]
pub struct Queue {
    entries: Vec<Entry>,
    cursor: Option<usize>,
    mode: PlaybackMode,
    next_seq: u64,
    rng: u64,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    /// 空队列，模式为 `repeat = off / shuffle = off`。
    pub fn new() -> Self {
        Self::with_seed(0x9E37_79B9_7F4A_7C15)
    }

    /// 指定随机种子（测试用，保证 shuffle 可复现）。
    pub fn with_seed(seed: u64) -> Self {
        Self {
            entries: Vec::new(),
            cursor: None,
            mode: PlaybackMode::default(),
            next_seq: 0,
            // xorshift 的状态不能是 0。
            rng: if seed == 0 { 1 } else { seed },
        }
    }

    /// 曲目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 当前播放模式。
    pub fn mode(&self) -> PlaybackMode {
        self.mode
    }

    /// 设置播放模式；`shuffle` 由关到开时打乱「当前之后」的顺序，
    /// 由开到关时按入队顺序还原（当前曲目保持不动）。
    pub fn set_mode(&mut self, mode: PlaybackMode) {
        if mode.shuffle != self.mode.shuffle {
            if mode.shuffle {
                self.mode = mode;
                self.shuffle_upcoming();
            } else {
                self.mode = mode;
                self.restore_insertion_order();
            }
        } else {
            self.mode = mode;
        }
    }

    /// 按播放顺序排列的曲目 id。
    pub fn track_ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.id.as_str())
    }

    /// 队列快照（`queue.list` 等 IPC 的返回载荷）。
    pub fn view(&self) -> QueueView {
        QueueView {
            tracks: self.entries.iter().map(|entry| entry.id.clone()).collect(),
            index: self.cursor,
        }
    }

    /// 指定下标的曲目 id。
    pub fn track_at(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|entry| entry.id.as_str())
    }

    /// 当前曲目 id；空队列为 `None`。
    pub fn current(&self) -> Option<&str> {
        self.cursor
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.id.as_str())
    }

    /// 当前曲目在播放顺序里的下标。
    pub fn position(&self) -> Option<usize> {
        self.cursor
    }

    /// 追加一首；空队列时新曲目同时成为当前项。返回它的下标。
    pub fn push(&mut self, id: impl Into<String>) -> usize {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push(Entry { id: id.into(), seq });
        let index = self.entries.len() - 1;
        if self.cursor.is_none() {
            self.cursor = Some(index);
        }
        index
    }

    /// 查找曲目下标（首个匹配）。
    pub fn find(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }

    /// 把当前指针移到 `index`；越界返回 `None`。
    pub fn jump(&mut self, index: usize) -> Option<usize> {
        if index >= self.entries.len() {
            return None;
        }
        self.cursor = Some(index);
        Some(index)
    }

    /// 删除指定下标；返回被删曲目 id。
    ///
    /// 指针调整规则：
    /// - 删除当前项之前的曲目 → 当前曲目不变，指针前移一位；
    /// - 删除当前项 → 指针停在原位（即原来的下一首），删掉的是末尾则退到新的末尾；
    /// - 删除当前项之后的曲目 → 指针不变。
    pub fn remove(&mut self, index: usize) -> Option<String> {
        if index >= self.entries.len() {
            return None;
        }
        let removed = self.entries.remove(index);
        match self.cursor {
            None => {}
            Some(cursor) if index < cursor => self.cursor = Some(cursor - 1),
            Some(cursor) if index == cursor => {
                let len = self.entries.len();
                self.cursor = if len == 0 {
                    None
                } else {
                    Some(cursor.min(len - 1))
                };
            }
            Some(_) => {}
        }
        Some(removed.id)
    }

    /// 清空队列并把指针复位。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = None;
    }

    /// 下一首的决策。
    ///
    /// - `repeat = one`：重播当前曲目（返回当前下标）；
    /// - `repeat = off`：前进一首，已在末尾则 [`QueueStep::Exhausted`]；
    /// - `repeat = all`：前进一首，末尾回绕到第一首。
    pub fn next(&mut self) -> QueueStep {
        let Some(cursor) = self.cursor else {
            return QueueStep::Exhausted;
        };
        match self.mode.repeat {
            RepeatMode::One => QueueStep::Open(cursor),
            RepeatMode::Off => {
                if cursor + 1 < self.entries.len() {
                    self.cursor = Some(cursor + 1);
                    QueueStep::Open(cursor + 1)
                } else {
                    QueueStep::Exhausted
                }
            }
            RepeatMode::All => {
                let next = (cursor + 1) % self.entries.len();
                self.cursor = Some(next);
                QueueStep::Open(next)
            }
        }
    }

    /// 上一首的决策。
    ///
    /// - `repeat = one`：重播当前曲目；
    /// - `repeat = off`：在首曲时重播首曲（不越界、不回绕）；
    /// - `repeat = all`：首曲时回绕到最后一首。
    pub fn prev(&mut self) -> QueueStep {
        match self.cursor {
            None => QueueStep::Exhausted,
            Some(cursor) => match self.mode.repeat {
                RepeatMode::One => QueueStep::Open(cursor),
                RepeatMode::Off => {
                    let prev = cursor.saturating_sub(1);
                    self.cursor = Some(prev);
                    QueueStep::Open(prev)
                }
                RepeatMode::All => {
                    let prev = (cursor + self.entries.len() - 1) % self.entries.len();
                    self.cursor = Some(prev);
                    QueueStep::Open(prev)
                }
            },
        }
    }

    /// 打开失败时跳到下一首候选（环形，与 `repeat` 无关）。
    ///
    /// 失败跳过不能被 `repeat = off` 卡死在末尾；由调用方限制尝试次数。
    pub fn next_for_skip(&mut self) -> Option<usize> {
        let cursor = self.cursor?;
        if self.entries.is_empty() {
            return None;
        }
        let next = (cursor + 1) % self.entries.len();
        self.cursor = Some(next);
        Some(next)
    }

    /// 打乱「当前项之后」的顺序（当前曲目保持不动，已播过的部分不动）。
    fn shuffle_upcoming(&mut self) {
        let start = self.cursor.map_or(0, |cursor| cursor + 1);
        for i in (start + 1..self.entries.len()).rev() {
            let span = (i - start + 1) as u64;
            let j = start + (self.rand() % span) as usize;
            self.entries.swap(i, j);
        }
    }

    /// 按入队顺序还原，指针跟随当前曲目。
    fn restore_insertion_order(&mut self) {
        let anchor = self
            .cursor
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.seq);
        self.entries.sort_by_key(|entry| entry.seq);
        self.cursor = anchor.and_then(|seq| self.entries.iter().position(|entry| entry.seq == seq));
    }

    /// xorshift64*：零依赖的确定性随机源。
    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_with(ids: &[&str]) -> Queue {
        let mut queue = Queue::new();
        for id in ids {
            queue.push(*id);
        }
        queue
    }

    #[test]
    fn empty_queue_has_no_current_track_and_steps_are_safe() {
        let mut queue = Queue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.position(), None);
        assert_eq!(queue.current(), None);
        assert_eq!(queue.next(), QueueStep::Exhausted);
        assert_eq!(queue.prev(), QueueStep::Exhausted);
        assert_eq!(queue.next_for_skip(), None);
        assert_eq!(queue.remove(0), None);
        assert_eq!(queue.jump(0), None);
        assert_eq!(
            queue.view(),
            QueueView {
                tracks: vec![],
                index: None
            }
        );
    }

    #[test]
    fn push_points_at_the_first_track_and_view_keeps_play_order() {
        let mut queue = queue_with(&["a", "b", "c"]);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.position(), Some(0));
        assert_eq!(queue.current(), Some("a"));
        assert_eq!(queue.view().tracks, vec!["a", "b", "c"]);
        assert_eq!(queue.view().index, Some(0));
        assert_eq!(queue.track_at(2), Some("c"));
        assert_eq!(queue.track_at(3), None);
    }

    #[test]
    fn next_advances_and_stops_at_the_last_track_with_repeat_off() {
        let mut queue = queue_with(&["a", "b"]);
        assert_eq!(queue.next(), QueueStep::Open(1));
        assert_eq!(queue.current(), Some("b"));
        assert_eq!(queue.next(), QueueStep::Exhausted);
        assert_eq!(queue.position(), Some(1), "exhausted keeps the last track");
    }

    #[test]
    fn next_wraps_with_repeat_all() {
        let mut queue = queue_with(&["a", "b"]);
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: false,
        });
        assert_eq!(queue.next(), QueueStep::Open(1));
        assert_eq!(queue.next(), QueueStep::Open(0));
        assert_eq!(queue.current(), Some("a"));
    }

    #[test]
    fn single_track_repeat_all_reopens_the_same_track() {
        let mut queue = queue_with(&["only"]);
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: false,
        });
        assert_eq!(queue.next(), QueueStep::Open(0));
        assert_eq!(queue.position(), Some(0));
    }

    #[test]
    fn repeat_one_replays_the_current_track() {
        let mut queue = queue_with(&["a", "b"]);
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::One,
            shuffle: false,
        });
        assert_eq!(queue.next(), QueueStep::Open(0));
        assert_eq!(queue.position(), Some(0));
        assert_eq!(queue.prev(), QueueStep::Open(0));
        assert_eq!(queue.position(), Some(0), "repeat=one must not move");
    }

    #[test]
    fn prev_clamps_at_the_first_track_with_repeat_off() {
        let mut queue = queue_with(&["a", "b"]);
        assert_eq!(queue.prev(), QueueStep::Open(0));
        assert_eq!(queue.position(), Some(0));
        queue.next();
        assert_eq!(queue.prev(), QueueStep::Open(0));
    }

    #[test]
    fn prev_wraps_to_the_last_track_with_repeat_all() {
        let mut queue = queue_with(&["a", "b", "c"]);
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: false,
        });
        assert_eq!(queue.prev(), QueueStep::Open(2));
        assert_eq!(queue.current(), Some("c"));
        assert_eq!(queue.prev(), QueueStep::Open(1));
    }

    #[test]
    fn shuffle_plays_every_remaining_track_once_before_repeating() {
        let mut queue = Queue::with_seed(7);
        for id in ["a", "b", "c", "d", "e"] {
            queue.push(id);
        }
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: true,
        });

        let mut visited = vec![queue.current().unwrap().to_owned()];
        for _ in 0..queue.len() - 1 {
            match queue.next() {
                QueueStep::Open(index) => visited.push(queue.track_at(index).unwrap().to_owned()),
                QueueStep::Exhausted => panic!("repeat=all must wrap, not exhaust"),
            }
        }
        visited.sort();
        assert_eq!(
            visited,
            vec!["a", "b", "c", "d", "e"],
            "shuffle must visit each remaining track exactly once"
        );
    }

    #[test]
    fn shuffle_is_a_permutation_and_keeps_the_current_track_playing() {
        let mut queue = Queue::with_seed(11);
        for id in ["a", "b", "c", "d", "e", "f"] {
            queue.push(id);
        }
        queue.next();
        queue.next();
        let current = queue.current().unwrap().to_owned();

        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: true,
        });

        let view = queue.view();
        let mut tracks = view.tracks.clone();
        tracks.sort();
        assert_eq!(tracks, vec!["a", "b", "c", "d", "e", "f"]);
        assert_eq!(
            queue.current(),
            Some(current.as_str()),
            "shuffle must not interrupt the current track"
        );
    }

    #[test]
    fn turning_shuffle_off_restores_insertion_order_around_the_current_track() {
        let mut queue = Queue::with_seed(3);
        for id in ["a", "b", "c", "d", "e"] {
            queue.push(id);
        }
        queue.next();
        queue.next();
        let current = queue.current().unwrap().to_owned();

        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: true,
        });
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::All,
            shuffle: false,
        });

        assert_eq!(queue.view().tracks, vec!["a", "b", "c", "d", "e"]);
        assert_eq!(queue.current(), Some(current.as_str()));
    }

    #[test]
    fn removing_an_earlier_track_keeps_the_current_one() {
        let mut queue = queue_with(&["a", "b", "c"]);
        queue.next();
        assert_eq!(queue.current(), Some("b"));
        assert_eq!(queue.remove(0), Some("a".into()));
        assert_eq!(queue.view().tracks, vec!["b", "c"]);
        assert_eq!(queue.current(), Some("b"));
        assert_eq!(queue.position(), Some(0));
    }

    #[test]
    fn removing_the_current_track_moves_to_the_next_one() {
        let mut queue = queue_with(&["a", "b", "c"]);
        queue.next();
        assert_eq!(queue.remove(1), Some("b".into()));
        assert_eq!(queue.current(), Some("c"));
        assert_eq!(queue.position(), Some(1));
    }

    #[test]
    fn removing_the_last_current_track_falls_back_to_the_new_last() {
        let mut queue = queue_with(&["a", "b", "c"]);
        queue.next();
        queue.next();
        assert_eq!(queue.remove(2), Some("c".into()));
        assert_eq!(queue.current(), Some("b"));
        assert_eq!(queue.position(), Some(1));
    }

    #[test]
    fn removing_the_only_track_empties_the_queue() {
        let mut queue = queue_with(&["a"]);
        assert_eq!(queue.remove(0), Some("a".into()));
        assert!(queue.is_empty());
        assert_eq!(queue.position(), None);
        assert_eq!(queue.next(), QueueStep::Exhausted);
    }

    #[test]
    fn remove_and_jump_reject_out_of_range_indices() {
        let mut queue = queue_with(&["a", "b"]);
        assert_eq!(queue.remove(9), None);
        assert_eq!(queue.jump(9), None);
        assert_eq!(queue.jump(1), Some(1));
        assert_eq!(queue.current(), Some("b"));
    }

    #[test]
    fn clear_empties_the_queue_and_resets_the_cursor() {
        let mut queue = queue_with(&["a", "b", "c"]);
        queue.next();
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.position(), None);
        assert_eq!(queue.view().tracks, Vec::<String>::new());
        queue.push("z");
        assert_eq!(queue.current(), Some("z"));
        assert_eq!(queue.position(), Some(0));
    }

    #[test]
    fn skip_candidates_cycle_regardless_of_repeat_mode() {
        let mut queue = queue_with(&["a", "b", "c"]);
        assert_eq!(queue.next_for_skip(), Some(1));
        assert_eq!(queue.next_for_skip(), Some(2));
        assert_eq!(queue.next_for_skip(), Some(0));
        queue.set_mode(PlaybackMode {
            repeat: RepeatMode::Off,
            shuffle: false,
        });
        assert_eq!(queue.next_for_skip(), Some(1));
    }

    #[test]
    fn find_locates_by_id_and_missing_ids_are_none() {
        let queue = queue_with(&["a", "b"]);
        assert_eq!(queue.find("b"), Some(1));
        assert_eq!(queue.find("zzz"), None);
        assert_eq!(queue.track_ids().collect::<Vec<_>>(), vec!["a", "b"]);
    }
}
