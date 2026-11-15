use std::path::PathBuf;

use np_app::PlaybackTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchDisplay {
    Empty { query: String },
    Searching,
    NoResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisualMode {
    Spectrum,
    Waveform,
    Vectorscope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrowseKind {
    PlaylistRoot,
    Playlist(String),
    Library,
    Search,
    Queue,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Browse,
    List,
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackRef {
    Local(PathBuf),
    Remote { source: String, id: String },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ViewData {
    pub(crate) title: String,
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) refs: Vec<TrackRef>,
    pub(crate) durations: Vec<u64>,
}

pub(crate) struct RemotePlayback {
    pub(crate) tx: std::sync::mpsc::Sender<String>,
}

impl PlaybackTarget for RemotePlayback {
    fn play_path(&self, path: &str) -> Result<np_core::PlaybackState, np_app::CmdError> {
        let _ = self.tx.send(path.to_owned());
        Ok(np_core::PlaybackState::default())
    }
}
