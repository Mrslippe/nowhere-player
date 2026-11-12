use std::path::PathBuf;

use np_app::PlaybackTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisualMode {
    Spectrum,
    Waveform,
    Vectorscope,
}

impl VisualMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "频谱",
            Self::Waveform => "波形",
            Self::Vectorscope => "矢量",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Spectrum => Self::Waveform,
            Self::Waveform => Self::Vectorscope,
            Self::Vectorscope => Self::Spectrum,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Library,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiMode {
    Normal,
    Filter,
    Command,
    Help,
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
