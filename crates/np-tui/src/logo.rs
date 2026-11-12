//! NOWHERE logo 字形与合成。
//!
//! 不依赖 App / Ratatui，只做“字形表 → 等宽行”的纯计算；
//! 启动页与 `spikes/logo-font` 测试工具共用同一份字形。

use unicode_width::UnicodeWidthStr;

pub(crate) const GLYPH_N: [&str; 6] = [
    "███╗   ██╗",
    "████╗  ██║",
    "██╔██╗ ██║",
    "██║╚████║ ",
    "██║ ╚███║ ",
    "╚═╝  ╚══╝ ",
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
