pub(crate) fn player_height_for(area_height: u16) -> u16 {
    6.min(area_height.saturating_sub(3).max(3))
}
