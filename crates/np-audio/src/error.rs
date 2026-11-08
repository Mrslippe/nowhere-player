//! Error type shared by every audio subsystem.

use crate::format::SampleFormat;

/// Everything the audio crate can fail with.
///
/// Errors are created on the control/decode threads only; the audio callback
/// never formats or allocates one.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),
    #[error("failed to decode audio: {0}")]
    Decode(String),
    #[error("no audio output device available: {0}")]
    DeviceUnavailable(String),
    #[error("device does not support any usable output format")]
    NoSupportedFormat,
    #[error("unsupported device sample format: {0}")]
    UnsupportedSampleFormat(SampleFormat),
    #[error("failed to open output stream: {0}")]
    DeviceOpen(String),
    #[error("audio device lost")]
    DeviceLost,
    #[error("seek failed: {0}")]
    Seek(String),
    #[error("resampling failed: {0}")]
    Resample(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
