//! 纯几何 / 命中 / 悬浮状态。
//!
//! 这里不碰 Win32：标题栏高度、按钮 Rect、hit-test 和 hover 状态机都可以在
//! 无窗口的单元测试里验证。三条硬约束在类型层面也是固定的：
//!
//! 1. 隐藏后的热区高度就是 [`titlebar_height`]（32px @ 96 DPI），不是 4px；
//! 2. [`video_rect`] 只由客户区尺寸决定，没有“标题栏是否可见”参数，显示 /
//!    隐藏标题栏不会改变视频子窗口的 Rect；
//! 3. 控制条隐藏时只有底部 40dp 的 [`control_hotzone_rect`] 能呼出，鼠标在
//!    窗口中间不再让控制条常驻。

use std::time::{Duration, Instant};

/// 标题栏高度（96 DPI 下的逻辑像素）。
pub const TITLEBAR_HEIGHT_DP: i32 = 32;
/// 单个标题栏按钮宽度（96 DPI 下的逻辑像素）。
pub const BUTTON_WIDTH_DP: i32 = 46;
/// 鼠标离开标题栏 / 窗口后延时收起。
pub const HIDE_DELAY: Duration = Duration::from_millis(500);
/// 窗口最小尺寸（逻辑像素）。Snap Layouts 建议最小宽度不超过 500 epx。
pub const MIN_WIDTH_DP: i32 = 400;
pub const MIN_HEIGHT_DP: i32 = 225;
/// 首次打开的默认尺寸（逻辑像素）。
pub const INITIAL_WIDTH_DP: i32 = 960;
pub const INITIAL_HEIGHT_DP: i32 = 540;

/// 相对 96 DPI 缩放并四舍五入。
pub fn scale_dp(dp: i32, dpi: u32) -> i32 {
    let dpi = i64::from(if dpi == 0 { 96 } else { dpi });
    let scaled = (i64::from(dp) * dpi + 48) / 96;
    scaled as i32
}

/// 标题栏高度；隐藏后的顶部热区必须复用同一个值。
pub fn titlebar_height(dpi: u32) -> i32 {
    scale_dp(TITLEBAR_HEIGHT_DP, dpi).max(1)
}

/// 标题栏按钮宽度。
pub fn button_width(dpi: u32) -> i32 {
    scale_dp(BUTTON_WIDTH_DP, dpi).max(1)
}

/// 客户区矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(self) -> i32 {
        self.right - self.left
    }

    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// 窗口框（非客户区）在四个方向上的厚度，通常来自
/// `SM_CXSIZEFRAME + SM_CXPADDEDBORDER`。
///
/// 客户区不再按它内缩；它只用于 `WM_NCHITTEST` 的 resize 命中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMetrics {
    pub x: i32,
    pub y: i32,
}

/// 普通状态下的客户区：**整个窗口矩形**。
///
/// caption 与可缩放边框都不再占客户区：DWM 阴影由
/// `DwmExtendFrameIntoClientArea` 托住，边缘 resize 交给
/// [`resize_edge`] + `WM_NCHITTEST`。
pub fn normal_client_rect(window: Rect) -> Rect {
    window
}

/// 最大化客户区：直接使用显示器工作区，避免 DWM 最大化边框把内容顶出屏幕。
pub fn maximized_client_rect(work: Rect) -> Rect {
    work
}

/// 客户区边缘的 resize 命中结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// 客户区坐标是否落在可缩放边缘内。
///
/// `border` 是每侧厚度（通常为 `SM_CXSIZEFRAME + SM_CXPADDEDBORDER`）；
/// 最大化时传 0，结果是 `None`——最大化窗口没有可拖拽边缘。
pub fn resize_edge(x: i32, y: i32, width: i32, height: i32, border: i32) -> Option<ResizeEdge> {
    let border = border.max(0);
    if border == 0 || width <= 0 || height <= 0 {
        return None;
    }
    let left = x < border;
    let right = x >= width - border;
    let top = y < border;
    let bottom = y >= height - border;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

/// 视频子窗口始终占满客户区；显示 / 隐藏标题栏都不改变它。
pub fn video_rect(width: i32, height: i32) -> Rect {
    Rect::new(0, 0, width.max(0), height.max(0))
}

/// 16:9 窗口比例约束。
///
/// `edge` 使用 Win32 `WM_SIZING` 的 WMSZ_* 数值：
/// 1=左、2=右、3=上、4=左上、5=右上、6=下、7=左下、8=右下。
///
/// 水平拖动以宽度为准调高度；垂直拖动以高度为准调宽度；四角也走宽度基准。
/// 对边保持不动，避免窗口在拖动时整体漂移。
pub fn constrain_aspect(mut rect: Rect, edge: usize, min_w: i32, min_h: i32) -> Rect {
    let min_w = min_w.max(1);
    let min_h = min_h.max(1);
    let top_edge = matches!(edge, 3..=5);
    let vertical = matches!(edge, 3 | 6);
    let left_edge = matches!(edge, 1 | 4 | 7);

    if vertical {
        let mut height = rect.height().max(min_h);
        let mut width = ((height as i64 * 16 + 8) / 9) as i32;
        if width < min_w {
            width = min_w;
            height = ((width as i64 * 9 + 8) / 16) as i32;
        }
        if left_edge {
            rect.left = rect.right - width;
        } else {
            rect.right = rect.left + width;
        }
        if top_edge {
            rect.top = rect.bottom - height;
        } else {
            rect.bottom = rect.top + height;
        }
    } else {
        let mut width = rect.width().max(min_w);
        let mut height = ((width as i64 * 9 + 8) / 16) as i32;
        if height < min_h {
            height = min_h;
            width = ((height as i64 * 16 + 8) / 9) as i32;
        }
        if left_edge {
            rect.left = rect.right - width;
        } else {
            rect.right = rect.left + width;
        }
        if top_edge {
            rect.top = rect.bottom - height;
        } else {
            rect.bottom = rect.top + height;
        }
    }
    rect
}

/// 标题栏 overlay 的 Rect：只有顶部 32px（按 DPI 缩放）。
pub fn overlay_rect(width: i32, dpi: u32) -> Rect {
    Rect::new(0, 0, width.max(0), titlebar_height(dpi))
}

/// 最小跟踪尺寸。
pub fn min_track_size(dpi: u32) -> (i32, i32) {
    (
        scale_dp(MIN_WIDTH_DP, dpi).max(1),
        scale_dp(MIN_HEIGHT_DP, dpi).max(1),
    )
}

/// 首次打开的窗口 Rect：屏幕中央，最大占屏幕 90%。
pub fn initial_window_rect(screen_width: i32, screen_height: i32, dpi: u32) -> Rect {
    let width = scale_dp(INITIAL_WIDTH_DP, dpi)
        .min((screen_width * 9 / 10).max(1))
        .max(1);
    let height = scale_dp(INITIAL_HEIGHT_DP, dpi)
        .min((screen_height * 9 / 10).max(1))
        .max(1);
    let left = (screen_width - width) / 2;
    let top = (screen_height - height) / 2;
    Rect::new(left, top, left + width, top + height)
}

/// 窗口标题（任务栏与标题栏共用）。
pub fn window_title(room_title: Option<&str>) -> String {
    match room_title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) => format!("Nowhere Player · {title}"),
        None => "Nowhere Player".to_owned(),
    }
}

/// 标题栏命中结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Minimize,
    Maximize,
    Close,
    Caption,
    /// 标题栏隐藏时，顶部 32px 热区（只用于 hover 检测）。
    HotZone,
    Client,
}

impl Hit {
    /// 只有三个按钮需要 hover 高亮。
    pub fn button(self) -> Option<Self> {
        match self {
            Self::Minimize | Self::Maximize | Self::Close => Some(self),
            _ => None,
        }
    }
}

/// 按钮 Rect；宽度过小时三等分，保证不重叠。
pub fn button_rect(width: i32, dpi: u32, hit: Hit) -> Rect {
    let width = width.max(0);
    let bar_height = titlebar_height(dpi);
    let max_button = button_width(dpi);
    let button = max_button.min((width / 3).max(1));
    let offset = match hit {
        Hit::Close => 0,
        Hit::Maximize => button,
        Hit::Minimize => button * 2,
        _ => 0,
    };
    let right = width - offset;
    Rect::new(right - button, 0, right, bar_height)
}

/// 命中测试。
///
/// 标题栏隐藏时返回 [`Hit::HotZone`]，高度与显示时完全一致——这是硬约束 1。
pub fn hit_test(x: i32, y: i32, width: i32, dpi: u32, visible: bool) -> Hit {
    if x < 0 || x >= width || y < 0 || y >= titlebar_height(dpi) {
        return Hit::Client;
    }
    if !visible {
        return Hit::HotZone;
    }
    for button in [Hit::Close, Hit::Maximize, Hit::Minimize] {
        if button_rect(width, dpi, button).contains(x, y) {
            return button;
        }
    }
    Hit::Caption
}

/// 一次 hover 状态更新的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TitlebarUpdate {
    pub visibility_changed: bool,
    pub hover_changed: bool,
}

/// 悬浮标题栏状态机：显示 / 隐藏只影响 overlay，不影响视频 Rect。
#[derive(Debug)]
pub struct TitlebarState {
    visible: bool,
    hovered: Option<Hit>,
    hide_at: Option<Instant>,
    interacting: bool,
}

impl Default for TitlebarState {
    fn default() -> Self {
        Self::new()
    }
}

impl TitlebarState {
    /// 初始隐藏：顶部留出 32px 热区，hover 后才显示。
    pub fn new() -> Self {
        Self {
            visible: false,
            hovered: None,
            hide_at: None,
            interacting: false,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn hovered(&self) -> Option<Hit> {
        self.hovered
    }

    /// 拖动 / 缩放 / 菜单期间不收起标题栏。
    pub fn set_interacting(&mut self, interacting: bool) {
        self.interacting = interacting;
        if interacting {
            self.hide_at = None;
        }
    }

    /// 每个定时器 tick 调用。`cursor` 是客户区坐标；鼠标不在窗口内时传 `None`。
    pub fn update(
        &mut self,
        cursor: Option<(i32, i32)>,
        now: Instant,
        width: i32,
        dpi: u32,
    ) -> TitlebarUpdate {
        let previous_visible = self.visible;
        let previous_hover = self.hovered;

        let in_hot_zone = cursor
            .is_some_and(|(x, y)| x >= 0 && x < width.max(0) && y >= 0 && y < titlebar_height(dpi));

        if in_hot_zone {
            self.hide_at = None;
            self.visible = true;
        } else if self.visible {
            if self.interacting {
                self.hide_at = None;
            } else {
                let deadline = *self.hide_at.get_or_insert(now + HIDE_DELAY);
                if now >= deadline {
                    self.visible = false;
                    self.hide_at = None;
                }
            }
        } else {
            self.hide_at = None;
        }

        self.hovered = match (self.visible, cursor) {
            (true, Some((x, y))) => hit_test(x, y, width, dpi, true).button(),
            _ => None,
        };

        TitlebarUpdate {
            visibility_changed: previous_visible != self.visible,
            hover_changed: previous_hover != self.hovered,
        }
    }
}

/// 底部控制条高度（96 DPI 逻辑像素）。
pub const CONTROL_BAR_HEIGHT_DP: i32 = 40;
/// 控制条距离客户区左右 / 底边的间距。
pub const CONTROL_BAR_MARGIN_DP: i32 = 12;
/// 鼠标离开客户区后控制条的收起延时。
pub const CONTROL_HIDE_DELAY: Duration = Duration::from_millis(2_000);
/// 控制条按钮边长。
const CONTROL_BUTTON_DP: i32 = 28;
/// 音量滑杆宽度。
const CONTROL_SLIDER_DP: i32 = 80;
/// 控制条水平内边距。
const CONTROL_PADDING_DP: i32 = 10;
/// 控件间距。
const CONTROL_GAP_DP: i32 = 6;
/// 清晰度按钮宽度。
const CONTROL_QUALITY_DP: i32 = 56;
/// 右侧 cache 小字宽度。
const CONTROL_CACHE_DP: i32 = 52;

/// 底部控制条热区高度（96 DPI 逻辑像素）：与控制条等高，不是 16px。
pub const CONTROL_HOTZONE_HEIGHT_DP: i32 = CONTROL_BAR_HEIGHT_DP;

/// 控制条高度（按 DPI 缩放）。
pub fn control_bar_height(dpi: u32) -> i32 {
    scale_dp(CONTROL_BAR_HEIGHT_DP, dpi).max(1)
}

/// 底部悬浮控制条的 Rect：盖在视频上，距底边 / 左右各 12px。
///
/// 与 [`video_rect`] 一样只看客户区尺寸：显示 / 隐藏控制条绝不 resize 视频。
pub fn control_bar_rect(width: i32, height: i32, dpi: u32) -> Rect {
    let width = width.max(0);
    let height = height.max(0);
    let margin = scale_dp(CONTROL_BAR_MARGIN_DP, dpi).max(0);
    let left = margin.min(width);
    let right = (width - margin).max(left);
    let bottom = (height - margin).max(0);
    let top = (bottom - control_bar_height(dpi)).max(0);
    Rect::new(left, top, right, bottom)
}

/// 底部控制条热区：窗口最底部 40dp（与控制条等高），横向覆盖整个客户区。
/// 控制条隐藏时只有这块区域能把它呼出。
///
/// 和控制条 / 视频一样，热区只由客户区尺寸与 DPI 决定；显示 / 隐藏 overlay
/// 不改变它。窗口过小时退化成零宽 / 零高，命中自然失效，不会 panic。
pub fn control_hotzone_rect(width: i32, height: i32, dpi: u32) -> Rect {
    let width = width.max(0);
    let client_h = height.max(0);
    let hotzone_height = scale_dp(CONTROL_HOTZONE_HEIGHT_DP, dpi).max(1);
    Rect::new(0, (client_h - hotzone_height).max(0), width, client_h)
}

/// cache 小字最多 2Hz 更新；mpv 的 cache 属性变化频率远高于可见需求。
pub const CACHE_TEXT_INTERVAL: Duration = Duration::from_millis(500);

/// cache 文本是否到了下一次允许刷新的时刻；`None` 表示第一次立即允许。
pub fn cache_text_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= CACHE_TEXT_INTERVAL)
}

/// 控制条 cache 小字在 controls 客户区内的局部矩形；退化时返回 `None`。
pub fn cache_local_rect(width: i32, height: i32, dpi: u32) -> Option<Rect> {
    let layout = control_layout(width, height, dpi);
    let bar = layout.bar;
    let cache = layout.cache;
    if cache.right <= cache.left || cache.bottom <= cache.top {
        return None;
    }
    Some(Rect::new(
        cache.left.saturating_sub(bar.left),
        cache.top.saturating_sub(bar.top),
        cache.right.saturating_sub(bar.left),
        cache.bottom.saturating_sub(bar.top),
    ))
}

/// 控制条内部各控件的 Rect（客户区坐标）。
///
/// 顺序从左到右：播放 / 暂停、刷新、静音、音量滑杆；右侧是清晰度与 cache。
/// 窗口过小时矩形会退化成 0 宽，命中测试自然失效，不会 panic。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlLayout {
    pub bar: Rect,
    pub play_pause: Rect,
    pub refresh: Rect,
    pub mute: Rect,
    pub slider: Rect,
    pub quality: Rect,
    pub cache: Rect,
}

fn control_slot(left: i32, top: i32, width: i32, height: i32, bar: Rect) -> Rect {
    let left = left.clamp(bar.left, bar.right);
    let right = (left + width.max(0)).min(bar.right);
    Rect::new(left, top, right, top + height)
}

/// 计算控制条内部布局；绘制与命中测试共用，保证两边永远一致。
pub fn control_layout(width: i32, height: i32, dpi: u32) -> ControlLayout {
    let bar = control_bar_rect(width, height, dpi);
    let padding = scale_dp(CONTROL_PADDING_DP, dpi).max(2);
    let gap = scale_dp(CONTROL_GAP_DP, dpi).max(1);
    let button = scale_dp(CONTROL_BUTTON_DP, dpi)
        .max(1)
        .min(bar.height().max(1));
    let top = bar.top + (bar.height() - button) / 2;

    let play_pause = control_slot(bar.left + padding, top, button, button, bar);
    let refresh = control_slot(play_pause.right + gap, top, button, button, bar);
    let mute = control_slot(refresh.right + gap, top, button, button, bar);

    let cache_right = (bar.right - padding).max(bar.left);
    let cache_left = (cache_right - scale_dp(CONTROL_CACHE_DP, dpi)).max(bar.left);
    let quality_right = (cache_left - gap).max(bar.left);
    let quality_left = (quality_right - scale_dp(CONTROL_QUALITY_DP, dpi)).max(bar.left);
    let slider_left = mute.right + gap;
    let slider_right =
        (slider_left + scale_dp(CONTROL_SLIDER_DP, dpi)).min((quality_left - gap).max(bar.left));
    let slider = Rect::new(
        slider_left.clamp(bar.left, bar.right),
        top,
        slider_right.max(slider_left).clamp(bar.left, bar.right),
        top + button,
    );

    ControlLayout {
        bar,
        play_pause,
        refresh,
        mute,
        slider,
        quality: Rect::new(quality_left, top, quality_right, top + button),
        cache: Rect::new(cache_left, top, cache_right, top + button),
    }
}

/// 控制条命中结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlHit {
    PlayPause,
    Refresh,
    Mute,
    /// 落在音量滑杆上；具体音量由 [`slider_volume`] 按 x 计算。
    Volume,
    Quality,
}

/// 鼠标是否落在控制条上（用于 hover / 拖动判定）。
pub fn control_bar_contains(x: i32, y: i32, width: i32, height: i32, dpi: u32) -> bool {
    control_bar_rect(width, height, dpi).contains(x, y)
}

/// 控制条命中测试；落在控制条空白处返回 `None`。
pub fn control_hit(x: i32, y: i32, width: i32, height: i32, dpi: u32) -> Option<ControlHit> {
    let layout = control_layout(width, height, dpi);
    if !layout.bar.contains(x, y) {
        return None;
    }
    if layout.play_pause.contains(x, y) {
        Some(ControlHit::PlayPause)
    } else if layout.refresh.contains(x, y) {
        Some(ControlHit::Refresh)
    } else if layout.mute.contains(x, y) {
        Some(ControlHit::Mute)
    } else if layout.slider.contains(x, y) {
        Some(ControlHit::Volume)
    } else if layout.quality.contains(x, y) {
        Some(ControlHit::Quality)
    } else {
        None
    }
}

/// 滑杆 x 坐标对应的音量 0–100；滑杆退化时返回 0。
pub fn slider_volume(x: i32, width: i32, height: i32, dpi: u32) -> u8 {
    let slider = control_layout(width, height, dpi).slider;
    if slider.width() <= 0 {
        return 0;
    }
    let ratio = (f64::from(x - slider.left) / f64::from(slider.width())).clamp(0.0, 1.0);
    (ratio * 100.0).round() as u8
}

/// 一次控制条可见性更新结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlBarUpdate {
    pub visibility_changed: bool,
}

/// 底部控制条状态机：隐藏时只有底部 40dp 热区能呼出；显示后鼠标在控制条
/// 本身或热区内保持，离开这两块区域 2 秒后收起——鼠标仍在窗口中间也一样。
///
/// 拖动滑杆 / 弹出菜单期间 [`Self::set_interacting`] 为 true，不会收起。
#[derive(Debug)]
pub struct ControlBarState {
    visible: bool,
    hide_at: Option<Instant>,
    interacting: bool,
}

impl Default for ControlBarState {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlBarState {
    pub fn new() -> Self {
        Self {
            visible: false,
            hide_at: None,
            interacting: false,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_interacting(&mut self, interacting: bool) {
        self.interacting = interacting;
        if interacting {
            self.hide_at = None;
        }
    }

    /// 每个定时器 tick 调用；`cursor` 是客户区坐标，鼠标不在窗口内时 `None`。
    ///
    /// 隐藏时只认 [`control_hotzone_rect`]；显示时认控制条本体或热区。其余
    /// 位置（包括窗口中间）不阻止 2 秒收起计时。
    pub fn update(
        &mut self,
        cursor: Option<(i32, i32)>,
        now: Instant,
        width: i32,
        height: i32,
        dpi: u32,
    ) -> ControlBarUpdate {
        let previous = self.visible;
        let (on_bar, on_hotzone) = match cursor {
            Some((x, y)) => (
                control_bar_contains(x, y, width, height, dpi),
                control_hotzone_rect(width, height, dpi).contains(x, y),
            ),
            None => (false, false),
        };
        let inside = if self.visible {
            on_bar || on_hotzone
        } else {
            // 控制条隐藏时它并不在屏幕上，只有底部热区能呼出。
            on_hotzone
        };
        if inside {
            self.hide_at = None;
            self.visible = true;
        } else if self.visible {
            if self.interacting {
                self.hide_at = None;
            } else {
                let deadline = *self.hide_at.get_or_insert(now + CONTROL_HIDE_DELAY);
                if now >= deadline {
                    self.visible = false;
                    self.hide_at = None;
                }
            }
        } else {
            self.hide_at = None;
        }
        ControlBarUpdate {
            visibility_changed: previous != self.visible,
        }
    }
}

/// overlay 滑入 / 滑出的缓动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    /// 先快后慢；呼出用。
    EaseOut,
    /// 先慢后快；收起用。
    EaseIn,
}

/// `ease-out-cubic`：起步快、临近终点变慢。输入先夹到 `0..=1`。
pub fn ease_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// `ease-in-cubic`：起步慢、临近终点加快。输入先夹到 `0..=1`。
pub fn ease_in_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t.powi(3)
}

/// overlay 滑入 / 滑出的纯时间状态：只记录 `from` / `to` / 时长 / 起点 /
/// 缓动方向，偏移由 [`Self::offset`] 按 `now` 计算，方便在无窗口的单元测试
/// 里验证插值与结束。
///
/// 这里只算位置，不碰 Win32，也没有 alpha 概念：overlay 始终不透明，滑入
/// 滑出只改变它在客户区里的 y。
#[derive(Debug, Clone, Copy)]
pub struct SlideAnimation {
    from: i32,
    to: i32,
    duration: Duration,
    start: Instant,
    easing: Easing,
}

impl SlideAnimation {
    /// 呼出动画：指定时长、`ease-out`。
    pub fn new(from: i32, to: i32, duration: Duration, now: Instant) -> Self {
        Self::with_easing(from, to, duration, now, Easing::EaseOut)
    }

    /// 指定缓动方向；收起用 [`Easing::EaseIn`]。
    pub fn with_easing(
        from: i32,
        to: i32,
        duration: Duration,
        now: Instant,
        easing: Easing,
    ) -> Self {
        Self {
            from,
            to,
            duration,
            start: now,
            easing,
        }
    }

    /// 当前偏移：起点前保持 `from`，结束后精确等于 `to`，不会过冲。
    pub fn offset(&self, now: Instant) -> i32 {
        if self.finished(now) {
            return self.to;
        }
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed.is_zero() || self.duration.is_zero() {
            return self.from;
        }
        let progress = (elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0);
        let eased = match self.easing {
            Easing::EaseOut => ease_out_cubic(progress),
            Easing::EaseIn => ease_in_cubic(progress),
        };
        let distance = f64::from(self.to) - f64::from(self.from);
        (f64::from(self.from) + distance * eased).round() as i32
    }

    /// 是否已到终点；零时长视为开始即结束。
    pub fn finished(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.duration
    }
}

/// 标题栏隐藏时的 y：整体滑出到客户区上方。
pub fn titlebar_hidden_offset(dpi: u32) -> i32 {
    -titlebar_height(dpi)
}

/// 标题栏显示时的 y：贴在客户区顶部。
pub const fn titlebar_shown_offset() -> i32 {
    0
}

/// 控制条隐藏时的 y：整体滑出到客户区下方。
pub fn controls_hidden_offset(client_h: i32) -> i32 {
    client_h.max(0)
}

/// 控制条显示时的 y：控制条 Rect 的顶边。
pub fn controls_shown_offset(client_w: i32, client_h: i32, dpi: u32) -> i32 {
    control_bar_rect(client_w, client_h, dpi).top
}

/// 一个 overlay 的滑动状态：隐藏 / 显示两个静止偏移，加上进行中的动画。
///
/// 只算几何与时间，不碰 Win32；宿主每 tick 用 [`Self::advance`] 的偏移决定
/// `SetWindowPos`。这样“从当前偏移继续、不跳变”“resize 直接落位”都能在
/// 无窗口的单元测试里验证。
#[derive(Debug, Clone, Copy)]
pub struct OverlaySlide {
    hidden: i32,
    shown: i32,
    animation: Option<SlideAnimation>,
}

impl OverlaySlide {
    pub fn new(hidden: i32, shown: i32) -> Self {
        Self {
            hidden,
            shown,
            animation: None,
        }
    }

    pub fn hidden(&self) -> i32 {
        self.hidden
    }

    pub fn shown(&self) -> i32 {
        self.shown
    }

    /// 换新几何并停掉进行中的动画；返回此前是否有动画（用于判断停在半路）。
    pub fn set_geometry(&mut self, hidden: i32, shown: i32) -> bool {
        self.hidden = hidden;
        self.shown = shown;
        self.animation.take().is_some()
    }

    /// 立即停掉动画（关闭窗口 / 退出时用），保留几何。
    pub fn stop(&mut self) {
        self.animation = None;
    }

    fn current(&self, now: Instant, fallback: i32) -> i32 {
        self.animation.map_or(fallback, |slide| slide.offset(now))
    }

    /// 呼出：从当前偏移续 `duration` 的 `ease-out`；返回起点 y。
    pub fn begin_show(&mut self, now: Instant, duration: Duration) -> i32 {
        let from = self.current(now, self.hidden);
        self.animation = Some(SlideAnimation::new(from, self.shown, duration, now));
        from
    }

    /// 收起：从当前偏移续 `duration` 的 `ease-in`，结束后停在 `hidden`。
    pub fn begin_hide(&mut self, now: Instant, duration: Duration) {
        let from = self.current(now, self.shown);
        self.animation = Some(SlideAnimation::with_easing(
            from,
            self.hidden,
            duration,
            now,
            Easing::EaseIn,
        ));
    }

    /// 推进一帧：`None` = 没有动画；`Some((y, true))` 是最后一帧，随后静止。
    pub fn advance(&mut self, now: Instant) -> Option<(i32, bool)> {
        let slide = self.animation?;
        let offset = slide.offset(now);
        let finished = slide.finished(now);
        if finished {
            self.animation = None;
        }
        Some((offset, finished))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titlebar_is_32px_at_96_dpi_and_scales() {
        assert_eq!(TITLEBAR_HEIGHT_DP, 32);
        assert_eq!(titlebar_height(96), 32);
        assert_eq!(titlebar_height(144), 48);
        assert_eq!(titlebar_height(192), 64);
        assert_eq!(titlebar_height(0), 32, "0 DPI falls back to 96");
    }

    #[test]
    fn hidden_hot_zone_is_the_full_titlebar_height_not_4px() {
        // 硬约束 1：隐藏后热区仍是 32px，不是 4px。
        assert_eq!(hit_test(10, 0, 800, 96, false), Hit::HotZone);
        assert_eq!(hit_test(10, 31, 800, 96, false), Hit::HotZone);
        assert_eq!(hit_test(10, 32, 800, 96, false), Hit::Client);
        assert_eq!(hit_test(10, 3, 800, 96, false), Hit::HotZone);
        assert_ne!(titlebar_height(96), 4);
    }

    #[test]
    fn control_bar_is_40px_above_the_bottom_margin_and_never_moves_the_video() {
        let bar = control_bar_rect(960, 540, 96);
        assert_eq!(bar, Rect::new(12, 488, 948, 528));
        assert_eq!(bar.height(), CONTROL_BAR_HEIGHT_DP);
        assert_eq!(bar.bottom, 540 - 12);
        // 硬约束：控制条出现 / 隐藏都不改变视频 Rect。
        assert_eq!(video_rect(960, 540), Rect::new(0, 0, 960, 540));
        // 高 DPI 按比例缩放。
        assert_eq!(
            control_bar_rect(1440, 810, 144),
            Rect::new(18, 732, 1422, 792)
        );
    }

    #[test]
    fn cache_text_refresh_is_throttled_to_two_hz() {
        let now = Instant::now();
        assert!(cache_text_due(None, now));
        assert!(!cache_text_due(
            Some(now),
            now + CACHE_TEXT_INTERVAL - Duration::from_millis(1)
        ));
        assert!(cache_text_due(Some(now), now + CACHE_TEXT_INTERVAL));
    }

    #[test]
    fn cache_rect_is_local_and_non_degenerate() {
        let layout = control_layout(1280, 720, 96);
        let rect = cache_local_rect(1280, 720, 96).expect("cache rect");
        assert_eq!(rect.left, layout.cache.left - layout.bar.left);
        assert_eq!(rect.top, layout.cache.top - layout.bar.top);
        assert_eq!(rect.right, layout.cache.right - layout.bar.left);
        assert_eq!(rect.bottom, layout.cache.bottom - layout.bar.top);
        assert!(rect.right > rect.left && rect.bottom > rect.top);
        assert_eq!(cache_local_rect(8, 8, 96), None);
    }

    #[test]
    fn tiny_clients_get_a_degenerate_bar_instead_of_panicking() {
        for (width, height) in [(0, 0), (200, 200), (900, 30), (-5, -5)] {
            let bar = control_bar_rect(width, height, 96);
            assert!(bar.width() >= 0 && bar.height() >= 0, "{bar:?}");
            assert!(bar.left >= 0 && bar.top >= 0, "{bar:?}");
            let layout = control_layout(width, height, 96);
            assert!(layout.slider.width() >= 0);
            assert!(layout.quality.width() >= 0);
        }
    }

    #[test]
    fn controls_are_left_to_right_and_right_aligned_at_the_end() {
        let layout = control_layout(960, 540, 96);
        assert_eq!(layout.play_pause.width(), 28);
        assert_eq!(layout.play_pause.left, 22);
        assert_eq!(layout.play_pause.left, layout.bar.left + 10);
        assert!(layout.play_pause.right < layout.refresh.left);
        assert!(layout.refresh.right < layout.mute.left);
        assert!(layout.mute.right < layout.slider.left);
        assert_eq!(layout.slider.width(), 80);
        assert!(layout.slider.right < layout.quality.left);
        assert!(layout.quality.right < layout.cache.left);
        assert_eq!(layout.cache.right, layout.bar.right - 10);
        // 所有控件垂直居中。
        assert_eq!(layout.play_pause.top, layout.bar.top + 6);
        assert_eq!(layout.play_pause.height(), 28);
    }

    #[test]
    fn control_hit_test_covers_every_control_and_nothing_outside() {
        let (width, height, dpi) = (960, 540, 96);
        let layout = control_layout(width, height, dpi);
        let center = |rect: Rect| ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);

        assert_eq!(
            control_hit(
                center(layout.play_pause).0,
                center(layout.play_pause).1,
                width,
                height,
                dpi
            ),
            Some(ControlHit::PlayPause)
        );
        assert_eq!(
            control_hit(
                center(layout.refresh).0,
                center(layout.refresh).1,
                width,
                height,
                dpi
            ),
            Some(ControlHit::Refresh)
        );
        assert_eq!(
            control_hit(
                center(layout.mute).0,
                center(layout.mute).1,
                width,
                height,
                dpi
            ),
            Some(ControlHit::Mute)
        );
        assert_eq!(
            control_hit(
                center(layout.slider).0,
                center(layout.slider).1,
                width,
                height,
                dpi
            ),
            Some(ControlHit::Volume)
        );
        assert_eq!(
            control_hit(
                center(layout.quality).0,
                center(layout.quality).1,
                width,
                height,
                dpi
            ),
            Some(ControlHit::Quality)
        );
        // 控制条之外的视频区域 / 标题栏 / 窗口外都不命中。
        assert_eq!(control_hit(480, 200, width, height, dpi), None);
        assert_eq!(control_hit(480, 10, width, height, dpi), None);
        assert_eq!(control_hit(-1, 500, width, height, dpi), None);
        // 控制条空白处（bar 两端）不命中具体控件。
        assert_eq!(
            control_hit(layout.bar.left + 1, layout.bar.top + 1, width, height, dpi),
            None
        );
        assert!(control_bar_contains(480, 500, width, height, dpi));
        assert!(!control_bar_contains(480, 10, width, height, dpi));
    }

    #[test]
    fn slider_volume_maps_x_to_0_50_100_and_clamps() {
        let layout = control_layout(960, 540, 96);
        let slider = layout.slider;
        assert_eq!(slider_volume(slider.left, 960, 540, 96), 0);
        assert_eq!(
            slider_volume(slider.left + slider.width() / 2, 960, 540, 96),
            50
        );
        assert_eq!(slider_volume(slider.right - 1, 960, 540, 96), 99);
        assert_eq!(slider_volume(slider.left - 40, 960, 540, 96), 0);
        assert_eq!(slider_volume(slider.right + 40, 960, 540, 96), 100);
    }

    #[test]
    fn control_hotzone_is_the_bottom_40dp_band_spanning_the_client() {
        let hotzone = control_hotzone_rect(960, 540, 96);
        // 底部 40dp，横向覆盖整个客户区；左右 12px 边距也属于热区。
        assert_eq!(hotzone, Rect::new(0, 500, 960, 540));
        assert_eq!(hotzone.height(), CONTROL_HOTZONE_HEIGHT_DP);
        assert_eq!(hotzone.height(), control_bar_height(96));
        assert_eq!(hotzone.height(), CONTROL_BAR_HEIGHT_DP);
        assert_ne!(hotzone.height(), 16, "hotzone is 40dp, not the old 16px");
        assert_eq!(hotzone.bottom, 540);
        // 边界是半开区间：顶边算，底边外不算。
        assert!(hotzone.contains(480, 500));
        assert!(!hotzone.contains(480, 499));
        assert!(!hotzone.contains(480, 540));
        assert!(hotzone.contains(5, 520), "left margin is inside");
        assert!(hotzone.contains(955, 520), "right margin is inside");
        // 高 DPI 同比缩放。
        let scaled = control_hotzone_rect(1440, 810, 144);
        assert_eq!(scaled, Rect::new(0, 750, 1440, 810));
        assert_eq!(scaled.height(), control_bar_height(144));
        // 极小 / 退化客户区不 panic。
        for (width, height) in [(0, 0), (200, 200), (-5, -5)] {
            let rect = control_hotzone_rect(width, height, 96);
            assert!(rect.width() >= 0 && rect.height() >= 0, "{rect:?}");
        }
    }

    #[test]
    fn hidden_control_bar_only_answers_the_bottom_hotzone() {
        let now = Instant::now();
        let mut state = ControlBarState::new();
        assert!(!state.is_visible(), "control bar starts hidden");

        // 鼠标在窗口中间：不再显示控制条。
        let update = state.update(Some((480, 300)), now, 960, 540, 96);
        assert!(!update.visibility_changed);
        assert!(!state.is_visible());

        // 窗口下部但不是底部 40dp 热区（这里在控制条 Rect 里）也不显示。
        let update = state.update(Some((480, 495)), now, 960, 540, 96);
        assert!(!update.visibility_changed);
        assert!(!state.is_visible());

        // 进入底部热区才显示；热区底边最后一个像素也算。
        let update = state.update(Some((480, 539)), now, 960, 540, 96);
        assert!(update.visibility_changed);
        assert!(state.is_visible());
    }

    #[test]
    fn shown_control_bar_holds_on_bar_or_hotzone_and_hides_two_seconds_after_middle() {
        let now = Instant::now();
        let mut state = ControlBarState::new();
        state.update(Some((480, 520)), now, 960, 540, 96);
        assert!(state.is_visible());

        // 控制条本体（bar.top..hotzone.top）保持显示。
        state.update(Some((480, 495)), now + Duration::from_secs(5), 960, 540, 96);
        assert!(state.is_visible());

        // 底部热区（含控制条下方 12dp 边距）也保持。
        state.update(
            Some((480, 530)),
            now + Duration::from_secs(10),
            960,
            540,
            96,
        );
        assert!(state.is_visible());

        // 移到窗口中间：仍在窗口内，但 2 秒后收起。
        let left_at = now + Duration::from_secs(11);
        let update = state.update(Some((480, 300)), left_at, 960, 540, 96);
        assert!(!update.visibility_changed);
        assert!(state.is_visible());
        let update = state.update(
            Some((480, 300)),
            left_at + CONTROL_HIDE_DELAY - Duration::from_millis(1),
            960,
            540,
            96,
        );
        assert!(!update.visibility_changed);
        assert!(state.is_visible(), "still visible before the 2s delay");
        let update = state.update(Some((480, 300)), left_at + CONTROL_HIDE_DELAY, 960, 540, 96);
        assert!(update.visibility_changed);
        assert!(!state.is_visible());
    }

    #[test]
    fn interacting_or_hovering_the_bar_stops_the_hide_timer() {
        let now = Instant::now();
        let mut state = ControlBarState::new();
        state.update(Some((480, 500)), now, 960, 540, 96);
        // 鼠标在控制条上始终显示（即使停留很久）。
        state.update(
            Some((480, 500)),
            now + Duration::from_secs(30),
            960,
            540,
            96,
        );
        assert!(state.is_visible());

        // 拖动滑杆时鼠标移出窗口也不收起。
        state.set_interacting(true);
        state.update(None, now + Duration::from_secs(31), 960, 540, 96);
        assert!(state.is_visible());
        state.update(None, now + Duration::from_secs(60), 960, 540, 96);
        assert!(state.is_visible());

        state.set_interacting(false);
        state.update(None, now + Duration::from_secs(61), 960, 540, 96);
        assert!(state.is_visible(), "hide delay restarts after interaction");
        state.update(
            None,
            now + Duration::from_secs(61) + CONTROL_HIDE_DELAY,
            960,
            540,
            96,
        );
        assert!(!state.is_visible());
    }

    #[test]
    fn slide_animation_interpolates_from_start_to_end() {
        let now = Instant::now();
        let duration = Duration::from_millis(160);
        let slide = SlideAnimation::new(-32, 0, duration, now);
        assert_eq!(slide.offset(now), -32, "starts at from");
        assert!(!slide.finished(now));
        assert!(slide.offset(now + duration / 2) > -32);
        assert!(slide.offset(now + duration / 2) < 0);
        assert_eq!(slide.offset(now + duration), 0, "ends exactly at to");
        assert!(slide.finished(now + duration));
        assert!(slide.finished(now + duration + Duration::from_millis(1)));
        assert_eq!(
            slide.offset(now + duration + Duration::from_millis(50)),
            0,
            "never overshoots the target"
        );
    }

    #[test]
    fn slide_animation_eases_out_on_show_and_in_on_hide() {
        let now = Instant::now();
        let duration = Duration::from_millis(160);
        let show = SlideAnimation::new(-32, 0, duration, now);
        // ease-out：一半时间已走过超过一半路程（-32..0 的中点本来是 -16）。
        let half = show.offset(now + duration / 2);
        assert!(half > -16, "ease-out should be past halfway: {half}");
        assert!(half < 0);
        let quarter = show.offset(now + duration / 4);
        assert!(quarter > -32 && quarter < half, "starts fast: {quarter}");

        let hide =
            SlideAnimation::with_easing(0, -32, Duration::from_millis(140), now, Easing::EaseIn);
        // ease-in：一半时间只走了不到一半路程。
        let half = hide.offset(now + Duration::from_millis(70));
        assert!(half > -16, "ease-in should lag at halfway: {half}");
        assert!(half < 0);
        // 反向：位置随时间单调变小，最后精确到终点。
        assert!(hide.offset(now + Duration::from_millis(10)) > half);
        assert_eq!(hide.offset(now + Duration::from_millis(140)), -32);
    }

    #[test]
    fn slide_animation_restarts_from_the_current_offset_without_jumping() {
        let now = Instant::now();
        let duration = Duration::from_millis(160);
        let show = SlideAnimation::new(-32, 0, duration, now);
        let interrupted_at = now + Duration::from_millis(40);
        let current = show.offset(interrupted_at);
        assert!(current > -32);

        // 收起途中再次呼出：新动画从当前偏移起步，第一帧不跳变。
        let retry = SlideAnimation::new(current, 0, Duration::from_millis(160), interrupted_at);
        assert_eq!(retry.offset(interrupted_at), current);
        assert_eq!(retry.offset(interrupted_at), show.offset(interrupted_at));

        // 呼出途中反向收起：同样从当前偏移开始。
        let reverse = SlideAnimation::with_easing(
            current,
            -32,
            Duration::from_millis(140),
            interrupted_at,
            Easing::EaseIn,
        );
        assert_eq!(reverse.offset(interrupted_at), current);
    }

    #[test]
    fn slide_animation_handles_zero_duration_and_reverse_direction() {
        let now = Instant::now();
        // 零时长：开始即结束，直接落到终点。
        let instant = SlideAnimation::new(10, 20, Duration::ZERO, now);
        assert!(instant.finished(now));
        assert_eq!(instant.offset(now), 20);

        // 控制条方向：从 client_h 往上滑到 bar.top。
        let slide = SlideAnimation::new(540, 488, Duration::from_millis(160), now);
        let a = slide.offset(now + Duration::from_millis(20));
        let b = slide.offset(now + Duration::from_millis(60));
        assert!(a > b && b > 488, "{a} > {b} > 488");
        assert_eq!(slide.offset(now + Duration::from_millis(160)), 488);
    }

    #[test]
    fn overlay_slide_glides_from_hidden_to_shown_and_back() {
        let now = Instant::now();
        let mut slide = OverlaySlide::new(540, 488);
        assert_eq!(slide.hidden(), 540);
        assert_eq!(slide.shown(), 488);
        // 隐藏静止时没有动画。
        assert_eq!(slide.advance(now), None);

        // 呼出：起点在 hidden，160ms 后落到 shown。
        let from = slide.begin_show(now, Duration::from_millis(160));
        assert_eq!(from, 540);
        let (y, finished) = slide.advance(now).expect("just started");
        assert!(!finished);
        assert_eq!(y, 540);
        let (y, finished) = slide
            .advance(now + Duration::from_millis(160))
            .expect("last frame");
        assert!(finished);
        assert_eq!(y, 488);
        assert_eq!(
            slide.advance(now + Duration::from_millis(161)),
            None,
            "animation cleared after the last frame"
        );

        // 收起：从 shown 滑回 hidden，140ms 后结束。
        slide.begin_hide(now, Duration::from_millis(140));
        let (y, finished) = slide.advance(now).expect("moving");
        assert!(!finished);
        assert_eq!(y, 488);
        let (y, finished) = slide
            .advance(now + Duration::from_millis(140))
            .expect("last frame");
        assert!(finished);
        assert_eq!(y, 540);
    }

    #[test]
    fn overlay_slide_reverses_from_the_current_offset_without_jumping() {
        let now = Instant::now();
        let mut slide = OverlaySlide::new(-32, 0);
        slide.begin_show(now, Duration::from_millis(160));
        let interrupted = now + Duration::from_millis(40);
        let current = slide.advance(interrupted).expect("moving").0;
        assert!(current > -32 && current < 0, "mid slide: {current}");

        // 收起途中再次呼出：新动画从当前偏移出发，第一帧不跳变。
        let from = slide.begin_show(interrupted, Duration::from_millis(160));
        assert_eq!(from, current);

        // 呼出途中反向收起：同样从当前偏移出发，最终回到 hidden。
        slide.begin_hide(interrupted, Duration::from_millis(140));
        let (y, finished) = slide.advance(interrupted).expect("moving");
        assert!(!finished);
        assert_eq!(y, current);
        let (y, finished) = slide
            .advance(interrupted + Duration::from_millis(140))
            .expect("last frame");
        assert!(finished);
        assert_eq!(y, -32);
    }

    #[test]
    fn overlay_slide_geometry_change_stops_animation_and_uses_the_new_geometry() {
        let now = Instant::now();
        let mut slide = OverlaySlide::new(0, 0);
        assert!(!slide.set_geometry(10, 20), "idle slide");
        slide.begin_show(now, Duration::from_millis(160));
        assert!(slide.set_geometry(30, 40), "moving slide is stopped");
        assert_eq!(slide.advance(now), None, "animation cleared");
        assert_eq!(slide.hidden(), 30);
        assert_eq!(slide.shown(), 40);
        // 下一次呼出直接使用新几何。
        assert_eq!(
            slide.begin_show(now, Duration::from_millis(160)),
            30,
            "new hidden offset is the slide start"
        );
    }

    #[test]
    fn overlay_offsets_match_the_window_geometry() {
        let (width, height, dpi) = (960, 540, 96);
        assert_eq!(titlebar_hidden_offset(dpi), -titlebar_height(dpi));
        assert_eq!(titlebar_shown_offset(), 0);
        assert_eq!(controls_hidden_offset(height), height);
        assert_eq!(
            controls_shown_offset(width, height, dpi),
            control_bar_rect(width, height, dpi).top
        );
        assert_eq!(controls_shown_offset(width, height, dpi), 488);
        // 高 DPI 与退化客户区都不 panic。
        assert_eq!(titlebar_hidden_offset(144), -48);
        assert_eq!(titlebar_hidden_offset(0), -32);
        assert_eq!(controls_hidden_offset(-5), 0);
    }

    #[test]
    fn cubic_easing_endpoints_midpoints_and_clamping_are_stable() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-9);
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert_eq!(ease_in_cubic(1.0), 1.0);
        assert!((ease_in_cubic(0.5) - 0.125).abs() < 1e-9);
        // 越界输入被夹紧，不会让 overlay 飞出客户区。
        assert_eq!(ease_out_cubic(-1.0), 0.0);
        assert_eq!(ease_in_cubic(2.0), 1.0);
    }

    #[test]
    fn video_rect_is_the_full_client_for_both_titlebar_states() {
        // 硬约束 2：video_rect 只由客户区尺寸决定；标题栏状态不参与。
        let now = Instant::now();
        let full = Rect::new(0, 0, 800, 600);
        let mut state = TitlebarState::new();

        state.update(Some((10, 10)), now, 800, 96);
        assert!(state.is_visible());
        assert_eq!(video_rect(800, 600), full);

        // 第一次离开只启动收起计时；超过延时后再 tick 才真正隐藏。
        state.update(Some((10, 200)), now + Duration::from_millis(1), 800, 96);
        assert!(state.is_visible());
        state.update(
            Some((10, 200)),
            now + Duration::from_millis(1) + HIDE_DELAY + Duration::from_millis(1),
            800,
            96,
        );
        assert!(!state.is_visible());
        assert_eq!(video_rect(800, 600), full);
        assert_eq!(video_rect(0, 0), Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn overlay_covers_only_the_top_32px() {
        assert_eq!(overlay_rect(800, 96), Rect::new(0, 0, 800, 32));
        assert_eq!(overlay_rect(1200, 144), Rect::new(0, 0, 1200, 48));
    }

    #[test]
    fn buttons_are_right_aligned_and_contiguous() {
        let width = 800;
        let close = button_rect(width, 96, Hit::Close);
        let maximize = button_rect(width, 96, Hit::Maximize);
        let minimize = button_rect(width, 96, Hit::Minimize);
        assert_eq!(close.right, width);
        assert_eq!(close.left, width - 46);
        assert_eq!(maximize.right, close.left);
        assert_eq!(minimize.right, maximize.left);
        assert_eq!(minimize.left, width - 46 * 3);
        assert_eq!(close.width(), 46);
        assert_eq!(close.height(), 32);
    }

    #[test]
    fn hit_test_prefers_buttons_then_caption_then_client() {
        let width = 800;
        for (hit, x) in [
            (Hit::Close, 780),
            (Hit::Maximize, 734),
            (Hit::Minimize, 688),
        ] {
            assert_eq!(hit_test(x, 16, width, 96, true), hit);
            assert_eq!(hit_test(x, 16, width, 96, false), Hit::HotZone);
        }
        assert_eq!(hit_test(20, 16, width, 96, true), Hit::Caption);
        assert_eq!(hit_test(20, 32, width, 96, true), Hit::Client);
        assert_eq!(hit_test(-1, 16, width, 96, true), Hit::Client);
    }

    #[test]
    fn tiny_windows_shrink_buttons_without_overlap() {
        let width = 90;
        let close = button_rect(width, 96, Hit::Close);
        let maximize = button_rect(width, 96, Hit::Maximize);
        let minimize = button_rect(width, 96, Hit::Minimize);
        assert_eq!(close.width(), 30);
        assert_eq!(close.left, 60);
        assert_eq!(maximize.left, 30);
        assert_eq!(minimize.left, 0);
        assert_eq!(hit_test(75, 10, width, 96, true), Hit::Close);
    }

    #[test]
    fn hover_shows_immediately_and_hides_after_delay() {
        let now = Instant::now();
        let mut state = TitlebarState::new();
        assert!(!state.is_visible());

        let update = state.update(Some((100, 10)), now, 800, 96);
        assert!(update.visibility_changed);
        assert!(state.is_visible());

        // 移到视频区域：先进入延时，不立刻收起。
        state.update(Some((100, 200)), now + Duration::from_millis(50), 800, 96);
        assert!(state.is_visible());

        let update = state.update(
            Some((100, 200)),
            now + Duration::from_millis(50) + HIDE_DELAY + Duration::from_millis(1),
            800,
            96,
        );
        assert!(update.visibility_changed);
        assert!(!state.is_visible());
    }

    #[test]
    fn interacting_blocks_hiding() {
        let now = Instant::now();
        let mut state = TitlebarState::new();
        state.update(Some((100, 10)), now, 800, 96);
        state.set_interacting(true);
        // 10 秒后鼠标也不在标题栏，但拖动 / 缩放开着就不收。
        state.update(Some((100, 200)), now + Duration::from_secs(10), 800, 96);
        assert!(state.is_visible());

        state.set_interacting(false);
        state.update(Some((100, 200)), now + Duration::from_secs(11), 800, 96);
        assert!(state.is_visible(), "hide delay restarts after interaction");
        state.update(
            Some((100, 200)),
            now + Duration::from_secs(11) + HIDE_DELAY + Duration::from_millis(1),
            800,
            96,
        );
        assert!(!state.is_visible());
    }

    #[test]
    fn cursor_outside_window_hides_after_delay() {
        let now = Instant::now();
        let mut state = TitlebarState::new();
        state.update(Some((100, 10)), now, 800, 96);
        state.update(None, now + Duration::from_millis(10), 800, 96);
        assert!(state.is_visible());
        state.update(
            None,
            now + Duration::from_millis(10) + HIDE_DELAY + Duration::from_millis(1),
            800,
            96,
        );
        assert!(!state.is_visible());
    }

    #[test]
    fn hovered_button_follows_the_cursor() {
        let now = Instant::now();
        let mut state = TitlebarState::new();
        state.update(Some((780, 10)), now, 800, 96);
        assert_eq!(state.hovered(), Some(Hit::Close));
        let update = state.update(Some((734, 10)), now, 800, 96);
        assert!(update.hover_changed);
        assert_eq!(state.hovered(), Some(Hit::Maximize));
        state.update(Some((20, 10)), now, 800, 96);
        assert_eq!(state.hovered(), None);
    }

    #[test]
    fn normal_client_is_the_whole_window_rect() {
        // T-0106：不再按 frame.x/frame.y 内缩，否则顶部会漏出 8px 原生 caption。
        let window = Rect::new(100, 100, 900, 700);
        assert_eq!(normal_client_rect(window), window);
        assert_eq!(normal_client_rect(window).width(), 800);
        assert_eq!(normal_client_rect(window).height(), 600);
        // FrameMetrics 仍用于 hit test，但不再参与客户区计算。
        let frame = FrameMetrics { x: 8, y: 8 };
        assert_eq!(frame.x + frame.y, 16);
    }

    #[test]
    fn maximized_client_is_the_monitor_work_area() {
        let work = Rect::new(0, 0, 2560, 1400);
        assert_eq!(maximized_client_rect(work), work);
    }

    #[test]
    fn resize_edge_hits_all_eight_borders_and_corners() {
        let (width, height, border) = (800, 600, 8);
        assert_eq!(
            resize_edge(0, 300, width, height, border),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge(799, 300, width, height, border),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge(400, 0, width, height, border),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(400, 599, width, height, border),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            resize_edge(0, 0, width, height, border),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(799, 0, width, height, border),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            resize_edge(0, 599, width, height, border),
            Some(ResizeEdge::BottomLeft)
        );
        assert_eq!(
            resize_edge(799, 599, width, height, border),
            Some(ResizeEdge::BottomRight)
        );
        // 边界是半开区间：第 8 个像素起算客户区。
        assert_eq!(resize_edge(8, 300, width, height, border), None);
        assert_eq!(resize_edge(400, 8, width, height, border), None);
        assert_eq!(resize_edge(400, 300, width, height, border), None);
    }

    #[test]
    fn maximized_windows_have_no_resize_edges() {
        // 最大化时 border=0；边缘不再返回 resize 命中。
        assert_eq!(resize_edge(0, 0, 2560, 1400, 0), None);
        assert_eq!(resize_edge(0, 700, 2560, 1400, 0), None);
        // 退化尺寸不 panic。
        assert_eq!(resize_edge(0, 0, 0, 0, 8), None);
    }

    #[test]
    fn initial_window_is_centered_and_clamped() {
        let rect = initial_window_rect(1920, 1080, 96);
        assert_eq!(rect.width(), 960);
        assert_eq!(rect.height(), 540);
        assert_eq!(rect.left, 480);
        assert_eq!(rect.top, 270);

        let small = initial_window_rect(1024, 600, 96);
        assert!(small.width() <= 1024);
        assert!(small.height() <= 540);
        assert!(small.left >= 0 && small.top >= 0);
    }

    #[test]
    fn min_track_size_fits_snap_layouts() {
        let (width, height) = min_track_size(96);
        assert_eq!((width, height), (400, 225));
        assert!(width <= 500, "Snap Layouts needs a small minimum width");
        assert_eq!(min_track_size(144), (600, 338));
    }

    #[test]
    fn window_title_uses_the_room_title() {
        assert_eq!(
            window_title(Some("BLAST 直播")),
            "Nowhere Player · BLAST 直播"
        );
        assert_eq!(window_title(Some("   ")), "Nowhere Player");
        assert_eq!(window_title(None), "Nowhere Player");
    }
}
