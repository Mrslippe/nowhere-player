//! 控制条图标：Windows 系统图标字体优先，细线几何回退。
//!
//! 主选 Windows 11 的 `Segoe Fluent Icons`，其次 Windows 10 的
//! `Segoe MDL2 Assets`；两套字体都不可用时用 GDI 几何画笔手绘 Lucide /
//! Feather 风格细线图标。字体检测必须比对 `GetTextFaceW` 返回的**实际**
//! 字体名：`CreateFontW` 在字体缺失时会静默替换成别的字体，只看句柄非空
//! 会把豆腐块画到屏幕上。
//!
//! 细线回退在 2× 离屏 DIB 上绘制，再按 2×2 平均降采样后贴回目标 DC：
//! 96 DPI 下 1.5px 的线宽在只接受整数画笔宽度的 GDI 里靠超采样得到，
//! 同时顺带获得抗锯齿边缘。端点 / 连接使用 `PS_ENDCAP_ROUND` /
//! `PS_JOIN_ROUND` 的几何画笔。
//!
//! `NP_VIDEO_ICON_STYLE` 可强制样式：`fluent`（系统图标字体）、`line`
//! （细线）、`auto`（默认，Fluent → MDL2 → 细线）；其它值按 `auto` 处理。

use std::ffi::c_void;

use windows::Win32::Foundation::{COLORREF, POINT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BS_SOLID, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, ExtCreatePen, FF_DONTCARE, FW_NORMAL, GGI_MARK_NONEXISTING_GLYPHS, GetGlyphIndicesW,
    GetStockObject, GetTextFaceW, HDC, HFONT, LOGBRUSH, NULL_BRUSH, OUT_DEFAULT_PRECIS,
    PS_ENDCAP_ROUND, PS_GEOMETRIC, PS_JOIN_ROUND, PS_SOLID, Polygon, Polyline, SelectObject,
    SetBkMode, SetDIBitsToDevice, SetTextColor, TRANSPARENT,
};

use crate::layout::{Rect, scale_dp};

use super::win;

/// 强制图标样式的环境变量。
pub(super) const STYLE_ENV: &str = "NP_VIDEO_ICON_STYLE";

/// 细线图标的绘制区域（96 DPI 逻辑像素）。
const ICON_AREA_DP: i32 = 16;
/// 细线图标线宽（96 DPI 约 1.5px）。
const MARK_STROKE_DP: f32 = 1.5;
/// 暂停双竖线略粗（96 DPI 2px），避免两条 1.5px 细线显得发虚。
const PAUSE_STROKE_DP: f32 = 2.0;
/// 离屏超采样倍数：2× 缓冲 + 2×2 平均降采样。
const SUPERSAMPLE: i32 = 2;

/// 主题里的具体颜色（与 `paint.rs` 一致）。
pub(super) type Rgb = (u8, u8, u8);

/// 图标样式。Fluent / Mdl2 都是系统图标字体，Line 是几何回退。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IconStyle {
    Fluent,
    Mdl2,
    Line,
}

impl IconStyle {
    /// 对应的字体家族名；细线样式没有字体。
    fn face(self) -> Option<&'static str> {
        match self {
            Self::Fluent => Some("Segoe Fluent Icons"),
            Self::Mdl2 => Some("Segoe MDL2 Assets"),
            Self::Line => None,
        }
    }

    /// 日志 / 结果里用的稳定名字。
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Fluent => "fluent",
            Self::Mdl2 => "mdl2",
            Self::Line => "line",
        }
    }
}

/// 控制条上的图标种类；码位固定，见 T-0119。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IconKind {
    Play,
    Pause,
    Refresh,
    Volume,
    Mute,
    Chevron,
}

impl IconKind {
    /// 系统图标字体里的码位。
    pub(super) const fn code_point(self) -> u32 {
        match self {
            Self::Play => 0xE768,
            Self::Pause => 0xE769,
            Self::Refresh => 0xE72C,
            Self::Volume => 0xE767,
            Self::Mute => 0xE74F,
            Self::Chevron => 0xE70D,
        }
    }

    /// 字体探测时要求全部存在的字形。
    pub(super) const ALL: [Self; 6] = [
        Self::Play,
        Self::Pause,
        Self::Refresh,
        Self::Volume,
        Self::Mute,
        Self::Chevron,
    ];

    /// 细线绘制时的线宽（96 DPI 逻辑像素）。
    fn stroke_dp(self) -> f32 {
        match self {
            Self::Pause => PAUSE_STROKE_DP,
            _ => MARK_STROKE_DP,
        }
    }
}

/// `NP_VIDEO_ICON_STYLE` 的解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preference {
    Auto,
    Fluent,
    Line,
}

impl Preference {
    /// 非法值 / 未设置按 auto。
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("fluent") => Self::Fluent,
            Some(value) if value.eq_ignore_ascii_case("line") => Self::Line,
            _ => Self::Auto,
        }
    }
}

/// 绘制入口：样式、图标字体、DPI 在 `Host` 里解析好后传进来。
#[derive(Debug, Clone, Copy)]
pub(super) struct Painter {
    pub(super) style: IconStyle,
    pub(super) font: HFONT,
    pub(super) dpi: u32,
}

impl Painter {
    /// 把图标画在 `rect` 中心；颜色与背景都由调用方按主题传入。
    ///
    /// `background` 只给细线回退的离屏画布用：2×2 降采样后边缘会与
    /// 按钮底色混合，必须用绘制时已知的底色，才能和周围背景无缝。
    pub(super) fn draw(&self, hdc: HDC, rect: Rect, kind: IconKind, rgb: Rgb, background: Rgb) {
        if rect.width() <= 0 || rect.height() <= 0 {
            return;
        }
        if self.style == IconStyle::Line {
            self.draw_line(hdc, rect, kind, rgb, background);
        } else if !draw_glyph(hdc, rect, kind, rgb, self.font) {
            // 字体在运行期意外不可用：不留空白，退到细线路径。
            self.draw_line(hdc, rect, kind, rgb, background);
        }
    }

    fn draw_line(&self, hdc: HDC, rect: Rect, kind: IconKind, rgb: Rgb, background: Rgb) {
        let square = icon_square(rect, self.dpi);
        let size = square.width();
        if size <= 0 {
            return;
        }
        let stroke_px = device_stroke_px(kind, self.dpi);
        match render_supersampled(kind, size, stroke_px, rgb, background) {
            Some(pixels) => blit(hdc, square.left, square.top, size, &pixels),
            // 离屏 DIB 创建失败：直接在当前 DC 上画，哪怕线宽只能取整。
            None => draw_direct(hdc, square, kind, rgb, stroke_px),
        }
    }
}

/// 解析样式并创建图标字体；返回 `(样式, 字体)`。
///
/// 细线样式返回空字体；字体 / 字形缺失时同样落到细线。
pub(super) fn load(dpi: u32) -> (IconStyle, HFONT) {
    let raw = std::env::var(STYLE_ENV).ok();
    let preference = Preference::parse(raw.as_deref());
    let style = select_style(preference, |style| face_available(style, dpi));
    let font = style
        .face()
        .and_then(|face| create_verified(face, dpi))
        .unwrap_or_default();
    if style != IconStyle::Line && font.is_invalid() {
        // 探针通过但真正创建失败：不要把空字体交给绘制路径。
        return (IconStyle::Line, HFONT::default());
    }
    (style, font)
}

/// DPI 变化时按已解析样式重建字体；失败由调用方回退到细线。
pub(super) fn reload(style: IconStyle, dpi: u32) -> HFONT {
    style
        .face()
        .and_then(|face| create_verified(face, dpi))
        .unwrap_or_default()
}

/// 图标绘制区域边长（随 DPI 缩放）；控制条清晰度按钮也用它给 chevron 留位。
pub(super) fn icon_area(dpi: u32) -> i32 {
    scale_dp(ICON_AREA_DP, dpi).max(1)
}

/// 图标绘制区域：`rect` 中心的一个 16dp 正方形，随 DPI 缩放。
fn icon_square(rect: Rect, dpi: u32) -> Rect {
    let size = icon_area(dpi);
    let left = rect.left + (rect.width() - size) / 2;
    let top = rect.top + (rect.height() - size) / 2;
    Rect::new(left, top, left + size, top + size)
}

/// 96 DPI 下 1.5px 的线宽按 DPI 缩放。
fn device_stroke_px(kind: IconKind, dpi: u32) -> f32 {
    kind.stroke_dp() * dpi as f32 / 96.0
}

/// 可用性探测：创建 → 用实际字体名 / 字形校验 → 释放。
fn face_available(style: IconStyle, dpi: u32) -> bool {
    let Some(face) = style.face() else {
        return false;
    };
    let Some(font) = create_verified(face, dpi) else {
        return false;
    };
    // SAFETY: 探针字体由本函数创建，校验后立即释放。
    unsafe {
        let _ = DeleteObject(font.into());
    }
    true
}

/// 按优先级选样式；`usable` 由调用方提供（生产环境是 GDI 探针，测试里是闭包）。
fn select_style(preference: Preference, mut usable: impl FnMut(IconStyle) -> bool) -> IconStyle {
    match preference {
        Preference::Line => IconStyle::Line,
        Preference::Auto | Preference::Fluent => [IconStyle::Fluent, IconStyle::Mdl2]
            .into_iter()
            .find(|&style| usable(style))
            .unwrap_or(IconStyle::Line),
    }
}

/// 创建图标字体并验证 GDI 实际选中的字体名与全部码位字形。
fn create_verified(face: &str, dpi: u32) -> Option<HFONT> {
    let font = create_face(face, dpi);
    if font.is_invalid() {
        return None;
    }
    // SAFETY: 用内存 DC 校验字体映射结果；GDI 资源在函数末尾释放。
    let hdc = unsafe { CreateCompatibleDC(None) };
    if hdc.is_invalid() {
        // SAFETY: 字体由本函数创建，校验失败时释放。
        unsafe {
            let _ = DeleteObject(font.into());
        }
        return None;
    }
    // SAFETY: hdc 有效；font 在调用期间有效。
    let old = unsafe { SelectObject(hdc, font.into()) };
    let verified = !old.is_invalid() && face_name_matches(hdc, face) && glyphs_exist(hdc);
    // SAFETY: hdc 有效；恢复旧对象后释放 DC。
    unsafe {
        if !old.is_invalid() {
            let _ = SelectObject(hdc, old);
        }
        let _ = DeleteDC(hdc);
    }
    if verified {
        Some(font)
    } else {
        // SAFETY: 字体由本函数创建，校验失败时释放。
        unsafe {
            let _ = DeleteObject(font.into());
        }
        None
    }
}

fn create_face(face: &str, dpi: u32) -> HFONT {
    let name = win::wide(face);
    let height = -scale_dp(ICON_AREA_DP, dpi).max(1);
    // SAFETY: 字体名在调用期间有效；返回的 HFONT 由调用方持有 / 释放。
    unsafe {
        CreateFontW(
            height,
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
            win::pcwstr(&name),
        )
    }
}

fn face_name_matches(hdc: HDC, expected: &str) -> bool {
    let mut buffer = [0u16; 64];
    // SAFETY: buffer 是有效可写切片；hdc 已选中待校验字体。
    let len = unsafe { GetTextFaceW(hdc, Some(&mut buffer)) };
    if len <= 0 {
        return false;
    }
    let actual = String::from_utf16_lossy(&buffer[..len as usize]);
    actual.trim_end_matches('\0').eq_ignore_ascii_case(expected)
}

fn glyphs_exist(hdc: HDC) -> bool {
    let text: Vec<u16> = IconKind::ALL
        .iter()
        .map(|kind| kind.code_point() as u16)
        .collect();
    let mut indices = [0u16; IconKind::ALL.len()];
    // SAFETY: text / indices 在调用期间有效；hdc 已选中待校验字体。
    let count = unsafe {
        GetGlyphIndicesW(
            hdc,
            win::pcwstr(&text),
            text.len() as i32,
            indices.as_mut_ptr(),
            GGI_MARK_NONEXISTING_GLYPHS,
        )
    };
    // GDI_ERROR（0xFFFF_FFFF）= 调用失败；0xFFFF = 该码位没有字形。
    count != u32::MAX
        && count as usize == text.len()
        && indices.iter().all(|&index| index != 0xFFFF)
}

/// 用图标字体居中绘制一个字形。
fn draw_glyph(hdc: HDC, rect: Rect, kind: IconKind, rgb: Rgb, font: HFONT) -> bool {
    if font.is_invalid() {
        return false;
    }
    let mut text = [kind.code_point() as u16];
    let mut text_rect = win::to_rect(rect);
    // SAFETY: 文本缓冲 / 字体 / rect 在调用期间有效；选择器在之后恢复。
    unsafe {
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, color(rgb));
        let old = SelectObject(hdc, font.into());
        if old.is_invalid() {
            return false;
        }
        let height = DrawTextW(
            hdc,
            &mut text,
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
        height > 0
    }
}

/// 在 2× 离屏 DIB 上画细线图标，返回降采样后的 BGRA 像素。
fn render_supersampled(
    kind: IconKind,
    size: i32,
    stroke_px: f32,
    rgb: Rgb,
    background: Rgb,
) -> Option<Vec<u32>> {
    let buffer = size.checked_mul(SUPERSAMPLE)?;
    if buffer <= 0 {
        return None;
    }
    let info = bitmap_info(buffer, buffer);
    let mut bits: *mut c_void = std::ptr::null_mut();
    // SAFETY: info 描述 buffer×buffer 的 32 位 DIB；bits 在成功后由系统写入指针。
    let bitmap =
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    if bits.is_null() {
        // SAFETY: bitmap 由 CreateDIBSection 创建，失败路径释放。
        unsafe {
            let _ = DeleteObject(bitmap.into());
        }
        return None;
    }
    // SAFETY: 内存 DC 只用于离屏绘制；函数末尾成对释放。
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.is_invalid() {
        // SAFETY: bitmap 已创建且未选入任何 DC。
        unsafe {
            let _ = DeleteObject(bitmap.into());
        }
        return None;
    }
    // SAFETY: dc / bitmap 有效。
    let old_bitmap = unsafe { SelectObject(dc, bitmap.into()) };
    if old_bitmap.is_invalid() {
        // SAFETY: dc 与 bitmap 均有效，只是选入失败；释放后退出。
        unsafe {
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bitmap.into());
        }
        return None;
    }
    // SAFETY: CreateDIBSection 成功后 bits 指向 buffer×buffer 个 32 位像素。
    let pixels =
        unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), (buffer * buffer) as usize) };
    pixels.fill(pixel(background));
    let grid = Grid {
        origin: (0, 0),
        scale: buffer as f32 / ICON_AREA_DP as f32,
    };
    let stroke = (stroke_px * SUPERSAMPLE as f32).round().max(1.0) as i32;
    let drawn = draw_shapes(dc, kind, grid, stroke, rgb);
    let result = drawn.then(|| downsample_2x(pixels, size));
    // SAFETY: 先恢复旧位图再释放 DC / 位图，避免删除仍被选中的对象。
    unsafe {
        let _ = SelectObject(dc, old_bitmap);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bitmap.into());
    }
    result
}

/// 离屏 DIB 不可用时直接在目标 DC 上绘制；线宽只能取整，但图标仍完整。
fn draw_direct(hdc: HDC, square: Rect, kind: IconKind, rgb: Rgb, stroke_px: f32) {
    let grid = Grid {
        origin: (square.left, square.top),
        scale: square.width() as f32 / ICON_AREA_DP as f32,
    };
    let stroke = stroke_px.round().max(1.0) as i32;
    let _ = draw_shapes(hdc, kind, grid, stroke, rgb);
}

/// 把细线图形按样式画到 `hdc`；几何画笔带圆头端点 / 连接。
fn draw_shapes(hdc: HDC, kind: IconKind, grid: Grid, stroke: i32, rgb: Rgb) -> bool {
    let brush = LOGBRUSH {
        lbStyle: BS_SOLID,
        lbColor: color(rgb),
        lbHatch: 0,
    };
    // SAFETY: brush 在调用期间有效；返回的画笔由本函数使用后释放。
    let pen = unsafe {
        ExtCreatePen(
            PS_GEOMETRIC | PS_SOLID | PS_ENDCAP_ROUND | PS_JOIN_ROUND,
            stroke.max(1) as u32,
            &brush,
            None,
        )
    };
    if pen.is_invalid() {
        return false;
    }
    // SAFETY: pen 与空画刷只在调用期间选中；结束时恢复旧对象并释放 pen。
    unsafe {
        let old_pen = SelectObject(hdc, pen.into());
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        match kind {
            IconKind::Play => closed(hdc, grid, PLAY_TRIANGLE),
            IconKind::Pause => {
                polyline(hdc, grid, &[(6.1, 3.8), (6.1, 12.2)]);
                polyline(hdc, grid, &[(9.9, 3.8), (9.9, 12.2)]);
            }
            IconKind::Refresh => polyline(hdc, grid, &refresh_path()),
            IconKind::Volume => {
                closed(hdc, grid, SPEAKER);
                for radius in [2.7, 4.5] {
                    polyline(hdc, grid, &arc_points((7.9, 8.0), radius, -50.0, 100.0, 16));
                }
            }
            IconKind::Mute => {
                closed(hdc, grid, SPEAKER);
                polyline(hdc, grid, &[(9.3, 5.0), (13.5, 11.0)]);
            }
            IconKind::Chevron => polyline(hdc, grid, CHEVRON),
        }
        let _ = SelectObject(hdc, old_brush);
        let _ = SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen.into());
    }
    true
}

/// 设计格（16×16）到目标坐标的映射。
#[derive(Debug, Clone, Copy)]
struct Grid {
    origin: (i32, i32),
    scale: f32,
}

impl Grid {
    fn points(self, points: &[(f32, f32)]) -> Vec<POINT> {
        points
            .iter()
            .map(|&(x, y)| POINT {
                x: self.origin.0 + (x * self.scale).round() as i32,
                y: self.origin.1 + (y * self.scale).round() as i32,
            })
            .collect()
    }
}

fn polyline(hdc: HDC, grid: Grid, points: &[(f32, f32)]) {
    let points = grid.points(points);
    // SAFETY: points 在调用期间有效。
    unsafe {
        let _ = Polyline(hdc, &points);
    }
}

fn closed(hdc: HDC, grid: Grid, points: &[(f32, f32)]) {
    let points = grid.points(points);
    // SAFETY: points 在调用期间有效；当前画刷为空画刷，只描边。
    unsafe {
        let _ = Polygon(hdc, &points);
    }
}

/// 自圆弧中心按屏幕角度（0° = 右，顺时针为正）取一段折线。
fn arc_points(
    center: (f32, f32),
    radius: f32,
    start_deg: f32,
    sweep_deg: f32,
    segments: usize,
) -> Vec<(f32, f32)> {
    let segments = segments.max(1);
    (0..=segments)
        .map(|index| {
            let fraction = index as f32 / segments as f32;
            let angle = (start_deg + sweep_deg * fraction).to_radians();
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect()
}

/// 刷新：300° 圆弧 + 与弧端点相连的箭头。
fn refresh_path() -> Vec<(f32, f32)> {
    let mut points = arc_points((8.0, 8.0), 5.1, 0.0, 300.0, 48);
    let tip = *points.last().expect("arc always has points");
    // 300° 处的屏幕顺时针切线方向，箭头沿切线指向。
    let end_angle = 300.0f32.to_radians();
    let tangent = (-end_angle.sin(), end_angle.cos());
    let wing = 2.9;
    let mut wings = [tip; 2];
    for (slot, offset) in wings.iter_mut().zip([35.0f32, -35.0]) {
        let rotated = rotate(tangent, offset);
        *slot = (tip.0 - rotated.0 * wing, tip.1 - rotated.1 * wing);
    }
    // 弧端点 → 一侧箭翼 → 回端点 → 另一侧箭翼：箭头与圆弧连成一体。
    points.push(wings[0]);
    points.push(tip);
    points.push(wings[1]);
    points
}

fn rotate(vector: (f32, f32), degrees: f32) -> (f32, f32) {
    let angle = degrees.to_radians();
    (
        vector.0 * angle.cos() - vector.1 * angle.sin(),
        vector.0 * angle.sin() + vector.1 * angle.cos(),
    )
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // 负高度 = 自上而下，和像素缓冲的行序一致。
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 2×2 平均降采样；`source` 是 width×2 的自上而下 BGRA 像素。
fn downsample_2x(source: &[u32], width: i32) -> Vec<u32> {
    let width = width.max(0) as usize;
    let source_width = width * 2;
    if source.len() < source_width * source_width {
        return Vec::new();
    }
    let mut out = vec![0u32; width * width];
    for y in 0..width {
        for x in 0..width {
            let index = (y * 2) * source_width + x * 2;
            out[y * width + x] = average_2x2([
                source[index],
                source[index + 1],
                source[index + source_width],
                source[index + source_width + 1],
            ]);
        }
    }
    out
}

fn average_2x2(pixels: [u32; 4]) -> u32 {
    let mut blue = 0u32;
    let mut green = 0u32;
    let mut red = 0u32;
    for value in pixels {
        blue += value & 0xFF;
        green += (value >> 8) & 0xFF;
        red += (value >> 16) & 0xFF;
    }
    // +2 做四舍五入（除以 4）。
    ((blue + 2) / 4) | (((green + 2) / 4) << 8) | (((red + 2) / 4) << 16)
}

fn blit(hdc: HDC, left: i32, top: i32, size: i32, pixels: &[u32]) {
    let info = bitmap_info(size, size);
    // SAFETY: pixels 长度 = size×size；BITMAPINFO 描述一致，调用期间有效。
    unsafe {
        let _ = SetDIBitsToDevice(
            hdc,
            left,
            top,
            size as u32,
            size as u32,
            0,
            0,
            0,
            size as u32,
            pixels.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
        );
    }
}

fn color((red, green, blue): Rgb) -> COLORREF {
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

fn pixel((red, green, blue): Rgb) -> u32 {
    u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16)
}

/// 细线三角（只描边，不填充）。
const PLAY_TRIANGLE: &[(f32, f32)] = &[(4.4, 3.4), (4.4, 12.6), (11.6, 8.0)];
/// 喇叭：矩形 + 锥形的一体描边。
const SPEAKER: &[(f32, f32)] = &[
    (2.6, 6.4),
    (4.6, 6.4),
    (7.8, 3.6),
    (7.8, 12.4),
    (4.6, 9.6),
    (2.6, 9.6),
];
/// 下拉 chevron：两段线。
const CHEVRON: &[(f32, f32)] = &[(5.4, 6.4), (8.0, 9.2), (10.6, 6.4)];

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgb = (0xFF, 0xFF, 0xFF);
    const BLACK: Rgb = (0x00, 0x00, 0x00);

    #[test]
    fn codepoints_match_the_contract() {
        assert_eq!(IconKind::Play.code_point(), 0xE768);
        assert_eq!(IconKind::Pause.code_point(), 0xE769);
        assert_eq!(IconKind::Refresh.code_point(), 0xE72C);
        assert_eq!(IconKind::Volume.code_point(), 0xE767);
        assert_eq!(IconKind::Mute.code_point(), 0xE74F);
        assert_eq!(IconKind::Chevron.code_point(), 0xE70D);
        assert_eq!(IconKind::ALL.len(), 6);
    }

    #[test]
    fn style_env_accepts_three_values_and_defaults_to_auto() {
        assert_eq!(Preference::parse(None), Preference::Auto);
        assert_eq!(Preference::parse(Some("auto")), Preference::Auto);
        assert_eq!(Preference::parse(Some(" Fluent ")), Preference::Fluent);
        assert_eq!(Preference::parse(Some("LINE")), Preference::Line);
        for invalid in ["", "mdl2", "1", "true", "fluent-line"] {
            assert_eq!(Preference::parse(Some(invalid)), Preference::Auto);
        }
    }

    #[test]
    fn auto_prefers_fluent_then_mdl2_then_line() {
        assert_eq!(select_style(Preference::Auto, |_| true), IconStyle::Fluent);
        assert_eq!(
            select_style(Preference::Auto, |style| style == IconStyle::Mdl2),
            IconStyle::Mdl2
        );
        assert_eq!(select_style(Preference::Auto, |_| false), IconStyle::Line);
        // 强制 line 时即使字体可用也不碰系统字体。
        assert_eq!(select_style(Preference::Line, |_| true), IconStyle::Line);
    }

    #[test]
    fn forced_fluent_still_falls_back_when_no_icon_font_exists() {
        assert_eq!(select_style(Preference::Fluent, |_| false), IconStyle::Line);
        // 只有 MDL2 时 fluent 仍接受 B 方案的第二选择。
        assert_eq!(
            select_style(Preference::Fluent, |style| style == IconStyle::Mdl2),
            IconStyle::Mdl2
        );
    }

    #[test]
    fn icon_square_is_centered_and_dpi_scaled() {
        assert_eq!(
            icon_square(Rect::new(0, 0, 28, 28), 96),
            Rect::new(6, 6, 22, 22)
        );
        // 125%：按钮 35px、图标 20px，仍居中。
        assert_eq!(
            icon_square(Rect::new(0, 0, 35, 35), 120),
            Rect::new(7, 7, 27, 27)
        );
    }

    #[test]
    fn thin_marks_are_1_5px_at_96_dpi_and_pause_is_2px() {
        assert!((device_stroke_px(IconKind::Play, 96) - 1.5).abs() < f32::EPSILON);
        assert!((device_stroke_px(IconKind::Refresh, 96) - 1.5).abs() < f32::EPSILON);
        assert!((device_stroke_px(IconKind::Pause, 96) - 2.0).abs() < f32::EPSILON);
        assert!((device_stroke_px(IconKind::Play, 192) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn refresh_arc_spans_about_300_degrees() {
        let center = (8.0, 8.0);
        let radius = 5.0;
        let points = arc_points(center, radius, 0.0, 300.0, 30);
        assert_eq!(points.len(), 31);
        let (first, last) = (points[0], points[30]);
        assert!((first.0 - 13.0).abs() < 0.01);
        assert!((first.1 - 8.0).abs() < 0.01);
        // 300° 终点在右上方。
        assert!((last.0 - 10.5).abs() < 0.01);
        assert!((last.1 - (8.0 - radius * 3.0f32.sqrt() / 2.0)).abs() < 0.01);
        for (x, y) in points {
            let distance = ((x - center.0).powi(2) + (y - center.1).powi(2)).sqrt();
            assert!((distance - radius).abs() < 0.01, "点不在圆弧上");
        }
    }

    #[test]
    fn downsampling_averages_2x2_blocks() {
        // 2×2 源 = 两个黑像素 + 两个白像素，1×1 输出的每个通道应是 128。
        let source = [0u32, 0, 0xFF_FFFF, 0xFF_FFFF];
        assert_eq!(downsample_2x(&source, 1), vec![0x80_8080]);
    }

    #[test]
    fn glyph_face_probe_rejects_a_substituted_font() {
        // Windows 字体映射器对不存在的家族会静默替换，`CreateFontW` 仍然成功；
        // 只有 `GetTextFaceW` 的实际名字能证明字体真的存在。
        let phantom = "Nowhere Player Missing Icon Font";
        let font = create_face(phantom, 96);
        assert!(!font.is_invalid(), "CreateFontW 通常会成功替换字体");
        // SAFETY: 探针字体由本测试创建。
        unsafe {
            let _ = DeleteObject(font.into());
        }
        assert!(create_verified(phantom, 96).is_none());
    }

    #[test]
    fn every_line_icon_draws_visible_pixels() {
        for kind in IconKind::ALL {
            let pixels = render_supersampled(kind, 32, device_stroke_px(kind, 96), WHITE, BLACK)
                .expect("离屏 DIB 应可用");
            assert_eq!(pixels.len(), 32 * 32);
            let lit = pixels.iter().filter(|&&pixel| pixel != 0).count();
            assert!(lit > 0, "{kind:?} 没有画出像素");
            assert!(lit < pixels.len() / 2, "{kind:?} 铺得太满，不是细线图标");
            // 白线 + 黑底 + 平均降采样：所有像素都应是灰阶。
            assert!(pixels.iter().all(|pixel| {
                let (blue, green, red) = (pixel & 0xFF, (pixel >> 8) & 0xFF, (pixel >> 16) & 0xFF);
                blue == green && green == red
            }));
        }
    }

    /// 在 28×28 离屏 DIB 上按控制条按钮尺寸画一个图标字体字形。
    fn render_glyph_pixels(face: &str, kind: IconKind) -> Option<Vec<u32>> {
        let font = create_verified(face, 96)?;
        let size = 28;
        let info = bitmap_info(size, size);
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: info 描述 28×28 的 32 位 DIB；bits 由系统写入。
        let bitmap =
            unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        if bits.is_null() {
            // SAFETY: bitmap 由本函数创建。
            unsafe {
                let _ = DeleteObject(bitmap.into());
            }
            return None;
        }
        // SAFETY: 内存 DC 只用于离屏绘制；函数末尾成对释放。
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            // SAFETY: bitmap 尚未选入 DC。
            unsafe {
                let _ = DeleteObject(bitmap.into());
            }
            return None;
        }
        // SAFETY: dc / bitmap 有效。
        let old = unsafe { SelectObject(dc, bitmap.into()) };
        if old.is_invalid() {
            // SAFETY: 选入失败时释放 DC / 位图 / 字体。
            unsafe {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteObject(font.into());
            }
            return None;
        }
        // SAFETY: bits 指向 size×size 个 32 位像素。
        let pixels =
            unsafe { std::slice::from_raw_parts_mut(bits.cast::<u32>(), (size * size) as usize) };
        pixels.fill(0);
        draw_glyph(dc, Rect::new(0, 0, size, size), kind, WHITE, font);
        let out = pixels.to_vec();
        // SAFETY: 先恢复旧位图，再释放 DC / 位图 / 字体。
        unsafe {
            let _ = SelectObject(dc, old);
            let _ = DeleteDC(dc);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteObject(font.into());
        }
        Some(out)
    }

    #[test]
    fn line_rendering_keeps_the_requested_color_channels() {
        // 32 位 DIB 是 BGRA 内存序；这里用纯红线校验通道没有写反。
        let pixels = render_supersampled(IconKind::Play, 32, 1.5, (255, 0, 0), BLACK)
            .expect("离屏 DIB 应可用");
        assert!(
            pixels.iter().any(|&pixel| (pixel >> 16) & 0xFF > 0),
            "红线没有落在 R 通道"
        );
        assert!(
            pixels.iter().all(|&pixel| pixel & 0xFF == 0),
            "B 通道应为 0"
        );
        assert!(
            pixels.iter().all(|&pixel| (pixel >> 8) & 0xFF == 0),
            "G 通道应为 0"
        );
    }

    #[test]
    fn installed_icon_font_draws_every_glyph() {
        let mut found = false;
        for face in ["Segoe Fluent Icons", "Segoe MDL2 Assets"] {
            let Some(font) = create_verified(face, 96) else {
                continue;
            };
            // SAFETY: 探针字体由本测试创建。
            unsafe {
                let _ = DeleteObject(font.into());
            }
            found = true;
            for kind in IconKind::ALL {
                let pixels = render_glyph_pixels(face, kind).expect("离屏 DIB 应可用");
                let lit = pixels.iter().filter(|&&pixel| pixel != 0).count();
                assert!(lit > 0, "{face} 的 {kind:?} 没有画出像素");
            }
        }
        if !found {
            // 没有系统图标字体的机器走细线回退，由上面的线描边测试覆盖。
            eprintln!("未安装 Segoe Fluent Icons / Segoe MDL2 Assets，跳过字体渲染测试");
        }
    }
}
