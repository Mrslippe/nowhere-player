//! 播放控制按钮的唯一可用性来源：渲染灰显和输入执行都查这里。

use crate::audio::TuiAudio;
use crate::keymap::Action;

#[derive(Clone, Copy)]
pub(crate) struct TransportAvailability {
    pub(crate) playing: bool,
    pub(crate) can_prev: bool,
    pub(crate) can_next: bool,
    pub(crate) can_back: bool,
    pub(crate) can_forward: bool,
}

impl TransportAvailability {
    pub(crate) fn from_player(player: &TuiAudio) -> Self {
        let live = player.is_live();
        Self {
            playing: player.is_playing(),
            can_prev: !live && player.can_prev(),
            can_next: !live && player.can_next(),
            can_back: !live && player.can_seek_back(),
            can_forward: !live && player.can_seek_forward(),
        }
    }

    pub(crate) fn enabled(self, action: Action) -> bool {
        match action {
            Action::PrevTrack => self.can_prev,
            Action::NextTrack => self.can_next,
            Action::SeekBack => self.can_back,
            Action::SeekForward => self.can_forward,
            Action::TogglePlay => true,
            _ => false,
        }
    }
}
