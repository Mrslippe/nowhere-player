#![allow(dead_code)]

//! TUI 字形精选表。组件里不允许写字形字面量，统一从这里取。

/// 播放。
pub const PLAY: &str = "▶";
/// 暂停。
pub const PAUSE: &str = "❚❚";
/// 上一首。
pub const PREV: &str = "◀◀";
/// 下一首。
pub const NEXT: &str = "▶▶";
/// 进度已完成。
pub const PROGRESS_DONE: &str = "▰";
/// 进度未完成。
pub const PROGRESS_TODO: &str = "▱";
/// 封面占位填充。
pub const COVER_FILL: &str = "░";
/// 音符（非 emoji）。
pub const NOTE: &str = "♪";
/// 候选选中。
pub const SELECT: &str = "▸";
/// 状态开。
pub const STATUS_ON: &str = "●";
/// 状态关。
pub const STATUS_OFF: &str = "○";
/// 加载旋转帧。
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// 块字符八级（Braille 不可用时的备份）。
pub const BLOCK_LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_non_empty_and_not_emoji() {
        let glyphs = [
            PLAY,
            PAUSE,
            PREV,
            NEXT,
            PROGRESS_DONE,
            PROGRESS_TODO,
            COVER_FILL,
            NOTE,
            SELECT,
            STATUS_ON,
            STATUS_OFF,
        ];
        for glyph in glyphs {
            assert!(!glyph.is_empty());
            assert!(!is_emoji(glyph), "{glyph}");
        }
        for glyph in SPINNER.iter().chain(BLOCK_LEVELS.iter()) {
            assert!(!glyph.is_empty());
            assert!(!is_emoji(glyph), "{glyph}");
        }
    }

    fn is_emoji(value: &str) -> bool {
        value.chars().any(|ch| {
            let code = ch as u32;
            (0x1F300..=0x1FAFF).contains(&code) || (0x1F900..=0x1F9FF).contains(&code)
        })
    }
}
