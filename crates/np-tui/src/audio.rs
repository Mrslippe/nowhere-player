//! TUI 的播放器适配：本地文件真正打开，队列保存可播放路径。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};

use crate::model::TrackRef;
use crate::playback::{PlaybackQueue, QueueItem, TrackKey};

pub struct TuiAudio {
    player: RefCell<Option<Player>>,
    tap: RefCell<Option<SampleTapHandle>>,
    playing: Cell<bool>,
    title: RefCell<String>,
    artist: RefCell<String>,
    duration_ms: Cell<Option<u64>>,
    source_path: RefCell<Option<PathBuf>>,
    volume: Cell<f32>,
    pending_metadata: RefCell<Option<(String, String, u64)>>,
    queue: Mutex<PlaybackQueue>,
}

impl TuiAudio {
    pub fn from_env() -> Self {
        let audio = Self::empty();
        let Some(path) = std::env::var_os("NP_TUI_AUDIO").map(PathBuf::from) else {
            return audio;
        };
        if audio.open_path(&path).is_ok() {
            if let Ok(title) = std::env::var("NP_TUI_TITLE") {
                *audio.title.borrow_mut() = title;
            }
            if let Ok(artist) = std::env::var("NP_TUI_ARTIST") {
                *audio.artist.borrow_mut() = artist;
            }
        }
        audio
    }

    pub fn empty() -> Self {
        Self {
            player: RefCell::new(None),
            tap: RefCell::new(None),
            playing: Cell::new(false),
            title: RefCell::new("未播放".to_owned()),
            artist: RefCell::new("Nowhere Player".to_owned()),
            duration_ms: Cell::new(None),
            source_path: RefCell::new(None),
            volume: Cell::new(1.0),
            pending_metadata: RefCell::new(None),
            queue: Mutex::new(PlaybackQueue::new()),
        }
    }

    fn open_path(&self, path: &Path) -> Result<(), CmdError> {
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

    pub fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub fn artist(&self) -> String {
        self.artist.borrow().clone()
    }

    pub fn duration_ms(&self) -> Option<u64> {
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

    pub fn set_pending_metadata(&self, title: String, artist: String, duration_ms: u64) {
        *self.pending_metadata.borrow_mut() = Some((title, artist, duration_ms));
    }

    pub fn queue_items_snapshot(&self) -> Vec<QueueItem> {
        self.lock_queue().items().to_vec()
    }

    pub fn mode(&self) -> PlaybackMode {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mode()
            .to_playback_mode()
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
            } else if self.player.borrow().is_some() {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Idle
            },
            track_id: self.player.borrow().as_ref().map(|_| self.title()),
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
        self.play(None)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        self.step(true)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.step(false)
    }

    fn seek(&self, position_ms: u64) -> Result<PlaybackState, CmdError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_play_opens_a_file_instead_of_returning_unsupported() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        let state = audio
            .play(Some(path.to_string_lossy().into_owned()))
            .expect("local play should open");
        assert!(matches!(state.status, PlaybackStatus::Playing));
        assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
    }

    #[test]
    fn tone_440_meets_the_four_playback_criteria() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        let state = audio
            .play(Some(path.to_string_lossy().into_owned()))
            .expect("no E_AUDIO");
        assert!(matches!(state.status, PlaybackStatus::Playing));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            audio.position_ms() >= 800,
            "position {}",
            audio.position_ms()
        );
        assert_eq!(audio.source_path().as_deref(), Some(path.as_path()));
    }

    #[test]
    fn queue_steps_and_volume_are_real() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        if !path.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        audio
            .queue_add(vec![path.to_string_lossy().into_owned()])
            .unwrap();
        audio.queue_jump(0).unwrap();
        PlayerApi::volume(&audio, 0.5).unwrap();
        assert!((audio.snapshot().unwrap().volume - 0.5).abs() < 0.001);
        let mode = PlaybackMode {
            repeat: np_core::RepeatMode::Off,
            shuffle: true,
        };
        PlayerApi::mode(&audio, mode).unwrap();
        audio.next().unwrap();
        assert!(audio.is_playing());
        let state = audio.snapshot().unwrap();
        assert!(
            (state.volume - 0.5).abs() < 0.001,
            "volume must survive track change"
        );
        assert_eq!(state.mode, mode, "mode must survive track change");
    }

    #[test]
    fn prepend_duplicate_and_next_follow_unique_queue() {
        let first =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone.wav");
        let second =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp/accept/tone-440.wav");
        if !first.is_file() || !second.is_file() {
            return;
        }
        let audio = TuiAudio::empty();
        audio
            .play(Some(first.to_string_lossy().into_owned()))
            .unwrap();
        audio
            .play(Some(second.to_string_lossy().into_owned()))
            .unwrap();
        assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
        audio
            .play(Some(second.to_string_lossy().into_owned()))
            .unwrap();
        assert_eq!(audio.queue_list().unwrap().tracks.len(), 2);
        audio.next().unwrap();
        assert_eq!(audio.source_path().as_deref(), Some(first.as_path()));
    }
}
