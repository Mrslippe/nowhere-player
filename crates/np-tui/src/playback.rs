//! 播放队列：唯一管线、去重、四模式和 next/prev 决策。
//!
//! 这里不接触音频设备；`TuiAudio` 只负责把当前项变成实际播放。

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use np_core::{PlaybackMode, RepeatMode};

use crate::model::TrackRef;

/// 队列中的歌曲身份；本地按路径，远程按 source + id。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TrackKey {
    Local(PathBuf),
    Remote { source: String, id: String },
}

impl TrackKey {
    pub(crate) fn from_track(track: &TrackRef) -> Self {
        match track {
            TrackRef::Local(path) => Self::Local(path.clone()),
            TrackRef::Remote { source, id } => Self::Remote {
                source: source.clone(),
                id: id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct QueueItem {
    pub(crate) key: TrackKey,
    pub(crate) track: TrackRef,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) duration_ms: u64,
}

impl QueueItem {
    pub(crate) fn new(
        track: TrackRef,
        title: impl Into<String>,
        artist: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            key: TrackKey::from_track(&track),
            track,
            title: title.into(),
            artist: artist.into(),
            duration_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueMode {
    SingleLoop,
    Shuffle,
    Sequential,
    ListLoop,
}

impl QueueMode {
    pub(crate) fn to_playback_mode(self) -> PlaybackMode {
        match self {
            Self::SingleLoop => PlaybackMode {
                repeat: RepeatMode::One,
                shuffle: false,
            },
            Self::Shuffle => PlaybackMode {
                repeat: RepeatMode::Off,
                shuffle: true,
            },
            Self::Sequential => PlaybackMode::default(),
            Self::ListLoop => PlaybackMode {
                repeat: RepeatMode::All,
                shuffle: false,
            },
        }
    }

    pub(crate) fn from_playback_mode(mode: PlaybackMode) -> Self {
        if mode.shuffle {
            Self::Shuffle
        } else {
            match mode.repeat {
                RepeatMode::One => Self::SingleLoop,
                RepeatMode::All => Self::ListLoop,
                RepeatMode::Off => Self::Sequential,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertOutcome {
    Inserted(usize),
    Existing(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct PlaybackQueue {
    items: Vec<QueueItem>,
    current: Option<usize>,
    mode: QueueMode,
    shuffle_order: Vec<usize>,
    history: Vec<usize>,
    rng: u64,
}

impl Default for PlaybackQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackQueue {
    pub(crate) fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |duration| {
                duration.as_nanos() as u64 ^ 0x9E37_79B9_7F4A_7C15
            });
        Self::with_seed(seed)
    }

    pub(crate) fn with_seed(seed: u64) -> Self {
        Self {
            items: Vec::new(),
            current: None,
            mode: QueueMode::Sequential,
            shuffle_order: Vec::new(),
            history: Vec::new(),
            rng: if seed == 0 { 1 } else { seed },
        }
    }

    pub(crate) fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<&QueueItem> {
        self.current.and_then(|index| self.items.get(index))
    }

    pub(crate) fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub(crate) fn item_at(&self, index: usize) -> Option<&QueueItem> {
        self.items.get(index)
    }

    pub(crate) fn mode(&self) -> QueueMode {
        self.mode
    }

    pub(crate) fn set_mode(&mut self, mode: QueueMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.history.clear();
        if mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        } else {
            self.shuffle_order.clear();
        }
    }

    pub(crate) fn contains_key(&self, key: &TrackKey) -> Option<usize> {
        self.items.iter().position(|item| &item.key == key)
    }

    pub(crate) fn jump(&mut self, index: usize) -> Option<usize> {
        if index >= self.items.len() {
            return None;
        }
        self.current = Some(index);
        self.history.clear();
        if self.mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        }
        Some(index)
    }

    pub(crate) fn insert_front(&mut self, item: QueueItem) -> InsertOutcome {
        if let Some(index) = self.contains_key(&item.key) {
            return InsertOutcome::Existing(index);
        }
        self.items.insert(0, item);
        self.current = Some(0);
        self.history.clear();
        if self.mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        }
        InsertOutcome::Inserted(0)
    }

    pub(crate) fn push_back(&mut self, item: QueueItem) -> InsertOutcome {
        if let Some(index) = self.contains_key(&item.key) {
            return InsertOutcome::Existing(index);
        }
        self.items.push(item);
        let index = self.items.len() - 1;
        if self.current.is_none() {
            self.current = Some(index);
        }
        if self.mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        }
        InsertOutcome::Inserted(index)
    }

    /// 搜索/媒体库选择：已存在则跳播，否则插队首并播放。
    pub(crate) fn play_or_prepend(&mut self, item: QueueItem) -> usize {
        match self.insert_front(item) {
            InsertOutcome::Existing(index) => {
                self.current = Some(index);
                self.history.clear();
                if self.mode == QueueMode::Shuffle {
                    self.rebuild_shuffle();
                }
                index
            }
            InsertOutcome::Inserted(index) => index,
        }
    }

    /// 歌单整体替换；保留首次出现，返回选中歌曲下标。
    pub(crate) fn replace_playlist(
        &mut self,
        items: Vec<QueueItem>,
        selected: Option<&TrackKey>,
    ) -> Option<usize> {
        let mut seen = HashSet::new();
        self.items = items
            .into_iter()
            .filter(|item| seen.insert(item.key.clone()))
            .collect();
        self.history.clear();
        self.current = if self.items.is_empty() {
            None
        } else {
            let index = selected.and_then(|key| self.contains_key(key)).unwrap_or(0);
            Some(index)
        };
        if self.mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        } else {
            self.shuffle_order.clear();
        }
        self.current
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.current = None;
        self.shuffle_order.clear();
        self.history.clear();
    }

    pub(crate) fn remove(&mut self, index: usize) -> Option<QueueItem> {
        if index >= self.items.len() {
            return None;
        }
        let removed = self.items.remove(index);
        self.current = match self.current {
            None => None,
            Some(current) if current > index => Some(current - 1),
            Some(current) if current == index => {
                if self.items.is_empty() {
                    None
                } else {
                    Some(current.min(self.items.len() - 1))
                }
            }
            current => current,
        };
        self.history.clear();
        if self.mode == QueueMode::Shuffle {
            self.rebuild_shuffle();
        } else {
            self.shuffle_order.clear();
        }
        Some(removed)
    }

    /// 下一首。`None` 表示顺序模式已到末尾、随机模式已播完。
    pub(crate) fn next(&mut self) -> Option<usize> {
        let current = self.current?;
        if self.items.is_empty() {
            return None;
        }
        match self.mode {
            QueueMode::SingleLoop => Some(current),
            QueueMode::Sequential => {
                if current + 1 < self.items.len() {
                    self.current = Some(current + 1);
                    Some(current + 1)
                } else {
                    None
                }
            }
            QueueMode::ListLoop => {
                let next = (current + 1) % self.items.len();
                self.current = Some(next);
                Some(next)
            }
            QueueMode::Shuffle => {
                if self.shuffle_order.is_empty() {
                    self.rebuild_shuffle();
                }
                let position = self
                    .shuffle_order
                    .iter()
                    .position(|index| *index == current)?;
                let next = *self.shuffle_order.get(position + 1)?;
                self.history.push(current);
                self.current = Some(next);
                Some(next)
            }
        }
    }

    /// 上一首。单曲循环不移动；随机模式按已播放历史回退。
    pub(crate) fn prev(&mut self) -> Option<usize> {
        let current = self.current?;
        if self.items.is_empty() {
            return None;
        }
        match self.mode {
            QueueMode::SingleLoop => Some(current),
            QueueMode::Sequential => {
                if current > 0 {
                    self.current = Some(current - 1);
                    Some(current - 1)
                } else {
                    None
                }
            }
            QueueMode::ListLoop => {
                let previous = (current + self.items.len() - 1) % self.items.len();
                self.current = Some(previous);
                Some(previous)
            }
            QueueMode::Shuffle => {
                let previous = self.history.pop()?;
                if previous >= self.items.len() {
                    return self.prev();
                }
                self.current = Some(previous);
                Some(previous)
            }
        }
    }

    fn rebuild_shuffle(&mut self) {
        self.shuffle_order = (0..self.items.len()).collect();
        if let Some(current) = self.current
            && let Some(position) = self
                .shuffle_order
                .iter()
                .position(|index| *index == current)
        {
            self.shuffle_order.swap(0, position);
        }
        let start = usize::from(self.current.is_some());
        for index in (start + 1..self.shuffle_order.len()).rev() {
            let span = u64::try_from(index - start + 1).unwrap_or(u64::MAX);
            let target = start + (self.rand() % span) as usize;
            self.shuffle_order.swap(index, target);
        }
    }

    fn rand(&mut self) -> u64 {
        let mut value = self.rng;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.rng = value;
        value.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str) -> QueueItem {
        QueueItem::new(TrackRef::Local(PathBuf::from(key)), key, "", 0)
    }

    fn queue(names: &[&str]) -> PlaybackQueue {
        let mut queue = PlaybackQueue::with_seed(7);
        queue.replace_playlist(names.iter().map(|name| item(name)).collect(), None);
        queue
    }

    fn current_key(queue: &PlaybackQueue) -> Option<String> {
        queue.current().map(|item| item.title.clone())
    }

    #[test]
    fn prepend_after_current_moves_next_to_old_first() {
        let mut queue = queue(&["a", "b", "c"]);
        queue.jump(1);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
        queue.play_or_prepend(item("x"));
        assert_eq!(current_key(&queue).as_deref(), Some("x"));
        assert_eq!(queue.next(), Some(1));
        assert_eq!(current_key(&queue).as_deref(), Some("a"));
    }

    #[test]
    fn existing_track_jumps_instead_of_duplicate() {
        let mut queue = queue(&["a", "b", "c"]);
        let index = queue.play_or_prepend(item("b"));
        assert_eq!(index, 1);
        assert_eq!(queue.len(), 3);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
    }

    #[test]
    fn playlist_replace_dedupes_and_selects() {
        let mut queue = PlaybackQueue::with_seed(7);
        queue.replace_playlist(
            vec![item("a"), item("b"), item("a"), item("c")],
            Some(&TrackKey::Local(PathBuf::from("c"))),
        );
        assert_eq!(queue.len(), 3);
        assert_eq!(current_key(&queue).as_deref(), Some("c"));
    }

    #[test]
    fn single_loop_keeps_current_on_manual_next_and_prev() {
        let mut queue = queue(&["a", "b", "c"]);
        queue.jump(1);
        queue.set_mode(QueueMode::SingleLoop);
        assert_eq!(queue.next(), Some(1));
        assert_eq!(queue.prev(), Some(1));
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
    }

    #[test]
    fn sequential_stops_at_ends_and_list_loop_wraps() {
        let mut queue = queue(&["a", "b"]);
        queue.jump(1);
        assert_eq!(queue.next(), None);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
        queue.jump(0);
        queue.set_mode(QueueMode::ListLoop);
        assert_eq!(queue.prev(), Some(1));
        assert_eq!(queue.next(), Some(0));
    }

    #[test]
    fn shuffle_visits_every_item_once_and_stops() {
        let mut queue = PlaybackQueue::with_seed(99);
        queue.replace_playlist(
            ["a", "b", "c", "d"].iter().map(|name| item(name)).collect(),
            Some(&TrackKey::Local(PathBuf::from("b"))),
        );
        queue.set_mode(QueueMode::Shuffle);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
        let mut seen = vec![queue.current_index().unwrap()];
        while let Some(index) = queue.next() {
            seen.push(index);
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    #[test]
    fn remove_adjusts_current_without_duplicates() {
        let mut queue = queue(&["a", "b", "c"]);
        queue.jump(1);
        assert!(queue.remove(0).is_some());
        assert_eq!(queue.len(), 2);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
        assert!(queue.remove(1).is_some());
        assert_eq!(queue.len(), 1);
        assert_eq!(current_key(&queue).as_deref(), Some("b"));
    }
}
