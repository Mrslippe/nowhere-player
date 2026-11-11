//! 可选音频：`NP_TUI_AUDIO` 指向本地文件时打开真实 `np_audio::Player`。

use np_app::{CmdError, PlayerApi};
use np_audio::{Player, SampleTapHandle};
use np_core::{PlaybackMode, PlaybackState, PlaybackStatus, QueueView};
use std::path::PathBuf;

pub struct TuiAudio {
    player: Option<Player>,
    tap: Option<SampleTapHandle>,
    playing: bool,
    title: String,
    artist: String,
    duration_ms: Option<u64>,
}

impl TuiAudio {
    pub fn from_env() -> Self {
        let path = std::env::var_os("NP_TUI_AUDIO").map(PathBuf::from);
        let Some(path) = path else {
            return Self::empty();
        };
        match Player::open(&path) {
            Ok(player) => {
                let tap = player.sample_tap();
                let _ = player.play();
                let title = std::env::var("NP_TUI_TITLE").unwrap_or_else(|_| {
                    path.file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "未命名".to_owned())
                });
                let artist =
                    std::env::var("NP_TUI_ARTIST").unwrap_or_else(|_| "Nowhere Player".to_owned());
                let duration_ms = player.duration_ms();
                Self {
                    player: Some(player),
                    tap: Some(tap),
                    playing: true,
                    title,
                    artist,
                    duration_ms,
                }
            }
            Err(_) => Self::empty(),
        }
    }

    pub fn empty() -> Self {
        Self {
            player: None,
            tap: None,
            playing: false,
            title: "未播放".to_owned(),
            artist: "Nowhere Player".to_owned(),
            duration_ms: None,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn artist(&self) -> &str {
        &self.artist
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.player
            .as_ref()
            .and_then(|player| player.duration_ms())
            .or(self.duration_ms)
    }

    pub fn position_ms(&self) -> u64 {
        self.player
            .as_ref()
            .map_or(0, |player| player.position_ms())
    }

    pub fn is_playing(&self) -> bool {
        self.playing
            && self
                .player
                .as_ref()
                .is_some_and(|player| !player.is_ended())
    }

    pub fn tap(&self) -> Option<&SampleTapHandle> {
        self.tap.as_ref()
    }

    pub fn toggle(&mut self) {
        let Some(player) = &self.player else {
            return;
        };
        if self.playing {
            let _ = player.pause();
            self.playing = false;
        } else {
            let _ = player.play();
            self.playing = true;
        }
    }

    fn state(&self) -> PlaybackState {
        PlaybackState {
            status: if self.playing {
                PlaybackStatus::Playing
            } else if self.player.is_some() {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Idle
            },
            track_id: self.player.as_ref().map(|_| self.title.clone()),
            position_ms: self.position_ms(),
            duration_ms: self.duration_ms(),
            volume: 1.0,
            mode: PlaybackMode::default(),
            error: None,
            queue_length: None,
            queue_index: None,
        }
    }

    fn empty_queue() -> QueueView {
        QueueView {
            tracks: Vec::new(),
            index: None,
        }
    }
}

impl PlayerApi for TuiAudio {
    fn snapshot(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }

    fn play(&self, path: Option<String>) -> Result<PlaybackState, CmdError> {
        if path.is_some() {
            return Err(CmdError::new("E_UNSUPPORTED", "TUI 骨架暂不支持播放新文件"));
        }
        if let Some(player) = &self.player {
            player
                .play()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        Ok(self.state())
    }

    fn pause(&self) -> Result<PlaybackState, CmdError> {
        if let Some(player) = &self.player {
            player
                .pause()
                .map_err(|error| CmdError::new("E_AUDIO", error.to_string()))?;
        }
        Ok(PlaybackState {
            status: PlaybackStatus::Paused,
            ..self.state()
        })
    }

    fn resume(&self) -> Result<PlaybackState, CmdError> {
        self.play(None)
    }

    fn next(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }
    fn prev(&self) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }
    fn seek(&self, _position_ms: u64) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }
    fn volume(&self, _volume: f32) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }
    fn mode(&self, _mode: PlaybackMode) -> Result<PlaybackState, CmdError> {
        Ok(self.state())
    }
    fn queue_list(&self) -> Result<QueueView, CmdError> {
        Ok(Self::empty_queue())
    }
    fn queue_add(&self, _paths: Vec<String>) -> Result<QueueView, CmdError> {
        self.queue_list()
    }
    fn queue_remove(&self, _index: usize) -> Result<QueueView, CmdError> {
        self.queue_list()
    }
    fn queue_clear(&self) -> Result<QueueView, CmdError> {
        self.queue_list()
    }
    fn queue_jump(&self, _index: usize) -> Result<QueueView, CmdError> {
        self.queue_list()
    }
}
