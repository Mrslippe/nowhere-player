//! 纯几何 / 命中 / 悬浮状态。
//!
//! 这里不碰 Win32：标题栏高度、按钮 Rect、hit-test 和 hover 状态机都可以在
//! 无窗口的单元测试里验证。两条硬约束在类型层面也是固定的：
//!
//! 1. 隐藏后的热区高度就是 [`titlebar_height`]（32px @ 96 DPI），不是 4px；
//! 2. [`video_rect`] 只由客户区尺寸决定，没有“标题栏是否可见”参数，显示 /
//!    隐藏标题栏不会改变视频子窗口的 Rect。

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMetrics {
    pub x: i32,
    pub y: i32,
}

/// 普通状态下的客户区：只去掉 caption，保留可缩放边框。
pub fn normal_client_rect(window: Rect, frame: FrameMetrics) -> Rect {
    Rect::new(
        window.left + frame.x,
        window.top + frame.y,
        window.right - frame.x,
        window.bottom - frame.y,
    )
}

/// 最大化客户区：直接使用显示器工作区，避免 DWM 最大化边框把内容顶出屏幕。
pub fn maximized_client_rect(work: Rect) -> Rect {
    work
}

/// 视频子窗口始终占满客户区；显示 / 隐藏标题栏都不改变它。
pub fn video_rect(width: i32, height: i32) -> Rect {
    Rect::new(0, 0, width.max(0), height.max(0))
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
    fn normal_client_keeps_the_native_resize_frame() {
        let window = Rect::new(100, 100, 900, 700);
        let client = normal_client_rect(window, FrameMetrics { x: 8, y: 8 });
        assert_eq!(client, Rect::new(108, 108, 892, 692));
        // caption 被去掉：客户区顶部只让出边框，不让出标题栏。
        assert_eq!(client.top - window.top, 8);
    }

    #[test]
    fn maximized_client_is_the_monitor_work_area() {
        let work = Rect::new(0, 0, 2560, 1400);
        assert_eq!(maximized_client_rect(work), work);
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
