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
