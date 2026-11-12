//! NOWHERE logo 字形与合成。
//!
//! 不依赖 App / Ratatui，只做“字形表 → 等宽行”的纯计算；
//! 启动页与 `spikes/logo-font` 测试工具共用同一份字形。

use unicode_width::UnicodeWidthStr;

pub(crate) const GLYPH_N: [&str; 6] = [
    "███╗  ██╗",
    "████╗ ██║",
    "██╔██╗██║",
    "██║╚████║",
    "██║ ╚███║",
    "╚═╝  ╚══╝",
];

pub(crate) const GLYPH_O: [&str; 6] = [
    "██████╗   ",
    "██╔═══██╗ ",
    "██║   ██║ ",
    "██║   ██║ ",
    "╚██████╔╝ ",
    "╚═════╝   ",
];

pub(crate) const GLYPH_W: [&str; 6] = [
    "██╗    ██╗",
    "██║    ██║",
    "██║ █╗ ██║",
    "██║███╗██║",
    "╚███╔███╔╝",
    "╚══╝╚══╝  ",
];

pub(crate) const GLYPH_H: [&str; 6] = [
    "██╗  ██╗",
    "██║  ██║",
    "███████║",
    "██╔══██║",
    "██║  ██║",
    "╚═╝  ╚═╝",
];

pub(crate) const GLYPH_E: [&str; 6] = [
    "███████╗",
    "██╔════╝",
    "█████╗  ",
    "██╔══╝  ",
    "███████╗",
    "╚══════╝",
];

pub(crate) const GLYPH_R: [&str; 6] = [
    "██████╗ ",
    "██╔══██╗",
    "██████╔╝",
    "██╔══██╗",
    "██║  ██║",
    "╚═╝  ╚═╝",
];

pub(crate) const GLYPHS: [[&str; 6]; 7] = [
    GLYPH_N, GLYPH_O, GLYPH_W, GLYPH_H, GLYPH_E, GLYPH_R, GLYPH_E,
];

pub(crate) fn compose(letters: &[[&str; 6]; 7]) -> Vec<String> {
    (0..6)
        .map(|row| {
            letters
                .iter()
                .map(|glyph| glyph[row])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

pub(crate) fn splash_art(width: u16) -> String {
    let composed = compose(&GLYPHS);
    let art_width = composed
        .first()
        .map(|line| UnicodeWidthStr::width(line.as_str()))
        .unwrap_or(0);
    let threshold = art_width.saturating_add(4);
    if usize::from(width) < threshold {
        "NOWHERE".to_owned()
    } else {
        composed.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N 是手绘 9 列。不要为了“每行等宽”在字形内部塞空格——
    /// 只允许在行尾补空格；当前字形 6 行都必须是 9 列。
    #[test]
    fn n_keeps_hand_drawn_nine_columns() {
        assert_eq!(
            GLYPH_N,
            [
                "███╗  ██╗",
                "████╗ ██║",
                "██╔██╗██║",
                "██║╚████║",
                "██║ ╚███║",
                "╚═╝  ╚══╝",
            ]
        );
        for row in GLYPH_N {
            assert_eq!(UnicodeWidthStr::width(row), 9, "{row:?}");
        }
    }

    #[test]
    fn composed_logo_is_67_columns() {
        let composed = compose(&GLYPHS);
        let widths: std::collections::HashSet<usize> = composed
            .iter()
            .map(|row| UnicodeWidthStr::width(row.as_str()))
            .collect();
        assert_eq!(widths, std::collections::HashSet::from([67]));
    }
}
