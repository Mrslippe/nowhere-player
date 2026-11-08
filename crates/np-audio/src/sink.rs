//! Audio output sinks and device capability probing.
//!
//! The [`AudioSink`] trait is the one seam that a future exclusive / bit-perfect
//! backend (WASAPI exclusive, CoreAudio hog, ALSA hw) has to implement; nothing
//! above it knows about cpal (ARCH-006 §2).
//!
//! The render callback runs on the audio thread and must not allocate, lock or
//! format strings. Everything it needs is either a stack buffer or an atomic.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat as CpalSampleFormat, SizedSample};

use crate::error::AudioError;
use crate::format::{AudioFormat, DeviceCaps, SampleFormat};

/// Fills `out` with interleaved f32 samples in the sink's negotiated format.
///
/// Implemented by the player's renderer; invoked from the audio callback.
pub type RenderFn = Box<dyn FnMut(&mut [f32]) + Send + 'static>;

/// Output backend abstraction (ARCH-006 §2).
pub trait AudioSink {
    /// Opens the device with exactly `format` and starts pulling audio from `render`.
    fn open(&mut self, format: AudioFormat, render: RenderFn) -> Result<(), AudioError>;

    /// Stops and releases the device stream.
    fn close(&mut self);

    /// Format the currently open stream runs at, if any.
    fn format(&self) -> Option<AudioFormat>;

    /// Format the device prefers when it is free to choose (shared-mode default).
    fn preferred_format(&self) -> Option<AudioFormat>;

    /// Estimated device-side latency in milliseconds (`0` when unknown).
    fn latency_ms(&self) -> u32;

    /// DSD is not supported by the v1 shared-mode sink (reserved, ARCH-006 §3).
    fn supports_dsd(&self) -> bool {
        false
    }

    /// Returns `true` once if the backend reported a runtime stream error.
    ///
    /// The error callback itself only flips an atomic, so this is the only place
    /// where a device loss is turned into an [`AudioError`].
    fn take_device_error(&self) -> bool {
        false
    }
}

/// One enumerable output device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// 默认输出设备的稳定标识（T-0016 §2.1）。
///
/// cpal 没有热插拔事件 API，上层 tick 循环只能周期性地取一次默认设备签名
/// 做比较。名字 + 通道数复合判断，降低驱动改名的误报。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSignature {
    /// 设备名（`cpal::Device::to_string()`）。
    pub name: String,
    /// 设备默认输出配置的通道数；拿不到默认配置时为 `0`。
    pub channels: u16,
}

/// Device enumeration / capability queries (ARCH-006 §2).
pub trait DeviceProvider {
    fn devices(&self) -> Result<Vec<DeviceInfo>, AudioError>;
    fn default_device(&self) -> Result<DeviceInfo, AudioError>;
    fn caps(&self, device_id: &str) -> Result<DeviceCaps, AudioError>;
}

/// cpal-backed shared-mode sink.
pub struct CpalSharedSink {
    device: cpal::Device,
    name: String,
    stream: Option<cpal::Stream>,
    format: Option<AudioFormat>,
    preferred_format: Option<AudioFormat>,
    latency_ms: u32,
    device_error: Arc<AtomicBool>,
}

impl CpalSharedSink {
    /// Binds to the system default output device.
    ///
    /// Returns a clear [`AudioError::DeviceUnavailable`] when Windows has no
    /// active output endpoint, instead of panicking.
    pub fn new() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| AudioError::DeviceUnavailable("no default output device".into()))?;
        let name = device.to_string();
        let preferred_format = device.default_output_config().ok().and_then(|config| {
            audio_format_from_cpal(
                config.sample_format(),
                config.channels(),
                config.sample_rate(),
            )
        });
        Ok(Self {
            device,
            name,
            stream: None,
            format: None,
            preferred_format,
            latency_ms: 0,
            device_error: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Human-readable device name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Every output format this device can drive with the shared-mode sink.
    pub fn device_caps(&self) -> Result<DeviceCaps, AudioError> {
        device_caps_from(&self.device)
    }

    fn config_for(&self, format: AudioFormat) -> Result<cpal::SupportedStreamConfig, AudioError> {
        let wanted = cpal_sample_format(format.sample_format);
        let configs = self
            .device
            .supported_output_configs()
            .map_err(|err| AudioError::DeviceOpen(err.to_string()))?;
        for range in configs {
            if range.channels() != format.channels || range.sample_format() != wanted {
                continue;
            }
            if let Some(config) = range.try_with_sample_rate(format.sample_rate) {
                return Ok(config);
            }
        }
        // Some hosts expose no ranges but still have a usable default config.
        if let Ok(default) = self.device.default_output_config()
            && default.channels() == format.channels
            && default.sample_rate() == format.sample_rate
            && default.sample_format() == wanted
        {
            return Ok(default);
        }
        Err(AudioError::NoSupportedFormat)
    }
}

impl AudioSink for CpalSharedSink {
    fn open(&mut self, format: AudioFormat, render: RenderFn) -> Result<(), AudioError> {
        self.close();
        let config = self.config_for(format)?;
        self.latency_ms = match config.buffer_size() {
            cpal::SupportedBufferSize::Range { min, .. } => {
                u64::from(*min).saturating_mul(1000) as u32 / format.sample_rate.max(1)
            }
            cpal::SupportedBufferSize::Unknown => 0,
        };
        let stream_config = config.config();
        let failed = Arc::clone(&self.device_error);
        let channels = usize::from(format.channels);
        let stream = match format.sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &self.device,
                stream_config,
                channels,
                render,
                Arc::clone(&failed),
            )?,
            SampleFormat::I16 => build_stream::<i16>(
                &self.device,
                stream_config,
                channels,
                render,
                Arc::clone(&failed),
            )?,
            SampleFormat::I32 => build_stream::<i32>(
                &self.device,
                stream_config,
                channels,
                render,
                Arc::clone(&failed),
            )?,
        };
        stream
            .play()
            .map_err(|err| AudioError::DeviceOpen(err.to_string()))?;
        self.stream = Some(stream);
        self.format = Some(format);
        Ok(())
    }

    fn close(&mut self) {
        self.stream = None;
        self.format = None;
    }

    fn format(&self) -> Option<AudioFormat> {
        self.format
    }

    fn preferred_format(&self) -> Option<AudioFormat> {
        self.preferred_format
    }

    fn latency_ms(&self) -> u32 {
        self.latency_ms
    }

    fn take_device_error(&self) -> bool {
        self.device_error.swap(false, Ordering::AcqRel)
    }
}

/// Maximum number of f32 samples rendered per callback chunk.
///
/// The stack scratch buffer is split into chunks of at most this size; the
/// audio callback therefore never needs a heap allocation, regardless of the
/// block size the backend asks for.
const MAX_CHUNK_SAMPLES: usize = 1024;

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut render: RenderFn,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    if channels == 0 {
        return Err(AudioError::UnsupportedSampleFormat(SampleFormat::F32));
    }
    device
        .build_output_stream::<T, _, _>(
            config,
            move |out: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let mut scratch = [0.0f32; MAX_CHUNK_SAMPLES];
                let mut offset = 0;
                while offset < out.len() {
                    let frames = (out.len() - offset) / channels;
                    let frames = frames.min(MAX_CHUNK_SAMPLES / channels);
                    if frames == 0 {
                        break;
                    }
                    let count = frames * channels;
                    render(&mut scratch[..count]);
                    for (dst, src) in out[offset..offset + count]
                        .iter_mut()
                        .zip(&scratch[..count])
                    {
                        *dst = T::from_sample(*src);
                    }
                    offset += count;
                }
                for dst in &mut out[offset..] {
                    *dst = T::from_sample(0.0f32);
                }
            },
            move |_err: cpal::Error| failed.store(true, Ordering::Release),
            None,
        )
        .map_err(|err| AudioError::DeviceOpen(err.to_string()))
}

/// cpal-backed device provider.
pub struct CpalDevices {
    host: cpal::Host,
}

impl CpalDevices {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// 当前默认输出设备的签名；没有可用设备时为 `None`。
    ///
    /// 只做一次默认设备查询 + 默认配置读取（廉价），供 actor 在 tick 里复用；
    /// 不新开线程，也不修改 [`AudioSink`] trait 的形状（T-0016 §3/§6）。
    pub fn default_output_signature(&self) -> Option<DeviceSignature> {
        let device = self.host.default_output_device()?;
        Some(device_signature(&device))
    }

    fn default_device_handle(&self) -> Result<cpal::Device, AudioError> {
        self.host
            .default_output_device()
            .ok_or_else(|| AudioError::DeviceUnavailable("no default output device".into()))
    }

    fn find(&self, device_id: &str) -> Result<cpal::Device, AudioError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|err| AudioError::DeviceUnavailable(err.to_string()))?;
        for device in devices {
            let id = device.id().map(|id| id.to_string()).unwrap_or_default();
            if id == device_id {
                return Ok(device);
            }
        }
        Err(AudioError::DeviceUnavailable(format!(
            "device not found: {device_id}"
        )))
    }
}

impl Default for CpalDevices {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceProvider for CpalDevices {
    fn devices(&self) -> Result<Vec<DeviceInfo>, AudioError> {
        let default_id = self
            .host
            .default_output_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());
        let devices = self
            .host
            .output_devices()
            .map_err(|err| AudioError::DeviceUnavailable(err.to_string()))?;
        let mut out = Vec::new();
        for device in devices {
            let id = device.id().map(|id| id.to_string()).unwrap_or_default();
            out.push(DeviceInfo {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name: device.to_string(),
            });
        }
        Ok(out)
    }

    fn default_device(&self) -> Result<DeviceInfo, AudioError> {
        let device = self.default_device_handle()?;
        let id = device.id().map(|id| id.to_string()).unwrap_or_default();
        Ok(DeviceInfo {
            id,
            name: device.to_string(),
            is_default: true,
        })
    }

    fn caps(&self, device_id: &str) -> Result<DeviceCaps, AudioError> {
        device_caps_from(&self.find(device_id)?)
    }
}

fn cpal_sample_format(format: SampleFormat) -> CpalSampleFormat {
    match format {
        SampleFormat::F32 => CpalSampleFormat::F32,
        SampleFormat::I16 => CpalSampleFormat::I16,
        SampleFormat::I32 => CpalSampleFormat::I32,
    }
}

fn audio_format_from_cpal(
    format: CpalSampleFormat,
    channels: u16,
    sample_rate: u32,
) -> Option<AudioFormat> {
    let sample_format = match format {
        CpalSampleFormat::F32 => SampleFormat::F32,
        CpalSampleFormat::I16 => SampleFormat::I16,
        CpalSampleFormat::I32 => SampleFormat::I32,
        _ => return None,
    };
    Some(AudioFormat::new(sample_rate, channels, sample_format))
}

fn device_signature(device: &cpal::Device) -> DeviceSignature {
    let name = device.to_string();
    let channels = device
        .default_output_config()
        .map(|config| config.channels())
        .unwrap_or(0);
    DeviceSignature { name, channels }
}

fn device_caps_from(device: &cpal::Device) -> Result<DeviceCaps, AudioError> {
    let ranges = device
        .supported_output_configs()
        .map_err(|err| AudioError::DeviceUnavailable(err.to_string()))?;
    let mut formats = Vec::new();
    for range in ranges {
        let Some(sample_format) = audio_format_from_cpal(
            range.sample_format(),
            range.channels(),
            range.min_sample_rate(),
        )
        .map(|format| format.sample_format) else {
            continue;
        };
        for rate in candidate_rates(range.min_sample_rate(), range.max_sample_rate()) {
            formats.push(AudioFormat::new(rate, range.channels(), sample_format));
        }
    }
    formats.sort_by_key(|format| (format.sample_rate, format.channels, format.sample_format));
    formats.dedup();
    Ok(DeviceCaps {
        formats,
        exclusive: false,
    })
}

/// Expands a supported rate range into representative rates.
///
/// `AudioFormat` carries one rate, so a range has to be sampled at the values
/// users actually care about. The endpoints are always included.
fn candidate_rates(min: u32, max: u32) -> Vec<u32> {
    const COMMON: [u32; 11] = [
        8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    ];
    let mut rates = vec![min, max];
    rates.extend(
        COMMON
            .into_iter()
            .filter(|rate| *rate >= min && *rate <= max),
    );
    rates.sort_unstable();
    rates.dedup();
    rates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_rates_keep_endpoints_and_common_rates() {
        let rates = candidate_rates(40_000, 50_000);
        assert_eq!(rates, vec![40_000, 44_100, 48_000, 50_000]);

        let rates = candidate_rates(44_100, 44_100);
        assert_eq!(rates, vec![44_100]);
    }

    #[test]
    fn device_signature_compares_name_and_channels() {
        let signature = DeviceSignature {
            name: "Speakers (Realtek)".into(),
            channels: 2,
        };
        assert_eq!(
            signature,
            DeviceSignature {
                name: "Speakers (Realtek)".into(),
                channels: 2,
            }
        );
        assert_ne!(
            signature,
            DeviceSignature {
                name: "Speakers (Realtek)".into(),
                channels: 6,
            },
            "the same name with a different channel count is a different device"
        );
        assert_ne!(
            signature,
            DeviceSignature {
                name: "Headphones".into(),
                channels: 2,
            }
        );
    }

    #[test]
    fn cpal_format_mapping_round_trips() {
        for format in [SampleFormat::F32, SampleFormat::I16, SampleFormat::I32] {
            let cpal = cpal_sample_format(format);
            assert_eq!(
                audio_format_from_cpal(cpal, 2, 48_000),
                Some(AudioFormat::new(48_000, 2, format))
            );
        }
        assert_eq!(
            audio_format_from_cpal(CpalSampleFormat::U16, 2, 48_000),
            None
        );
    }
}
