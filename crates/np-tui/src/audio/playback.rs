//! 队列、本地播放与 `PlayerApi`。

use std::path::{Path, PathBuf};
use std::time::Instant;

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};

use crate::model::TrackRef;
use crate::playback::{PlaybackQueue, QueueItem, TrackKey};

use super::LiveState;
use super::TuiAudio;

impl TuiAudio {
    pub(super) fn open_path(&self, path: &Path) -> Result<(), CmdError> {
        if self.is_live() {
            self.stop_live();
        }
        let player = Player::open(path)
            .map_err(|error| CmdError::new("E_AUDIO", format!("打开音频失败：{error}")))?;
        player.set_volume(self.volume.get());
        let tap = player.sample_tap();
        player
            .play()
            .map_err(|error| CmdError::new("E_AUDIO", format!("播放失败：{error}")))?;
        *self.title.borrow_mut() = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_owned());
        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
        self.duration_ms.set(player.duration_ms());
        *self.source_path.borrow_mut() = Some(path.to_path_buf());
        *self.tap.borrow_mut() = Some(tap);
        *self.player.borrow_mut() = Some(player);
        self.playing.set(true);
        Ok(())
    }
    pub(super) fn stop_playback(&self) {
        self.shutdown_live();
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        self.playing.set(false);
        self.duration_ms.set(None);
        *self.source_path.borrow_mut() = None;
    }
    pub fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub fn artist(&self) -> String {
        self.artist.borrow().clone()
    }

    pub fn duration_ms(&self) -> Option<u64> {
        if self.is_live() {
            return None;
        }
        self.player
            .borrow()
            .as_ref()
            .and_then(|player| player.duration_ms())
            .or(self.duration_ms.get())
    }

    pub fn position_ms(&self) -> u64 {
        self.player
            .borrow()
            .as_ref()
            .map_or(0, |player| player.position_ms())
    }

    pub fn is_playing(&self) -> bool {
        self.playing.get()
            && self
                .player
                .borrow()
                .as_ref()
                .is_some_and(|player| !player.is_ended())
    }

    pub fn tap(&self) -> Option<SampleTapHandle> {
        self.tap.borrow().clone()
    }

    #[cfg(test)]
    pub fn source_path(&self) -> Option<PathBuf> {
        self.source_path.borrow().clone()
    }

    pub fn volume(&self) -> f32 {
        self.volume.get()
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(player) = self.player.borrow().as_ref() {
            player.set_volume(volume);
        }
        self.volume.set(volume);
    }

    pub fn set_pending_metadata(&self, title: String, artist: String, duration_ms: u64) {
        *self.pending_metadata.borrow_mut() = Some((title, artist, duration_ms));
    }

    pub fn queue_items_snapshot(&self) -> Vec<QueueItem> {
        self.lock_queue().items().to_vec()
    }

    pub fn can_next(&self) -> bool {
        !self.is_live() && self.lock_queue().can_next()
    }

    pub fn can_prev(&self) -> bool {
        !self.is_live() && self.lock_queue().can_prev()
    }

    pub fn can_seek_back(&self) -> bool {
        !self.is_live() && self.position_ms() > 0
    }

    pub fn can_seek_forward(&self) -> bool {
        !self.is_live()
            && self
                .duration_ms()
                .is_some_and(|duration| duration > 0 && self.position_ms() < duration)
    }

    pub fn mode(&self) -> PlaybackMode {
        self.lock_queue().mode().to_playback_mode()
    }

    pub fn mode_id(&self) -> &'static str {
        match self.mode() {
            PlaybackMode {
                repeat: np_core::RepeatMode::One,
                shuffle: false,
            } => "single",
            PlaybackMode {
                repeat: np_core::RepeatMode::Off,
                shuffle: true,
            } => "shuffle",
            PlaybackMode {
                repeat: np_core::RepeatMode::All,
                shuffle: false,
            } => "list",
            _ => "sequential",
        }
    }

    pub fn set_mode_id(&self, id: &str) -> bool {
        let mode = match id {
            "single" => crate::playback::QueueMode::SingleLoop,
            "shuffle" => crate::playback::QueueMode::Shuffle,
            "sequential" => crate::playback::QueueMode::Sequential,
            "list" => crate::playback::QueueMode::ListLoop,
            _ => return false,
        };
        self.lock_queue().set_mode(mode);
        true
    }

    pub fn cycle_mode(&self) -> PlaybackMode {
        let mut queue = self.lock_queue();
        let next = match queue.mode() {
            crate::playback::QueueMode::SingleLoop => crate::playback::QueueMode::Shuffle,
            crate::playback::QueueMode::Shuffle => crate::playback::QueueMode::Sequential,
            crate::playback::QueueMode::Sequential => crate::playback::QueueMode::ListLoop,
            crate::playback::QueueMode::ListLoop => crate::playback::QueueMode::SingleLoop,
        };
        queue.set_mode(next);
        next.to_playback_mode()
    }

    fn lock_queue(&self) -> std::sync::MutexGuard<'_, PlaybackQueue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn queue_item(path: &str, _origin: &str) -> QueueItem {
        let path_buf = PathBuf::from(path);
        let title = path_buf
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "未命名".to_owned());
        QueueItem::new(TrackRef::Local(path_buf), title, "Nowhere Player", 0)
    }

    fn open_queue_item(&self, index: usize) -> Result<(), CmdError> {
        let item = self
            .lock_queue()
            .item_at(index)
            .cloned()
            .ok_or_else(|| CmdError::new("E_CMD_ARG", "队列序号越界"))?;
        match item.track {
            TrackRef::Local(path) => {
                self.open_path(&path)?;
                if !item.title.is_empty() {
                    *self.title.borrow_mut() = item.title;
                }
                if !item.artist.is_empty() {
                    *self.artist.borrow_mut() = item.artist;
                }
                if item.duration_ms > 0 {
                    self.duration_ms.set(Some(item.duration_ms));
                } else if let Some(duration_ms) = self.duration_ms.get() {
                    self.lock_queue().set_duration(index, duration_ms);
                }
                Ok(())
            }
            TrackRef::Remote { .. } => {
                Err(CmdError::new("E_SRC_UNAVAILABLE", "远程队列项需要在线解析"))
            }
        }
    }

    pub fn play_or_prepend(&self, item: QueueItem) -> Result<PlaybackState, CmdError> {
        let index = self.lock_queue().play_or_prepend(item);
        self.open_queue_item(index)?;
        Ok(self.state())
    }

    pub fn replace_playlist(
        &self,
        items: Vec<QueueItem>,
        selected: Option<&TrackKey>,
    ) -> Result<PlaybackState, CmdError> {
        let index = self.lock_queue().replace_playlist(items, selected);
        if let Some(index) = index {
            self.open_queue_item(index)?;
        }
        Ok(self.state())
    }

    pub fn toggle(&mut self) {
        if self.is_live() {
            match self.live_state.get() {
                LiveState::Playing => {
                    self.pause_live();
                }
                LiveState::Paused => {
                    self.resume_live();
                }
                LiveState::Idle | LiveState::Preparing | LiveState::Reconnecting => {}
            }
            return;
        }
        let player = self.player.borrow();
        let Some(player) = player.as_ref() else {
            return;
        };
        if self.playing.get() {
            let _ = player.pause();
            self.playing.set(false);
        } else {
            let _ = player.play();
            self.playing.set(true);
        }
    }

    fn queue_view(&self) -> QueueView {
        let queue = self.lock_queue();
        QueueView {
            tracks: queue
                .items()
                .iter()
                .filter_map(|item| match &item.track {
                    TrackRef::Local(path) => Some(path.to_string_lossy().into_owned()),
                    TrackRef::Remote { .. } => None,
                })
                .collect(),
            index: queue.current_index(),
        }
    }

    fn state(&self) -> PlaybackState {
        let (mode, queue_length, queue_index) = {
            let queue = self.lock_queue();
            (
                queue.mode().to_playback_mode(),
                queue.len(),
                queue.current_index(),
            )
        };
        PlaybackState {
            status: if self.playing.get() {
                PlaybackStatus::Playing
            } else if self.live_state.get() == LiveState::Paused
                || self.live_state.get() == LiveState::Reconnecting
                || self.player.borrow().is_some()
            {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Idle
            },
            track_id: if self.is_live()
                && (self.live_state.get() == LiveState::Paused
                    || self.live_state.get() == LiveState::Reconnecting)
            {
                Some(self.title())
            } else {
                self.player.borrow().as_ref().map(|_| self.title())
            },
            position_ms: self.position_ms(),
            duration_ms: self.duration_ms(),
            volume: self.volume.get(),
            mode,
            error: None,
            queue_length: Some(queue_length),
            queue_index,
        }
    }

    fn step(&self, forward: bool) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            return Err(CmdError::new("E_CMD_ARG", "直播中不可切换歌曲"));
        }
        let (index, same) = {
            let mut queue = self.lock_queue();
            if queue.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "队列为空"));
            }
            let before = queue.current_index();
            let index = if forward { queue.next() } else { queue.prev() };
            (index, index == before)
        };
        let Some(index) = index else {
            return Ok(self.state());
        };
        if !same {
            self.open_queue_item(index)?;
        }
        Ok(self.state())
    }
    pub fn handle_ended(&self) -> Result<bool, CmdError> {
        match self.live_state.get() {
            LiveState::Idle => {}
            LiveState::Preparing => return Ok(false),
            LiveState::Reconnecting => {
                let Some(at) = self.reconnect_at.get() else {
                    return Ok(false);
                };
                if Instant::now() < at {
                    return Ok(false);
                }
                let room = self.live_room.get();
                if room == 0 {
                    self.fail_live("直播重连失败：房间号丢失".to_owned());
                    return Ok(true);
                }
                self.reconnect_at.set(None);
                self.start_live_inner(room, false);
                return Ok(true);
            }
            LiveState::Paused => {
                let (ended, error) = {
                    let player = self.player.borrow();
                    player.as_ref().map_or((false, None), |player| {
                        (player.is_ended(), player.last_error())
                    })
                };
                if ended || error.is_some() {
                    let mut note = self.live_note.borrow_mut();
                    if note.is_none() {
                        *note = Some("直播连接已断开，恢复时重新连接".to_owned());
                    }
                    return Ok(true);
                }
                return Ok(false);
            }
            LiveState::Playing => {
                let (ended, error) = {
                    let player = self.player.borrow();
                    player.as_ref().map_or((false, None), |player| {
                        (player.is_ended(), player.last_error())
                    })
                };
                if ended || error.is_some() {
                    self.begin_reconnect(error);
                    return Ok(true);
                }
                return Ok(false);
            }
        }
        let ended = self
            .player
            .borrow()
            .as_ref()
            .is_some_and(|player| player.is_ended());
        if !ended {
            return Ok(false);
        }
        let next = {
            let mut queue = self.lock_queue();
            queue.next()
        };
        if let Some(index) = next {
            self.open_queue_item(index)?;
        } else {
            *self.player.borrow_mut() = None;
            *self.tap.borrow_mut() = None;
            self.playing.set(false);
            *self.source_path.borrow_mut() = None;
        }
        Ok(true)
    }
}

impl PlayerApi for TuiAudio {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        if path.is_none() && self.live_state.get() == LiveState::Paused {
            self.resume_live();
            return Ok(self.state());
        }
        if let Some(path) = path {
            let pending = self.pending_metadata.borrow_mut().take();
            let mut item = Self::queue_item(&path, "play");
            if let Some((title, artist, duration_ms)) = pending {
                item.title = title;
                item.artist = artist;
                item.duration_ms = duration_ms;
            }
            let index = self.lock_queue().play_or_prepend(item);
            self.open_queue_item(index)?;
        } else if let Some(player) = self.player.borrow().as_ref() {
            player
                .play()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
            self.playing.set(true);
        }
        Ok(self.state())
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            self.pause_live();
            return Ok(self.state());
        }
        if let Some(player) = self.player.borrow().as_ref() {
            player
                .pause()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        self.playing.set(false);
        Ok(PlaybackState {
            status: PlaybackStatus::Paused,
            ..self.state()
        })
    }

    fn resume(&self) -> Result<PlaybackState, CmdError> {
        if self.live_state.get() == LiveState::Paused {
            self.resume_live();
            return Ok(self.state());
        }
        self.play(None)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.step(true)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.step(false)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
        if self.is_live() {
            return Err(CmdError::new("E_CMD_ARG", "直播中不可跳转"));
        }
        if let Some(player) = self.player.borrow().as_ref() {
            player
                .seek(position_ms)
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        Ok(self.state())
    }

    fn volume(&self, volume: f32) -> Result<PlaybackState, CmdError> {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(player) = self.player.borrow().as_ref() {
            player.set_volume(volume);
        }
        self.volume.set(volume);
        Ok(self.state())
    }

    fn mode(&self, mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        self.lock_queue()
            .set_mode(crate::playback::QueueMode::from_playback_mode(mode));
        Ok(self.state())
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        Ok(self.queue_view())
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        {
            let mut queue = self.lock_queue();
            for path in paths {
                queue.push_back(Self::queue_item(&path, "queue add"));
            }
        }
        Ok(self.queue_view())
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        self.lock_queue().remove(index);
        Ok(self.queue_view())
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        if self.is_live() {
            self.stop_live();
        }
        self.lock_queue().clear();
        *self.player.borrow_mut() = None;
        *self.tap.borrow_mut() = None;
        self.playing.set(false);
        *self.title.borrow_mut() = "未播放".to_owned();
        *self.artist.borrow_mut() = "Nowhere Player".to_owned();
        self.duration_ms.set(None);
        *self.source_path.borrow_mut() = None;
        Ok(self.queue_view())
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        if self.lock_queue().jump(index).is_none() {
            return Err(CmdError::new("E_CMD_ARG", "队列序号越界"));
        }
        self.open_queue_item(index)?;
        Ok(self.queue_view())
    }
}
