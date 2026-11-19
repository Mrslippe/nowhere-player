//! 滚动窗口：Browse 树与 Settings/列表共用的选中项跟随算法。
//!
//! 窗口宽度由调用方给出：72×24 下 Browse 树是 7 行，Settings 列表扣除
//! 表头后是 6 行；这里只负责 `first`，不改变选中项本身。

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollState {
    pub(crate) first: usize,
    pub(crate) visible: usize,
}

impl ScrollState {
    pub(crate) fn new(visible: usize) -> Self {
        Self { first: 0, visible }
    }

    /// 让选中项落在 `[first, first + visible)`。
    ///
    /// 沿用 T-0098 的窗口算法：选中项贴住窗口底部时窗口整体下移，
    /// 空列表或零高窗口回到起点。
    pub(crate) fn follow(&mut self, selected: usize, total: usize) {
        if self.visible == 0 || total == 0 {
            self.first = 0;
            return;
        }
        self.first = selected.saturating_sub(self.visible - 1);
    }

    /// 当前可见的索引区间；超出总行数时截断。
    pub(crate) fn range(&self, total: usize) -> Range<usize> {
        let start = self.first.min(total);
        let end = self
            .first
            .saturating_add(self.visible)
            .min(total)
            .max(start);
        start..end
    }

    /// 窗口上方是否还有内容（仅测试辅助）。
    #[cfg(test)]
    pub(crate) fn has_above(&self) -> bool {
        self.first > 0
    }

    /// 窗口下方是否还有内容（仅测试辅助）。
    #[cfg(test)]
    pub(crate) fn has_below(&self, total: usize) -> bool {
        self.first.saturating_add(self.visible) < total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_zero_visible_windows_are_safe() {
        let mut state = ScrollState::new(0);
        state.follow(8, 9);
        assert_eq!(state.first, 0);
        assert_eq!(state.range(9), 0..0);
        assert!(!state.has_above());
        assert!(state.has_below(9));

        let mut empty = ScrollState::new(7);
        empty.follow(3, 0);
        assert_eq!(empty.first, 0);
        assert_eq!(empty.range(0), 0..0);
        assert!(!empty.has_above());
        assert!(!empty.has_below(0));
    }

    #[test]
    fn browse_seven_rows_keep_the_ninth_view_visible() {
        let mut state = ScrollState::new(7);
        state.follow(8, 9);
        assert_eq!(state.first, 2);
        assert!(state.has_above());
        assert!(!state.has_below(9));
        assert_eq!(state.range(9), 2..9);
    }

    #[test]
    fn settings_six_rows_keep_the_ninth_field_visible() {
        let mut state = ScrollState::new(6);
        state.follow(8, 9);
        assert_eq!(state.first, 3);
        assert!(state.has_above());
        assert!(!state.has_below(9));
        assert_eq!(state.range(9), 3..9);
    }

    #[test]
    fn selection_moves_the_window_only_at_the_bottom_edge() {
        let mut state = ScrollState::new(6);
        state.follow(4, 9);
        assert_eq!(state.first, 0);
        assert!(state.has_below(9));
        state.follow(5, 9);
        assert_eq!(state.first, 0);
        state.follow(6, 9);
        assert_eq!(state.first, 1);
        assert_eq!(state.range(9), 1..7);
        state.follow(0, 9);
        assert_eq!(state.first, 0);
    }

    #[test]
    fn range_is_clamped_when_total_shrinks() {
        let mut state = ScrollState::new(6);
        state.follow(8, 9);
        assert_eq!(state.range(4), 3..4);
        assert_eq!(state.range(0), 0..0);
    }
}
