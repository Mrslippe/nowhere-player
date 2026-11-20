//! 悬浮标题栏 + 底部控制条的 GDI 绘制：背景、标题、按钮、滑杆与文字。
//!
//! 只画 overlay 自己的客户区；视频子窗口完全不参与。

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateSolidBrush,
    DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DRAW_TEXT_FORMAT, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint,
    FF_DONTCARE, FW_NORMAL, FillRect, GetStockObject, HDC, HFONT, HGDIOBJ, LineTo, MoveToEx,
    NULL_BRUSH, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, Rectangle, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::core::PCWSTR;

use crate::layout::{
    ControlHit, Hit, Rect, button_rect, control_layout, scale_dp, titlebar_height,
};

use super::Host;
use super::icons::{IconKind, icon_area};
use super::win::{to_rect, wide};

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

    let theme = host.theme;
    fill(hdc, Rect::new(0, 0, width, height), theme.surface);

    let close = button_rect(width, host.dpi, Hit::Close);
    let padding = scale_dp(12, host.dpi).max(4);
    let title_rect = Rect::new(padding, 0, (close.left - padding).max(padding), height);
    draw_title(hdc, &host.title, title_rect, host.font, theme.text);

    line(
        hdc,
        (0, height.saturating_sub(1)),
        (width, height.saturating_sub(1)),
        1,
        theme.line,
    );

    let stroke = scale_dp(1, host.dpi).max(1);
    let glyph = scale_dp(5, host.dpi).max(4);
    for hit in [Hit::Minimize, Hit::Maximize, Hit::Close] {
        let rect = button_rect(width, host.dpi, hit);
        let hovered = host.titlebar.hovered() == Some(hit);
        if hovered {
            let hover = if hit == Hit::Close {
                theme.danger
            } else {
                theme.hover
            };
            fill(hdc, rect, hover);
        }
        let center = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        // 关闭按钮 hover 时底色是 danger：用 accentFg 保证红底上的对比度；
        // 未 hover 时与其它按钮一样用正文色，避免浅色主题下看不清。
        let glyph_color = if hovered && hit == Hit::Close {
            theme.accent_fg
        } else {
            theme.text
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

/// 画一帧底部控制条。
pub(super) fn controls(host: &Host) {
    let layout = control_layout(host.client_w, host.client_h, host.dpi);
    let bar = layout.bar;
    let width = bar.width().max(0);
    let height = bar.height().max(0);
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: `host.controls` 是本进程创建的子窗口；paint 为本函数局部变量。
    let hdc = unsafe { BeginPaint(host.controls, &mut paint) };
    if hdc.is_invalid() {
        return;
    }

    let theme = host.theme;
    fill(hdc, Rect::new(0, 0, width, height), theme.surface);
    // 只保留顶部 1px 分隔线；不再画上下双边框。
    line(hdc, (0, 0), (width, 0), 1, theme.line);

    let local = |rect: Rect| {
        Rect::new(
            rect.left - bar.left,
            rect.top - bar.top,
            rect.right - bar.left,
            rect.bottom - bar.top,
        )
    };
    let hovered = host.hovered_control;
    let painter = host.icon_painter();
    let background = |hit: ControlHit| {
        if hovered == Some(hit) {
            theme.hover
        } else {
            theme.surface
        }
    };

    let play_pause = local(layout.play_pause);
    hover_background(hdc, hovered, play_pause, ControlHit::PlayPause, theme.hover);
    if host.paused {
        // 暂停中显示播放三角：正文色。
        painter.draw(
            hdc,
            play_pause,
            IconKind::Play,
            theme.text,
            background(ControlHit::PlayPause),
        );
    } else {
        // 播放中显示暂停双竖线：accent。
        painter.draw(
            hdc,
            play_pause,
            IconKind::Pause,
            theme.accent,
            background(ControlHit::PlayPause),
        );
    }

    let refresh = local(layout.refresh);
    hover_background(hdc, hovered, refresh, ControlHit::Refresh, theme.hover);
    painter.draw(
        hdc,
        refresh,
        IconKind::Refresh,
        theme.text,
        background(ControlHit::Refresh),
    );

    let mute = local(layout.mute);
    hover_background(hdc, hovered, mute, ControlHit::Mute, theme.hover);
    let (mute_icon, mute_color) = if host.volume == 0 {
        (IconKind::Mute, theme.muted)
    } else {
        (IconKind::Volume, theme.text)
    };
    painter.draw(
        hdc,
        mute,
        mute_icon,
        mute_color,
        background(ControlHit::Mute),
    );

    let slider = local(layout.slider);
    let track = if hovered == Some(ControlHit::Volume) {
        // 滑杆 hover 只提亮轨道 / 圆点，不画整块背景。
        theme.muted
    } else {
        theme.line
    };
    draw_slider(hdc, slider, host.volume, track, theme.accent, theme.accent);

    if !host.qualities.is_empty() {
        let quality = local(layout.quality);
        hover_background(hdc, hovered, quality, ControlHit::Quality, theme.hover);
        let color = if hovered == Some(ControlHit::Quality) {
            theme.text
        } else {
            theme.muted
        };
        let icon_background = background(ControlHit::Quality);
        let label = host.quality_label();
        if label.is_empty() {
            // 没有清晰度标签时只显示 chevron。
            painter.draw(hdc, quality, IconKind::Chevron, color, icon_background);
        } else {
            // 文字保留，chevron 单独绘制；不再依赖字体 `▾` 的基线。
            let chevron = Rect::new(
                (quality.right - icon_area(host.dpi)).max(quality.left),
                quality.top,
                quality.right,
                quality.bottom,
            );
            let gap = scale_dp(2, host.dpi).max(0);
            let label_rect = Rect::new(
                quality.left,
                quality.top,
                (chevron.left - gap).max(quality.left),
                quality.bottom,
            );
            draw_text(hdc, label, label_rect, host.font, DT_RIGHT, color);
            painter.draw(hdc, chevron, IconKind::Chevron, color, icon_background);
        }
    }
    draw_text(
        hdc,
        &host.cache_text(),
        local(layout.cache),
        host.font,
        DT_RIGHT,
        theme.faint,
    );

    // SAFETY: hdc 来自 BeginPaint，配对结束。
    unsafe {
        let _ = EndPaint(host.controls, &paint);
    }
}

fn hover_background(
    hdc: HDC,
    hovered: Option<ControlHit>,
    rect: Rect,
    hit: ControlHit,
    hover: (u8, u8, u8),
) {
    if hovered == Some(hit) {
        fill(hdc, rect, hover);
    }
}

fn filled_ellipse(hdc: HDC, left: i32, top: i32, right: i32, bottom: i32, rgb: (u8, u8, u8)) {
    // SAFETY: 画刷 / 空画笔只在调用期间使用；旧对象全部恢复并释放。
    unsafe {
        let brush = CreateSolidBrush(color(rgb));
        if brush.is_invalid() {
            return;
        }
        let old_brush = SelectObject(hdc, brush.into());
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        let _ = Ellipse(hdc, left, top, right, bottom);
        let _ = SelectObject(hdc, old_pen);
        let _ = SelectObject(hdc, old_brush);
        let _ = DeleteObject(brush.into());
    }
}

fn draw_slider(
    hdc: HDC,
    rect: Rect,
    volume: u8,
    track: (u8, u8, u8),
    fill_color: (u8, u8, u8),
    knob: (u8, u8, u8),
) {
    let left = rect.left + 2;
    let right = rect.right - 2;
    if right <= left {
        return;
    }
    let cy = (rect.top + rect.bottom) / 2;
    let x = left + ((right - left) * i32::from(volume.min(100))) / 100;
    line(hdc, (left, cy), (right, cy), 2, track);
    line(hdc, (left, cy), (x, cy), 2, fill_color);
    filled_ellipse(hdc, x - 4, cy - 4, x + 4, cy + 4, knob);
}

fn draw_text(
    hdc: HDC,
    text: &str,
    rect: Rect,
    font: HFONT,
    align: DRAW_TEXT_FORMAT,
    rgb: (u8, u8, u8),
) {
    if text.is_empty() || rect.width() <= 0 {
        return;
    }
    let mut text: Vec<u16> = text.encode_utf16().collect();
    let mut text_rect = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    // SAFETY: 文本缓冲、字体和 rect 在调用期间有效；选择器在之后恢复。
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, color(rgb));
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
            align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
    }
}

fn draw_title(hdc: HDC, title: &str, rect: Rect, font: HFONT, rgb: (u8, u8, u8)) {
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
        let _ = SetTextColor(hdc, color(rgb));
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
