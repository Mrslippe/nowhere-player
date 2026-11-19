//! 悬浮标题栏的 GDI 绘制：背景、标题、三个窗口按钮。
//!
//! 只画 overlay 自己的 32px 客户区；视频子窗口完全不参与。

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FillRect,
    GetStockObject, HDC, HFONT, HGDIOBJ, LineTo, MoveToEx, NULL_BRUSH, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, PS_SOLID, Rectangle, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::core::PCWSTR;

use crate::layout::{Hit, Rect, button_rect, scale_dp, titlebar_height};

use super::Host;
use super::win::{to_rect, wide};

const BACKGROUND: (u8, u8, u8) = (24, 24, 28);
const TITLE: (u8, u8, u8) = (232, 232, 236);
const BORDER: (u8, u8, u8) = (48, 48, 54);
const BUTTON_HOVER: (u8, u8, u8) = (52, 52, 58);
const CLOSE_HOVER: (u8, u8, u8) = (196, 43, 28);
const CLOSE_GLYPH: (u8, u8, u8) = (255, 255, 255);
const GLYPH: (u8, u8, u8) = (230, 230, 234);

fn color((red, green, blue): (u8, u8, u8)) -> COLORREF {
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

fn fill(hdc: HDC, rect: Rect, rgb: (u8, u8, u8)) {
    // SAFETY: 画刷 / rect 都在调用期间有效；GDI 对象在本函数内释放。
    unsafe {
        let brush = CreateSolidBrush(color(rgb));
        if !brush.is_invalid() {
            let rect = to_rect(rect);
            let _ = FillRect(hdc, &rect, brush);
            let _ = DeleteObject(brush.into());
        }
    }
}

fn line(hdc: HDC, a: (i32, i32), b: (i32, i32), width: i32, rgb: (u8, u8, u8)) {
    // SAFETY: 画笔在调用期间有效；SelectObject 恢复旧对象后删除新画笔。
    unsafe {
        let pen = CreatePen(PS_SOLID, width.max(1), color(rgb));
        if pen.is_invalid() {
            return;
        }
        let old = SelectObject(hdc, pen.into());
        let _ = MoveToEx(hdc, a.0, a.1, None);
        let _ = LineTo(hdc, b.0, b.1);
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(pen.into());
    }
}

fn outline(hdc: HDC, rect: Rect, width: i32, rgb: (u8, u8, u8)) {
    // SAFETY: 画笔与空画刷只在调用期间使用；旧对象全部恢复并释放新画笔。
    unsafe {
        let pen = CreatePen(PS_SOLID, width.max(1), color(rgb));
        if pen.is_invalid() {
            return;
        }
        let old_pen = SelectObject(hdc, pen.into());
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        let _ = Rectangle(hdc, rect.left, rect.top, rect.right, rect.bottom);
        let _ = SelectObject(hdc, old_brush);
        let _ = SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen.into());
    }
}

/// 创建标题栏字体；DPI 变化时由 [`Host`] 重建。
pub(super) fn create_font(dpi: u32) -> HFONT {
    // SAFETY: 字体名在调用期间有效，返回的 HFONT 由 Host 持有并在退出时删除。
    unsafe {
        let face = wide("Segoe UI");
        CreateFontW(
            -scale_dp(12, dpi).max(1),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            u32::from(DEFAULT_PITCH.0) | u32::from(FF_DONTCARE.0),
            PCWSTR::from_raw(face.as_ptr()),
        )
    }
}

/// 画一帧标题栏。
pub(super) fn overlay(host: &Host) {
    let width = host.client_w.max(0);
    let height = titlebar_height(host.dpi);
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: `host.overlay` 是本进程创建的窗口；paint 为本函数局部变量。
    let hdc = unsafe { BeginPaint(host.overlay, &mut paint) };
    if hdc.is_invalid() {
        return;
    }

    fill(hdc, Rect::new(0, 0, width, height), BACKGROUND);

    let close = button_rect(width, host.dpi, Hit::Close);
    let padding = scale_dp(12, host.dpi).max(4);
    let title_rect = Rect::new(padding, 0, (close.left - padding).max(padding), height);
    draw_title(hdc, &host.title, title_rect, host.font);

    line(
        hdc,
        (0, height.saturating_sub(1)),
        (width, height.saturating_sub(1)),
        1,
        BORDER,
    );

    let stroke = scale_dp(1, host.dpi).max(1);
    let glyph = scale_dp(5, host.dpi).max(4);
    for hit in [Hit::Minimize, Hit::Maximize, Hit::Close] {
        let rect = button_rect(width, host.dpi, hit);
        let hovered = host.titlebar.hovered() == Some(hit);
        if hovered {
            let hover = if hit == Hit::Close {
                CLOSE_HOVER
            } else {
                BUTTON_HOVER
            };
            fill(hdc, rect, hover);
        }
        let center = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let glyph_color = if hovered && hit == Hit::Close {
            CLOSE_GLYPH
        } else {
            GLYPH
        };
        match hit {
            Hit::Minimize => line(
                hdc,
                (center.0 - glyph, center.1),
                (center.0 + glyph, center.1),
                stroke,
                glyph_color,
            ),
            Hit::Maximize if host.is_maximized => {
                outline(
                    hdc,
                    Rect::new(
                        center.0 - glyph,
                        center.1 - glyph,
                        center.0 + glyph / 2,
                        center.1 + glyph / 2,
                    ),
                    stroke,
                    glyph_color,
                );
                outline(
                    hdc,
                    Rect::new(
                        center.0 - glyph / 2,
                        center.1 - glyph / 2,
                        center.0 + glyph,
                        center.1 + glyph,
                    ),
                    stroke,
                    glyph_color,
                );
            }
            Hit::Maximize => outline(
                hdc,
                Rect::new(
                    center.0 - glyph,
                    center.1 - glyph,
                    center.0 + glyph,
                    center.1 + glyph,
                ),
                stroke,
                glyph_color,
            ),
            Hit::Close => {
                line(
                    hdc,
                    (center.0 - glyph, center.1 - glyph),
                    (center.0 + glyph, center.1 + glyph),
                    stroke,
                    glyph_color,
                );
                line(
                    hdc,
                    (center.0 - glyph, center.1 + glyph),
                    (center.0 + glyph, center.1 - glyph),
                    stroke,
                    glyph_color,
                );
            }
            _ => {}
        }
    }

    // SAFETY: hdc 来自 BeginPaint，配对结束。
    unsafe {
        let _ = EndPaint(host.overlay, &paint);
    }
}

fn draw_title(hdc: HDC, title: &str, rect: Rect, font: HFONT) {
    if title.is_empty() || rect.width() <= 0 {
        return;
    }
    let mut text: Vec<u16> = title.encode_utf16().collect();
    let mut text_rect = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    // SAFETY: 文本缓冲、字体和 rect 在调用期间有效；选择器在之后恢复。
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, color(TITLE));
        let object: HGDIOBJ = if font.0.is_null() {
            GetStockObject(DEFAULT_GUI_FONT)
        } else {
            font.into()
        };
        let old = SelectObject(hdc, object);
        let _ = DrawTextW(
            hdc,
            &mut text,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
    }
}
