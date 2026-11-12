//! TUI 的播放器适配：本地文件真正打开，队列保存可播放路径。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};

pub struct TuiAudio {
    player: RefCell<Option<Player>>,
    tap: RefCell<Option<SampleTapHandle>>,
    playing: Cell<bool>,
    title: RefCell<String>,
    artist: RefCell<String>,
    duration_ms: Cell<Option<u64>>,
    source_path: RefCell<Option<PathBuf>>,
    volume: Cell<f32>,
    queue: Mutex<QueueView>,
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
            queue: Mutex::new(QueueView {
                tracks: Vec::new(),
                index: None,
            }),
        }
    }

    fn open_path(&self, path: &Path) -> Result<(), CmdError> {
        let player = Player::open(path)
            .map_err(|error| CmdError::new("E_AUDIO", format!("打开音频失败：{error}")))?;
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
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn state(&self) -> PlaybackState {
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
            mode: PlaybackMode::default(),
            error: None,
            queue_length: Some(self.queue_view().tracks.len()),
            queue_index: self.queue_view().index,
        }
    }

    fn step(&self, direction: i64) -> Result<PlaybackState, CmdError> {
        let (path, index) = {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if queue.tracks.is_empty() {
                return Err(CmdError::new("E_CMD_ARG", "队列为空"));
            }
            let current = queue.index.unwrap_or(0) as i64;
            let next = (current + direction).clamp(0, queue.tracks.len().saturating_sub(1) as i64)
                as usize;
            queue.index = Some(next);
            (queue.tracks[next].clone(), next)
        };
        self.open_path(Path::new(&path))?;
        Ok(PlaybackState {
            queue_index: Some(index),
            ..self.state()
        })
    }
}

impl PlayerApi for TuiAudio {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        if let Some(path) = path {
            self.open_path(Path::new(&path))?;
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
        self.step(1)
    }

    fn prev(&self) -> Result<PlaybackState, CmdError> {
        self.step(-1)
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

    fn mode(&self, _mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }

    fn queue_list(&self) -> Result<QueueView, CmdError> {
        Ok(self.queue_view())
    }

    fn queue_add(&self, paths: Vec<String>) -> Result<QueueView, CmdError> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.tracks.extend(paths);
        if queue.index.is_none() && !queue.tracks.is_empty() {
            queue.index = Some(0);
        }
        Ok(queue.clone())
    }

    fn queue_remove(&self, index: usize) -> Result<QueueView, CmdError> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if index < queue.tracks.len() {
            queue.tracks.remove(index);
        }
        queue.index = if queue.tracks.is_empty() {
            None
        } else {
            Some(0)
        };
        Ok(queue.clone())
    }

    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.tracks.clear();
        queue.index = None;
        Ok(queue.clone())
    }

    fn queue_jump(&self, index: usize) -> Result<QueueView, CmdError> {
        let path = {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if index >= queue.tracks.len() {
                return Err(CmdError::new("E_CMD_ARG", "队列序号越界"));
            }
            queue.index = Some(index);
            queue.tracks[index].clone()
        };
        self.open_path(Path::new(&path))?;
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
        PlayerApi::volume(&audio, 0.5).unwrap();
        assert!((audio.snapshot().unwrap().volume - 0.5).abs() < 0.001);
        audio.next().unwrap();
        assert!(audio.is_playing());
    }
}
