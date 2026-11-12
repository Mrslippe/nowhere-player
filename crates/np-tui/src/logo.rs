//! NOWHERE logo 字形与合成。
//!
//! 绘制规则：
//! - 每个字形只写“自然形状”，**行尾不留空格**；
//! - 每个字形声明一个 `width`（槽位宽度），合成器只在行尾补空格到槽位；
//! - 禁止在字形内部塞空格来凑等宽——那会改变字母形状（T-0088 的 N 就是例子）。
//!
//! 这样任意字母都可以按手绘形状直接录入，布局对齐由 `compose` 统一负责。
//! 不依赖 App / Ratatui，启动页与 `spikes/logo-font` 测试工具共用。

use unicode_width::UnicodeWidthStr;

/// 一个 6 行高的字形。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glyph {
    /// 自然形状；每行不得以空格结尾。
    pub rows: [&'static str; 6],
    /// 该字形在合成结果中的固定槽位宽度。
    pub width: usize,
}

const fn glyph(rows: [&'static str; 6], width: usize) -> Glyph {
    Glyph { rows, width }
}

/// N：手绘 9 列，6 行完全一致。
pub(crate) const GLYPH_N: Glyph = glyph(
    [
        "███╗  ██╗",
        "████╗ ██║",
        "██╔██╗██║",
        "██║╚████║",
        "██║ ╚███║",
        "╚═╝  ╚══╝",
    ],
    9,
);

/// O：自然形状 9 列；顶行、底行各有一个前导空格，不能省。
pub(crate) const GLYPH_O: Glyph = glyph(
    [
        " ██████╗",
        "██╔═══██╗",
        "██║   ██║",
        "██║   ██║",
        "╚██████╔╝",
        " ╚═════╝",
    ],
    9,
);

pub(crate) const GLYPH_W: Glyph = glyph(
    [
        "██╗    ██╗",
        "██║    ██║",
        "██║ █╗ ██║",
        "██║███╗██║",
        "╚███╔███╔╝",
        "╚══╝╚══╝",
    ],
    10,
);

pub(crate) const GLYPH_H: Glyph = glyph(
    [
        "██╗  ██╗",
        "██║  ██║",
        "███████║",
        "██╔══██║",
        "██║  ██║",
        "╚═╝  ╚═╝",
    ],
    8,
);

pub(crate) const GLYPH_E: Glyph = glyph(
    [
        "███████╗",
        "██╔════╝",
        "█████╗",
        "██╔══╝",
        "███████╗",
        "╚══════╝",
    ],
    8,
);

pub(crate) const GLYPH_R: Glyph = glyph(
    [
        "██████╗",
        "██╔══██╗",
        "██████╔╝",
        "██╔══██╗",
        "██║  ██║",
        "╚═╝  ╚═╝",
    ],
    8,
);

pub(crate) const GLYPHS: [Glyph; 7] = [
    GLYPH_N, GLYPH_O, GLYPH_W, GLYPH_H, GLYPH_E, GLYPH_R, GLYPH_E,
];

/// 把一行自然形状右补空格到槽位宽度。
///
/// 只处理行尾；行内空格原样保留，因此不会改变字母形状。
fn padded(row: &str, width: usize) -> String {
    let visible = row.trim_end();
    let pad = width.saturating_sub(UnicodeWidthStr::width(visible));
    let mut out = String::with_capacity(visible.len() + pad);
    out.push_str(visible);
    out.push_str(&" ".repeat(pad));
    out
}

pub(crate) fn compose(letters: &[Glyph]) -> Vec<String> {
    (0..6)
        .map(|row| {
            letters
                .iter()
                .map(|glyph| padded(glyph.rows[row], glyph.width))
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

    fn visible_width(row: &str) -> usize {
        UnicodeWidthStr::width(row.trim_end())
    }

    /// N 是手绘 9 列。不要为了“每行等宽”在字形内部塞空格——
    /// 只允许在行尾补空格；当前字形 6 行都必须是 9 列。
    #[test]
    fn n_keeps_hand_drawn_nine_columns() {
        assert_eq!(
            GLYPH_N.rows,
            [
                "███╗  ██╗",
                "████╗ ██║",
                "██╔██╗██║",
                "██║╚████║",
                "██║ ╚███║",
                "╚═╝  ╚══╝",
            ]
        );
        assert_eq!(GLYPH_N.width, 9);
        for row in GLYPH_N.rows {
            assert_eq!(visible_width(row), 9, "{row:?}");
        }
    }

    /// O 的最后一行有前导空格；这是字形的一部分，不是补位。
    #[test]
    fn o_keeps_bottom_curve_alignment() {
        assert_eq!(
            GLYPH_O.rows,
            [
                " ██████╗",
                "██╔═══██╗",
                "██║   ██║",
                "██║   ██║",
                "╚██████╔╝",
                " ╚═════╝",
            ]
        );
        assert_eq!(GLYPH_O.width, 9);
        let top = GLYPH_O.rows[0];
        assert!(
            top.starts_with(' '),
            "O top must start with a space: {top:?}"
        );
        assert_eq!(top.chars().filter(|ch| *ch == '█').count(), 6);
        assert!(top.ends_with('╗'), "O top must end with ╗: {top:?}");
        assert!(
            GLYPH_O.rows[5].starts_with(' '),
            "O bottom must start with a space"
        );
    }

    /// 数据层不允许用行尾空格“预对齐”；对齐是合成器的职责。
    #[test]
    fn glyph_rows_are_right_trimmed() {
        for glyph in GLYPHS {
            for row in glyph.rows {
                assert_eq!(
                    row,
                    row.trim_end(),
                    "glyph row has trailing spaces: {row:?}"
                );
            }
        }
    }

    #[test]
    fn glyph_rows_fit_their_slots() {
        for glyph in GLYPHS {
            for row in glyph.rows {
                assert!(
                    visible_width(row) <= glyph.width,
                    "row {row:?} wider than slot {}",
                    glyph.width
                );
            }
        }
    }

    #[test]
    fn composed_logo_is_66_columns() {
        assert_eq!(
            GLYPHS.iter().map(|glyph| glyph.width).collect::<Vec<_>>(),
            [9, 9, 10, 8, 8, 8, 8]
        );

        let composed = compose(&GLYPHS);
        let widths: std::collections::HashSet<usize> = composed
            .iter()
            .map(|row| UnicodeWidthStr::width(row.as_str()))
            .collect();
        assert_eq!(widths, std::collections::HashSet::from([66]));
    }
}
