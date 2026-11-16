//! 解码、重采样、混音与音频输出
//!
//! 对上层只暴露 [`Player`]；`sink` / `source` / `resample` / `format` 里的 trait
//! 是 ARCH-006 约定的扩展点（独占后端、在线音源、位完美直通）。

mod engine;
mod tap;

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
    AudioSink, CpalDevices, CpalSharedSink, DeviceInfo, DeviceProvider, DeviceSignature, RenderFn,
};
pub use source::{AudioSource, FileSource};
pub use tap::{SampleTapHandle, TAP_CAPACITY};
pub use volume::{clamp_volume, gain_for_volume};

use std::path::Path;

/// 一个已打开的本地音频文件 + 输出设备。
///
/// 创建即打开文件并启动输出流（默认暂停）。所有控制方法可以并发调用，
/// 内部只经原子变量通信，不会阻塞音频回调。
pub struct Player {
    engine: engine::Engine,
    tap: SampleTapHandle,
}

impl Player {
    /// 打开 `path`，协商设备格式并启动输出流。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let (tap_writer, tap) = tap::create(48_000);
        Ok(Self {
            engine: engine::Engine::start(path.as_ref(), tap_writer)?,
            tap,
        })
    }

    /// 打开任意 [`AudioSource`]（在线流 / 直播），使用默认 300 ms ring。
    pub fn open_source(source: Box<dyn AudioSource>) -> Result<Self, AudioError> {
        Self::open_source_with_ring(source, engine::DEFAULT_RING_MS)
    }

    /// 打开任意 [`AudioSource`] 并指定 ring 缓冲窗口。
    ///
    /// 直播用 150 ms，普通文件用默认 300 ms。不可 seek 的 source 会让
    /// [`Player::seek`] 返回 [`AudioError::Seek`]。
    pub fn open_source_with_ring(
        source: Box<dyn AudioSource>,
        ring_ms: u32,
    ) -> Result<Self, AudioError> {
        if ring_ms == 0 {
            return Err(AudioError::InvalidArgument("ring_ms must be > 0"));
        }
        let (tap_writer, tap) = tap::create(48_000);
        Ok(Self {
            engine: engine::Engine::start_source(source, ring_ms, tap_writer)?,
            tap,
        })
    }

    /// 只读样本 tap；供频谱等可视化读取，不改变播放路径。
    pub fn sample_tap(&self) -> SampleTapHandle {
        self.tap.clone()
    }

    /// 开始 / 继续播放。
    pub fn play(&self) -> Result<(), AudioError> {
        self.engine.play()
    }

    /// 暂停。已解码但未播放的样本保留在环形缓冲里，继续时立即接着放。
    pub fn pause(&self) -> Result<(), AudioError> {
        self.engine.pause()
    }

    /// 跳转到 `position_ms`（毫秒）。
    ///
    /// 先淡出旧位置，再清空缓冲并按新位置重新解码，避免爆音和旧样本残留。
    /// 位置以解码器实际到达的位置为准（采样级误差 ≤ 一个 packet）。
    pub fn seek(&self, position_ms: u64) -> Result<(), AudioError> {
        self.engine.seek(position_ms)
    }

    /// 停止：淡出并保持暂停；可 seek 的源回到 0 位置，直播只暂停。
    pub fn stop(&self) -> Result<(), AudioError> {
        self.engine.stop()
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

    /// 输出流是否在运行中报过错（消费式探测）。
    ///
    /// 设备热插拔检测用（T-0016 §2.1）：与 [`Player::last_error`] 不同，
    /// 设备回调不写错误文本，只翻转标志；actor 的 tick 里调用一次。
    pub fn take_device_error(&self) -> bool {
        self.engine.take_device_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::SampleFormat;

    struct FakeSource {
        seekable: bool,
    }

    impl AudioSource for FakeSource {
        fn format(&self) -> AudioFormat {
            AudioFormat::new(48_000, 2, SampleFormat::F32)
        }

        fn codec(&self) -> Codec {
            Codec::Aac
        }

        fn duration_ms(&self) -> Option<u64> {
            None
        }

        fn is_seekable(&self) -> bool {
            self.seekable
        }

        fn read_frames(&mut self, _out: &mut [f32]) -> Result<usize, AudioError> {
            Ok(0)
        }

        fn seek(&mut self, _position_ms: u64) -> Result<u64, AudioError> {
            Err(AudioError::Seek("not seekable".to_owned()))
        }
    }

    #[test]
    fn zero_ring_is_rejected_before_opening_a_device() {
        let result = Player::open_source_with_ring(Box::new(FakeSource { seekable: false }), 0);
        assert!(matches!(result, Err(AudioError::InvalidArgument(_))));
    }

    #[test]
    fn live_source_reports_no_duration_and_no_seek() {
        let source = FakeSource { seekable: false };
        assert!(source.duration_ms().is_none());
        assert!(!source.is_seekable());
    }
}
