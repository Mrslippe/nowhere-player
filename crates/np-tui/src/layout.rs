use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// 主区域高度：Controller 固定 3 行，top/Player 按设计稿 9:10 分配。
/// 状态栏嵌在外框下边线，不占主区域。
pub(crate) fn main_heights(inner_height: u16) -> (u16, u16, u16) {
    let controller = 3.min(inner_height);
    let remaining = inner_height.saturating_sub(controller);
    let top = rounded_ratio(remaining, 9, 19);
    let player = remaining.saturating_sub(top);
    (top, player, controller)
}

fn rounded_ratio(value: u16, numerator: u16, denominator: u16) -> u16 {
    let scaled = u32::from(value) * u32::from(numerator);
    let rounded = (scaled + u32::from(denominator) / 2) / u32::from(denominator);
    u16::try_from(rounded).unwrap_or(u16::MAX)
}

#[cfg(test)]
pub(crate) fn vertical_heights(inner_height: u16) -> (u16, u16, u16) {
    main_heights(inner_height)
}

pub(crate) fn layout_inner(area: Rect) -> Rect {
    // 外框左右各 1 格，工作区再左右各留 1 格；纵向只去掉外框。
    Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    }
}

pub(crate) fn layout_areas(area: Rect) -> [Rect; 3] {
    let inner = layout_inner(area);
    let (top_h, player_h, controller_h) = main_heights(inner.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_h),
            Constraint::Length(player_h),
            Constraint::Length(controller_h),
        ])
        .split(inner);
    [rows[0], rows[1], rows[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn designed_ratios_match_72x24_and_larger_windows() {
        assert_eq!(main_heights(22), (9, 10, 3));
        assert_eq!(main_heights(28), (12, 13, 3));
        assert_eq!(main_heights(38), (17, 18, 3));
    }

    #[test]
    fn areas_fit_inside_outer_border() {
        let outer = Rect::new(0, 0, 72, 24);
        let [top, player, controller] = layout_areas(outer);
        assert_eq!(top.height + player.height + controller.height, 22);
        assert_eq!(top.y, 1);
        assert_eq!(top.x, 2);
        assert_eq!(top.width, 68);
        assert_eq!(player.x, 2);
        assert_eq!(player.width, 68);
        assert_eq!(controller.bottom(), 23);
    }
}
