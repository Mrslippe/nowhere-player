//! 直播 source：网络线程产 `AacFrame` → 有界队列 → 本线程 direct `AacDecoder`。
//!
//! 与 B1 的 `AdtsReader + MediaSourceStream(64KB)` 不同，raw AAC 帧直接进
//! `symphonia` 的 `AacDecoder`（ASC 作为 `extra_data`），中间只有 [`FrameQueue`]。
//! 因此缓冲是显式、可观测的：
//!
//! - 启动等到约一个安全垫（默认 min(target,850ms)），再从最新帧截到安全垫，避免把 CDN 初始 GOP 全部播完；
//! - 运行期解码出的 PCM 过 WSOLA：`buffer > target` 1.10x、`> target+1s` 1.20x、
//!   `> target+2s` 1.30x；`buffer < min(600ms,target/2)` 降到 0.60x；
//! - **不做运行期硬丢帧**；队列满时生产者等待，靠 1.30x 追回；
//! - 队列有数据时持续解码喂 ring；只有队列（和 WSOLA 尾部）都空才补静音并记录 underrun；
//! - 暂停时保留 Player/解码器，网络线程继续读；缓存超过 30s 丢新帧；
//! - 恢复后先播暂停缓存，播到缓存尾再恢复目标缓冲控制器追 live。

use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration as MediaDuration, Timestamp};
use symphonia::default::codecs::AacDecoder;

use np_audio::format::{AudioFormat, Codec, SampleFormat};
use np_audio::{AudioError, AudioSource, WsolaTempo};

use crate::control::{LiveControl, PAUSE_CAP_FRAMES, PAUSE_CAP_MS};
use crate::error::LiveError;
use crate::flv::{AacConfig, AacFrame, FlvFrameSource};
use crate::metrics::LiveMetrics;
#[cfg(test)]
use crate::queue::TARGET_BUFFER_MS;
use crate::queue::{
    BufferController, Control, FrameQueue, QUEUE_CAPACITY_FRAMES, target_buffer_ms,
};

/// 启动最多等这么久攒够安全垫；超时就用手上已有的帧开始播。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
/// 启动安全垫上限：网络按秒级块到达时，850ms 比 target 1000ms 少约 150ms 固定延迟；
/// 700ms 对约 1 秒一块的源流偏薄（T-0110 真机启动即 underrun）。
const STARTUP_SAFETY_MS: u64 = 850;
/// 启动后等待初始 GOP 到齐的 settle 窗口；防止 850ms 起播后立刻被 3s 突发灌满。
const STARTUP_SETTLE_MS: u64 = 200;
/// 本地 `r` 刷新保留的尾部音频：从最新缓冲前 300ms 起播。
const REFRESH_KEEP_MS: u64 = 300;
/// AAC-LC 每个 raw frame 固定 1024 个采样。
const AAC_FRAME_SAMPLES: u64 = 1024;
/// 连续解码失败这么多次就认定流不可解，交给 TUI 停播；单帧损坏不致命。
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 30;

/// 启动垫：`min(target, STARTUP_SAFETY_MS)`，target 更小时不强行垫到 850。
fn startup_ms(target_ms: u64) -> u64 {
    target_ms.min(STARTUP_SAFETY_MS)
}

/// 直接解码 AAC 的直播 source。
pub(crate) struct LiveAudioSource {
    decoder: AacDecoder,
    queue: FrameQueue,
    metrics: LiveMetrics,
    format: AudioFormat,
    decoded: Vec<f32>,
    /// WSOLA 追帧器：常开，1.00x 时逐样本还原。
    tempo: WsolaTempo,
    controller: BufferController,
    consecutive_decode_errors: u32,
    control: LiveControl,
    replay_until_pts: Option<u64>,
}

impl LiveAudioSource {
    /// 读 FLV 头 / AAC 配置、起网络线程、等到约一个目标缓冲后返回。
    ///
    /// 可能阻塞在网络读取上（准备线程专用）。
    pub(crate) fn open(
        reader: Box<dyn Read + Send>,
        control: LiveControl,
    ) -> Result<(Self, LiveMetrics), LiveError> {
        let metrics = LiveMetrics::default();
        let (flv, config) = FlvFrameSource::open(reader, metrics.clone())?;
        let format = AudioFormat::new(config.sample_rate(), config.channels(), SampleFormat::F32);
        let decoder = build_decoder(&config)?;

        let queue = FrameQueue::new(QUEUE_CAPACITY_FRAMES);
        // 不保存 JoinHandle：drop 即 detach；退出由 `queue.stop()` 通知。
        thread::Builder::new()
            .name("np-live-flv".to_owned())
            .spawn({
                let queue = queue.clone();
                let control = control.clone();
                let metrics = metrics.clone();
                move || producer_loop(flv, queue, control, metrics)
            })
            .map_err(LiveError::Io)?;

        let target_ms = target_buffer_ms();
        let startup_ms = startup_ms(target_ms);
        log::info!(
            target: "live",
            "event=open target_ms={target_ms} startup_ms={startup_ms}"
        );
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        queue.wait_span(startup_ms, deadline);

        if queue.len() == 0 {
            queue.stop();
            let message = queue
                .error()
                .unwrap_or_else(|| "直播流没有音频帧".to_owned());
            return Err(LiveError::BadStream(message));
        }

        // 初始 GOP 可能在达到安全垫之后才到齐；等一个短 settle 再裁一次，
        // 否则 Player 一打开就会被几秒突发灌满，1.3x 要很久才追回来。
        let settle_until = Instant::now() + Duration::from_millis(STARTUP_SETTLE_MS);
        while !queue.is_done() && Instant::now() < settle_until {
            thread::sleep(Duration::from_millis(20));
        }
        let before_settle = queue.buffer_ms();

        // 启动裁剪：只保留安全垫，不把 CDN 初始 GOP 全部播一遍；target 是运行期高水位。
        let newest = queue.newest_pts().unwrap_or_default();
        let front = queue.front_pts().unwrap_or_default();
        if newest.saturating_sub(front) > startup_ms {
            let threshold = newest.saturating_sub(startup_ms);
            queue.drop_below(threshold);
        }
        metrics.set_buffer(queue.buffer_ms());
        log::info!(
            target: "live",
            "event=started target_ms={} startup_ms={} settle_ms={} before={} buffer_ms={} frames={}",
            target_ms,
            startup_ms,
            STARTUP_SETTLE_MS,
            before_settle,
            queue.buffer_ms(),
            queue.len()
        );

        Ok((
            Self {
                decoder,
                queue,
                metrics: metrics.clone(),
                format,
                decoded: Vec::new(),
                tempo: WsolaTempo::new(format.sample_rate, format.channels),
                controller: BufferController::new(),
                consecutive_decode_errors: 0,
                control,
                replay_until_pts: None,
            },
            metrics,
        ))
    }

    /// 100ms 控制周期：只调整 WSOLA 速度；低于下限由 `note_starved` 记录。
    fn update_control(&mut self) {
        let now = Instant::now();
        let buffer = self.queue.buffer_ms();
        self.metrics.set_buffer(buffer);

        // 恢复后的 replay 阶段：front 还没走到暂停缓存尾，不能丢旧帧。
        if let Some(until) = self.replay_until_pts {
            let front = self.queue.front_pts().unwrap_or(0);
            if front < until {
                return;
            }
            self.replay_until_pts = None;
        }

        if !self.controller.due(now) {
            return;
        }
        let before = self.controller.speed();
        let action = self.controller.tick(now, buffer);
        let after = self.controller.speed();
        if (after - before).abs() > 0.001 {
            log::info!(
                target: "live",
                "event=speed from={before:.2} to={after:.2} buffer_ms={buffer} target_ms={}",
                self.controller.target_ms()
            );
        }
        match action {
            Control::Recovered => {
                log::info!(
                    target: "live",
                    "event=recovered buffer_ms={} speed={after:.2}",
                    self.queue.buffer_ms()
                );
            }
            Control::Idle => {}
        }
    }

    /// 本地 `r` 刷新：丢掉队头旧帧，只保留最新缓冲前 300ms。
    fn refresh_to_latest(&mut self) {
        let Some(newest) = self.queue.newest_pts() else {
            self.tempo.reset();
            self.decoder.reset();
            self.decoded.clear();
            self.replay_until_pts = None;
            self.metrics.set_buffer(0);
            log::info!(target: "live", "event=refresh dropped=0 buffer_ms=0");
            return;
        };
        let threshold = newest.saturating_sub(REFRESH_KEEP_MS);
        let dropped = self.queue.drop_below(threshold);
        // 丢掉旧帧后解码器/WSOLA 的重叠窗不再连续，重置避免爆音和旧相位。
        self.tempo.reset();
        self.decoder.reset();
        self.decoded.clear();
        self.consecutive_decode_errors = 0;
        self.replay_until_pts = None;
        self.metrics.set_buffer(self.queue.buffer_ms());
        log::info!(
            target: "live",
            "event=refresh dropped={} keep_ms={} buffer_ms={}",
            dropped,
            REFRESH_KEEP_MS,
            self.queue.buffer_ms()
        );
    }

    fn decode_frame(&mut self, frame: AacFrame) -> Result<(), AudioError> {
        let packet = Packet::new(
            0,
            Timestamp::new(frame.pts_ms as i64),
            MediaDuration::new(AAC_FRAME_SAMPLES),
            frame.raw,
        );
        match self.decoder.decode(&packet) {
            Ok(decoded) => {
                self.consecutive_decode_errors = 0;
                let spec = decoded.spec();
                let rate = spec.rate();
                let channels = spec.channels().count();
                if rate != self.format.sample_rate || channels != usize::from(self.format.channels)
                {
                    return Err(AudioError::Decode(format!(
                        "直播音频格式变化：{rate}Hz/{}ch，期望 {}Hz/{}ch",
                        channels, self.format.sample_rate, self.format.channels
                    )));
                }
                decoded.copy_to_vec_interleaved::<f32>(&mut self.decoded);
                self.metrics.set_buffer(self.queue.buffer_ms());
                Ok(())
            }
            Err(error) => {
                self.metrics.note_decode_error();
                self.consecutive_decode_errors += 1;
                log::warn!(
                    target: "live",
                    "event=decode_error count={} error={error}",
                    self.consecutive_decode_errors
                );
                if self.consecutive_decode_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                    return Err(AudioError::Decode(format!(
                        "直播 AAC 连续解码失败：{error}"
                    )));
                }
                Ok(())
            }
        }
    }

    /// 队列空但流没结束：补静音，保持 1x 时间轴，不减速。
    fn fill_silence(&mut self, out: &mut [f32], channels: usize, written: usize) -> usize {
        if self.controller.note_starved() {
            self.metrics.note_underrun();
            log::warn!(
                target: "live",
                "event=underrun buffer_ms={} queue_empty={}",
                self.queue.buffer_ms(),
                self.metrics.queue_empty_events()
            );
        }
        let remaining_frames = out.len() / channels - written;
        let silence_ms = (remaining_frames as u64 * 1_000) / u64::from(self.format.sample_rate);
        self.metrics.note_fill_silence(silence_ms);
        out[written * channels..].fill(0.0);
        out.len() / channels
    }
}

impl AudioSource for LiveAudioSource {
    fn format(&self) -> AudioFormat {
        self.format
    }

    fn codec(&self) -> Codec {
        Codec::Aac
    }

    fn duration_ms(&self) -> Option<u64> {
        None
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn read_frames(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        let channels = usize::from(self.format.channels);
        let max_frames = out.len() / channels;
        if max_frames == 0 {
            return Ok(0);
        }
        // resume 后第一次读取时固定 replay 边界：暂停缓存尾。
        if self.control.take_replay() {
            self.replay_until_pts = self.queue.newest_pts();
        }
        // 本地刷新要在 replay 之后处理，清掉刚设置的 replay 边界。
        if self.control.take_refresh() {
            self.refresh_to_latest();
        }
        let mut written = 0;
        loop {
            // 先交出 WSOLA 已合成好的样本。
            if written < max_frames {
                let samples = self
                    .tempo
                    .take(&mut out[written * channels..max_frames * channels]);
                written += samples / channels;
            }
            if written == max_frames {
                break;
            }

            self.update_control();
            self.tempo.set_speed(self.controller.speed());

            // 只要队列里还有帧，就必须持续解码喂 150ms ring。低 buffer 时
            // 绝不能阻塞 decode 线程去等回填，否则 cpal 会先 underrun。
            let Some(frame) = self.queue.pop() else {
                if self.queue.is_done() {
                    // 网络错误要尽快冒泡，但不能把已经解码、卡在 WSOLA
                    // 分析窗里的最后一帧直接吞掉。
                    if let Some(error) = self.queue.error() {
                        self.metrics.set_buffer(0);
                        if written == 0 {
                            self.tempo.finish();
                            let samples = self
                                .tempo
                                .take(&mut out[written * channels..max_frames * channels]);
                            if samples > 0 {
                                return Ok(samples / channels);
                            }
                            return Err(AudioError::Decode(format!("直播流中断：{error}")));
                        }
                        return Ok(written);
                    }
                    // 正常结束：把 WSOLA 重叠区里剩下的样本合成完再收尾。
                    self.tempo.finish();
                    let samples = self
                        .tempo
                        .take(&mut out[written * channels..max_frames * channels]);
                    written += samples / channels;
                    if samples > 0 {
                        continue;
                    }
                    self.metrics.set_buffer(0);
                    return Ok(written);
                }
                self.metrics.note_queue_empty();
                return Ok(self.fill_silence(out, channels, written));
            };

            // 队列里只会留下 pts 递增的帧（丢帧只从队头删），不存在过期帧。
            self.decode_frame(frame)?;
            self.tempo.push(&self.decoded);
        }
        Ok(written)
    }

    fn seek(&mut self, _position_ms: u64) -> Result<u64, AudioError> {
        Err(AudioError::Seek("直播流不可 seek".to_owned()))
    }
}

impl Drop for LiveAudioSource {
    fn drop(&mut self) {
        // 只置停止标志，不 join：网络线程可能正阻塞在一次 HTTP read 上。
        self.queue.stop();
    }
}

fn build_decoder(config: &AacConfig) -> Result<AacDecoder, LiveError> {
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(config.sample_rate())
        .with_channels(Channels::Discrete(config.channels()))
        .with_extra_data(config.asc().to_vec().into_boxed_slice());
    AacDecoder::try_new(&params, &AudioDecoderOptions::default().gapless(false))
        .map_err(|error| LiveError::UnsupportedAudio(format!("创建 AAC 解码器失败：{error}")))
}

fn producer_loop(
    mut flv: FlvFrameSource,
    queue: FrameQueue,
    control: LiveControl,
    metrics: LiveMetrics,
) {
    loop {
        match flv.next_frame() {
            Ok(Some(frame)) => {
                // 暂停时保留暂停起点：缓存已到 cap，就丢弃新帧但不阻塞读取。
                if control.is_paused()
                    && (queue.len() >= PAUSE_CAP_FRAMES || queue.buffer_ms() >= PAUSE_CAP_MS)
                {
                    control.note_paused_drop();
                    metrics.set_buffer(queue.buffer_ms());
                    continue;
                }
                if !queue.push(frame) {
                    break;
                }
                metrics.set_buffer(queue.buffer_ms());
            }
            Ok(None) => {
                queue.finish();
                break;
            }
            Err(error) => {
                queue.fail(error.to_string());
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// ffmpeg 生成的 AAC-LC 48kHz 立体声 ADTS（440Hz，2.03s）：
    ///
    /// ```text
    /// ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" \
    ///        -ac 2 -c:a aac -b:a 64k -f adts aac_lc_48k_stereo.adts
    /// ```
    ///
    /// 测试只从里面取 raw frame，避免访问真实直播间。
    const ADTS_FIXTURE: &[u8] = include_bytes!("../fixtures/aac_lc_48k_stereo.adts");
    const ASC_48K_STEREO: [u8; 4] = [0xAF, 0x00, 0x11, 0x90];
    const PTS_STEP_MS: u64 = 1024 * 1000 / 48_000;

    fn fixture_frames() -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut position = 0;
        while position + 7 <= ADTS_FIXTURE.len() {
            let protection_absent = ADTS_FIXTURE[position + 1] & 0x01;
            let header = if protection_absent == 0 { 9 } else { 7 };
            let frame_len = ((usize::from(ADTS_FIXTURE[position + 3]) & 0x03) << 11)
                | (usize::from(ADTS_FIXTURE[position + 4]) << 3)
                | (usize::from(ADTS_FIXTURE[position + 5]) >> 5);
            assert!(frame_len > header, "malformed ADTS fixture");
            frames.push(ADTS_FIXTURE[position + header..position + frame_len].to_vec());
            position += frame_len;
        }
        assert_eq!(
            position,
            ADTS_FIXTURE.len(),
            "ADTS fixture has trailing bytes"
        );
        assert!(frames.len() > 40, "fixture too short for a 900ms startup");
        frames
    }

    fn tag(tag_type: u8, payload: &[u8], timestamp: u32) -> Vec<u8> {
        let size = payload.len();
        let mut bytes = vec![
            tag_type,
            (size >> 16) as u8,
            (size >> 8) as u8,
            size as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (timestamp >> 24) as u8,
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&(size as u32 + 11).to_be_bytes());
        bytes
    }

    /// 把 raw AAC 帧拼成 FLV（ASC + 每帧一个音频 tag）。
    fn flv_stream(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"FLV\x01".to_vec();
        bytes.push(4);
        bytes.extend_from_slice(&9u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&tag(8, &ASC_48K_STEREO, 0));
        for (index, raw) in frames.iter().enumerate() {
            let mut payload = vec![0xAF, 0x01];
            payload.extend_from_slice(raw);
            let pts = index as u64 * PTS_STEP_MS;
            bytes.extend_from_slice(&tag(8, &payload, pts as u32));
        }
        bytes
    }

    fn open_source(bytes: Vec<u8>) -> (LiveAudioSource, LiveMetrics) {
        LiveAudioSource::open(Box::new(Cursor::new(bytes)), LiveControl::default())
            .expect("open live source")
    }

    fn read_until_done(source: &mut LiveAudioSource) -> (usize, f64, usize) {
        let channels = usize::from(source.format().channels);
        let mut out = vec![0.0f32; 1024 * channels];
        let mut frames = 0usize;
        let mut sum_sq = 0.0f64;
        let mut samples = 0usize;
        loop {
            let read = source.read_frames(&mut out).expect("decode");
            if read == 0 {
                break;
            }
            frames += read;
            for sample in &out[..read * channels] {
                sum_sq += f64::from(*sample) * f64::from(*sample);
            }
            samples += read * channels;
        }
        let rms = (sum_sq / samples.max(1) as f64).sqrt();
        (frames, rms, samples)
    }

    fn read_all_samples(source: &mut LiveAudioSource) -> Vec<f32> {
        let channels = usize::from(source.format().channels);
        let mut out = vec![0.0f32; 1024 * channels];
        let mut samples = Vec::new();
        loop {
            let read = source.read_frames(&mut out).expect("decode");
            if read == 0 {
                break;
            }
            samples.extend_from_slice(&out[..read * channels]);
        }
        samples
    }

    /// 过零率估算基频；直播夹具是 440Hz 正弦。
    fn estimated_frequency(samples: &[f32], channels: usize, rate: u32) -> f64 {
        let frames = samples.len() / channels;
        let trim = rate as usize / 10; // 首尾各跳过 100ms
        let end = frames.saturating_sub(trim);
        if end <= trim {
            return 0.0;
        }
        let mut crossings = 0usize;
        let mut previous = samples[trim * channels];
        for frame in (trim + 1)..end {
            let current = samples[frame * channels];
            if (previous <= 0.0 && current > 0.0) || (previous >= 0.0 && current < 0.0) {
                crossings += 1;
            }
            previous = current;
        }
        let seconds = (end - trim) as f64 / f64::from(rate);
        crossings as f64 / (2.0 * seconds)
    }

    #[test]
    fn refresh_drops_to_the_latest_300ms() {
        let control = LiveControl::default();
        let (mut source, _metrics) = LiveAudioSource::open(
            Box::new(Cursor::new(flv_stream(&fixture_frames()))),
            control.clone(),
        )
        .expect("open");
        control.request_refresh();
        let mut out = vec![0.0f32; 1024 * 2];
        let read = source.read_frames(&mut out).expect("refresh read");
        assert!(read > 0);
        assert!(
            source.queue.buffer_ms() <= 500,
            "refresh should keep about 300ms, got {}ms",
            source.queue.buffer_ms()
        );
    }

    #[test]
    fn startup_pad_is_capped_by_target() {
        // 目标 ≥ 850：垫到 850；目标更小则等于 target，不硬拉高。
        assert_eq!(startup_ms(900), 850);
        assert_eq!(startup_ms(1_500), 850);
        assert_eq!(startup_ms(850), 850);
        assert_eq!(startup_ms(500), 500);
        assert_eq!(startup_ms(0), 0);
    }

    #[test]
    fn startup_keeps_only_the_safety_margin() {
        let frames = fixture_frames();
        // 闸门放在 60 帧（约 1.24s），保证启动后确实按 850ms 裁掉多余缓冲。
        let gate_at = flv_stream(&frames[..60]).len();
        let gate = Arc::new(AtomicBool::new(false));
        let reader = GateReader {
            data: Cursor::new(flv_stream(&frames)),
            gate_at,
            gate: gate.clone(),
        };
        let (source, metrics) =
            LiveAudioSource::open(Box::new(reader), LiveControl::default()).expect("open");
        let buffer = metrics.buffer_ms();
        assert!(
            (830..=1_000).contains(&buffer),
            "startup buffer {buffer}ms should be about 850ms safety margin"
        );
        assert_eq!(
            metrics.drop_events(),
            0,
            "startup trim is not a runtime drop"
        );
        gate.store(true, Ordering::Release);
        drop(source);
    }

    #[test]
    fn direct_decode_produces_pcm_without_errors() {
        let (mut source, metrics) = open_source(flv_stream(&fixture_frames()));
        assert_eq!(source.format().sample_rate, 48_000);
        assert_eq!(source.format().channels, 2);
        assert_eq!(source.duration_ms(), None);
        assert!(!source.is_seekable());

        let (frames, rms, _samples) = read_until_done(&mut source);
        // 启动只保留约 850ms 安全垫；夹具后续帧仍会继续解码。
        assert!(
            frames > 30_000,
            "expected at least the safety margin of PCM, got {frames} frames"
        );
        assert!(rms > 1e-4, "decoded PCM is silent (rms {rms})");
        assert_eq!(metrics.decode_errors(), 0);
        assert!(matches!(source.seek(1_000), Err(AudioError::Seek(_))));
    }

    #[test]
    fn wsola_passthrough_keeps_the_tone_pitch() {
        let (mut source, _) = open_source(flv_stream(&fixture_frames()));
        let samples = read_all_samples(&mut source);
        let channels = 2;
        let hz = estimated_frequency(&samples, channels, 48_000);
        assert!(
            (hz - 440.0).abs() < 8.0,
            "WSOLA at 1.00x changed the pitch: {hz:.1}Hz"
        );
    }

    #[test]
    fn excess_buffer_speeds_up_without_dropping_frames() {
        let frames = fixture_frames();
        let mut burst = frames.clone();
        burst.extend(frames.iter().cloned());
        let all = flv_stream(&burst);
        // 前 60 帧之后闸门关闭，先让启动缓冲攒到 60 帧。
        let gate_at = flv_stream(&frames[..60]).len();
        let gate = Arc::new(AtomicBool::new(false));
        let reader = GateReader {
            data: Cursor::new(all),
            gate_at,
            gate: gate.clone(),
        };

        let (mut source, metrics) = LiveAudioSource::open(Box::new(reader), LiveControl::default())
            .expect("open gated source");
        wait_until(
            || source.queue.newest_pts().unwrap_or(0) >= 59 * PTS_STEP_MS,
            Duration::from_secs(2),
        );
        gate.store(true, Ordering::Release);
        wait_until(
            || source.queue.newest_pts().unwrap_or(0) >= 150 * PTS_STEP_MS,
            Duration::from_secs(2),
        );

        let mut out = vec![0.0f32; 1024 * 2];
        let read = source.read_frames(&mut out).expect("decode");
        assert!(read > 0);
        assert_eq!(metrics.drop_events(), 0, "runtime must not drop frames");
        assert!(
            source.controller.speed() >= 1.10,
            "buffer above target must engage WSOLA, speed={}",
            source.controller.speed()
        );
        assert!(metrics.buffer_ms() > TARGET_BUFFER_MS);
    }

    #[test]
    fn starvation_fills_silence_and_counts_one_underrun() {
        let frames = fixture_frames();
        let gate = Arc::new(AtomicBool::new(false));
        let reader = StallReader {
            data: Cursor::new(flv_stream(&frames[..60])),
            release: gate.clone(),
        };
        let (mut source, metrics) = LiveAudioSource::open(Box::new(reader), LiveControl::default())
            .expect("open stalling source");

        let mut out = vec![0.0f32; 1024 * 2];
        let mut decoded = 0usize;
        let mut saw_silence = false;
        for _ in 0..400 {
            let read = source.read_frames(&mut out).expect("decode");
            assert!(read > 0, "stalled stream must not report EOF");
            if out[..read * 2].iter().all(|sample| *sample == 0.0) {
                saw_silence = true;
                break;
            }
            decoded += read;
        }
        assert!(saw_silence, "starvation must output silence");
        assert!(decoded > 0, "buffered audio must play before starving");
        assert_eq!(metrics.underruns(), 1, "one starvation episode");
        assert_eq!(metrics.buffer_ms(), 0);
        assert_eq!(metrics.decode_errors(), 0);

        gate.store(true, Ordering::Release);
        wait_until(|| source.queue.is_done(), Duration::from_secs(2));
        // 结束前 WSOLA 可能还有不到一个分析窗（20ms）的尾音，读完再到 EOF。
        let mut tail = 0usize;
        for _ in 0..8 {
            let read = source.read_frames(&mut out).expect("clean EOF");
            if read == 0 {
                break;
            }
            tail += read;
        }
        assert!(tail <= 2_048, "unexpected tail after starvation: {tail}");
        assert_eq!(source.read_frames(&mut out).expect("clean EOF"), 0);
    }

    #[test]
    fn producer_error_surfaces_as_decode_error() {
        let mut bytes = flv_stream(&fixture_frames()[..1]);
        // 半个 tag 头：网络线程读取失败。
        bytes.extend_from_slice(&[8, 0, 0, 0, 0]);
        let (mut source, metrics) = open_source(bytes);

        let mut out = vec![0.0f32; 1024 * 2];
        assert!(source.read_frames(&mut out).expect("first frame") > 0);
        let error = source
            .read_frames(&mut out)
            .expect_err("error must surface");
        assert!(
            matches!(&error, AudioError::Decode(message) if message.contains("直播流中断")),
            "unexpected error: {error}"
        );
        assert_eq!(
            metrics.decode_errors(),
            0,
            "network errors are not decode errors"
        );
    }

    #[test]
    fn source_rejects_a_stream_without_audio_frames() {
        let mut bytes = b"FLV\x01".to_vec();
        bytes.push(4);
        bytes.extend_from_slice(&9u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&tag(8, &ASC_48K_STEREO, 0));
        let error = LiveAudioSource::open(Box::new(Cursor::new(bytes)), LiveControl::default())
            .err()
            .expect("expected an error");
        assert!(matches!(error, LiveError::BadStream(_)), "{error}");
    }

    fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !condition() {
            assert!(Instant::now() < deadline, "condition timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// 读到 `gate_at` 字节后阻塞，直到测试放行；放行后读到 EOF。
    struct GateReader {
        data: Cursor<Vec<u8>>,
        gate_at: usize,
        gate: Arc<AtomicBool>,
    }

    impl Read for GateReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            while (self.data.position() as usize) >= self.gate_at
                && !self.gate.load(Ordering::Acquire)
            {
                thread::sleep(Duration::from_millis(5));
            }
            self.data.read(buf)
        }
    }

    /// 数据读完后阻塞，直到放行才返回 EOF。
    struct StallReader {
        data: Cursor<Vec<u8>>,
        release: Arc<AtomicBool>,
    }

    impl Read for StallReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            loop {
                let read = self.data.read(buf)?;
                if read > 0 {
                    return Ok(read);
                }
                if self.release.load(Ordering::Acquire) {
                    return Ok(0);
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
