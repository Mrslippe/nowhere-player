//! 解码、重采样、混音与音频输出

pub mod error;
pub mod format;
pub mod ring;
pub mod sink;
pub mod volume;

pub use error::AudioError;
pub use format::{AudioFormat, Codec, DeviceCaps, SampleFormat, frames_to_ms, ms_to_frames, negotiate};
pub use sink::{AudioSink, CpalDevices, CpalSharedSink, DeviceInfo, DeviceProvider, RenderFn};
pub use volume::{clamp_volume, gain_for_volume};
