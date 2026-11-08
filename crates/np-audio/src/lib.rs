//! 解码、重采样、混音与音频输出
//!
//! 对上层只暴露 [`Player`]；`sink` / `source` / `resample` / `format` 里的 trait
//! 是 ARCH-006 约定的扩展点（独占后端、在线音源、位完美直通）。

mod engine;

pub mod error;
pub mod format;
pub mod resample;
pub mod ring;
pub mod sink;
pub mod source;
pub mod volume;

pub use error::AudioError;
pub use format::{
    AudioFormat, Codec, DeviceCaps, SampleFormat, frames_to_ms, ms_to_frames, negotiate,
};
pub use resample::{GainPolicy, ProcessingPlan};
pub use sink::{
    AudioSink, CpalDevices, CpalSharedSink, DeviceInfo, DeviceProvider, RenderFn,
};
pub use source::{AudioSource, FileSource};
pub use volume::{clamp_volume, gain_for_volume};

use std::path::Path;

/// 一个已打开的本地音频文件 + 输出设备。
///
/// 创建即打开文件并启动输出流（默认暂停）。所有控制方法可以并发调用，
/// 内部只经原子变量通信，不会阻塞音频回调。
pub struct Player {
    engine: engine::Engine,
}

impl Player {
    /// 打开 `path`，协商设备格式并启动输出流。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        Ok(Self {
            engine: engine::Engine::start(path.as_ref())?,
        })
    }

    /// 开始 / 继续播放。
    pub fn play(&self) -> Result<(), AudioError> {
        self.engine.play()
    }

    /// 暂停。已解码但未播放的样本保留在环形缓冲里，继续时立即接着放。
    pub fn pause(&self) -> Result<(), AudioError> {
        self.engine.pause()
    }

    /// 设置音量（0.0–1.0，感知曲线 `gain = v^3`）。
    pub fn set_volume(&self, volume: f32) {
        self.engine.set_volume(volume);
    }

    /// 当前播放位置（毫秒），以设备实际消费的样本数为准。
    pub fn position_ms(&self) -> u64 {
        self.engine.position_ms()
    }

    /// 文件总时长（毫秒）；容器没有给出时为 `None`。
    pub fn duration_ms(&self) -> Option<u64> {
        self.engine.duration_ms()
    }

    /// 源文件格式（解码后内部为 f32）。
    pub fn format(&self) -> AudioFormat {
        self.engine.source_format()
    }

    /// 实际输出格式（设备协商结果）。
    pub fn output_format(&self) -> AudioFormat {
        self.engine.output_format()
    }

    /// 当前链路的处理计划（重采样 / 软件增益）。
    pub fn processing_plan(&self) -> ProcessingPlan {
        self.engine.plan()
    }

    /// 当前输出设备的支持格式（含采样率与通道数）。
    pub fn device_caps(&self) -> DeviceCaps {
        self.engine.device_caps()
    }

    /// 当前输出设备名。
    pub fn device_name(&self) -> &str {
        self.engine.device_name()
    }

    /// 是否已播到文件末尾。
    pub fn is_ended(&self) -> bool {
        self.engine.is_ended()
    }

    /// 累计欠载（回调拿不到样本）次数，用于诊断缓冲是否过小。
    pub fn underruns(&self) -> u64 {
        self.engine.underruns()
    }

    /// 解码线程或设备运行期错误（只记录第一条）。
    pub fn last_error(&self) -> Option<String> {
        self.engine.last_error()
    }
}
