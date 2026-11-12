use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(crate) fn player_height_for(inner_height: u16) -> u16 {
    let proportional = inner_height.saturating_mul(40) / 100;
    proportional.clamp(8, 14).min(inner_height)
}

pub(crate) fn vertical_heights(inner_height: u16) -> (u16, u16, u16, u16) {
    let player = player_height_for(inner_height);
    let controller = 3.min(inner_height.saturating_sub(1));
    let top = inner_height.saturating_sub(player + controller + 1).max(1);
    (top, player, controller, 1)
}

pub(crate) fn layout_inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(crate) fn layout_areas(area: Rect) -> [Rect; 4] {
    let inner = layout_inner(area);
    let (top_h, player_h, controller_h, status_h) = vertical_heights(inner.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_h),
            Constraint::Length(player_h),
            Constraint::Length(controller_h),
            Constraint::Length(status_h),
        ])
        .split(inner);
    [rows[0], rows[1], rows[2], rows[3]]
}
