//! 直播 source：网络线程产 `AacFrame` → 有界队列 → 本线程 direct `AacDecoder`。
//!
//! 与 B1 的 `AdtsReader + MediaSourceStream(64KB)` 不同，raw AAC 帧直接进
//! `symphonia` 的 `AacDecoder`（ASC 作为 `extra_data`），中间只有 [`FrameQueue`]。
//! 因此缓冲是显式、可观测的：
//!
//! - 启动等到约 2000ms 音频，再从最新帧截到 2000ms，避免把 CDN 初始 GOP 全部播完；
//! - 运行期 `buffer > 2500ms` 时按 AAC 帧边界丢旧帧追到 2000ms；
//! - 队列有数据时持续解码喂 ring；只有队列为空才补静音并记录 underrun；
//! - 暂停时 ring 停止消费，队列自然涨到硬上限；恢复后第一次控制周期追回 900ms。

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
use np_audio::{AudioError, AudioSource};

use crate::error::LiveError;
use crate::flv::{AacConfig, AacFrame, FlvFrameSource};
use crate::metrics::LiveMetrics;
use crate::queue::{
    BufferController, Control, FrameQueue, QUEUE_CAPACITY_FRAMES, TARGET_BUFFER_MS,
};

/// 启动最多等这么久攒够目标缓冲；超时就用手上已有的帧开始播。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
/// AAC-LC 每个 raw frame 固定 1024 个采样。
const AAC_FRAME_SAMPLES: u64 = 1024;
/// 连续解码失败这么多次就认定流不可解，交给 TUI 停播；单帧损坏不致命。
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 30;

/// 直接解码 AAC 的直播 source。
pub(crate) struct LiveAudioSource {
    decoder: AacDecoder,
    queue: FrameQueue,
    metrics: LiveMetrics,
    format: AudioFormat,
    decoded: Vec<f32>,
    offset: usize,
    controller: BufferController,
    consecutive_decode_errors: u32,
}

impl LiveAudioSource {
    /// 读 FLV 头 / AAC 配置、起网络线程、等到约 2000ms 音频后返回。
    ///
    /// 可能阻塞在网络读取上（准备线程专用）。
    pub(crate) fn open(reader: Box<dyn Read + Send>) -> Result<(Self, LiveMetrics), LiveError> {
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
                move || producer_loop(flv, queue)
            })
            .map_err(LiveError::Io)?;

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        queue.wait_span(TARGET_BUFFER_MS, deadline);

        if queue.len() == 0 {
            queue.stop();
            let message = queue
                .error()
                .unwrap_or_else(|| "直播流没有音频帧".to_owned());
            return Err(LiveError::BadStream(message));
        }

        // 启动裁剪：只保留最新的约 2000ms，不把 CDN 初始突发全部播一遍。
        let newest = queue.newest_pts().unwrap_or_default();
        let front = queue.front_pts().unwrap_or_default();
        if newest.saturating_sub(front) > TARGET_BUFFER_MS {
            let threshold = newest.saturating_sub(TARGET_BUFFER_MS);
            queue.drop_below(threshold);
        }
        metrics.set_buffer(queue.buffer_ms());

        Ok((
            Self {
                decoder,
                queue,
                metrics: metrics.clone(),
                format,
                decoded: Vec::new(),
                offset: 0,
                controller: BufferController::new(),
                consecutive_decode_errors: 0,
            },
            metrics,
        ))
    }

    /// 250ms 控制周期：超上限丢旧帧；低于下限由 `note_starved` 记录。
    fn update_control(&mut self) {
        let now = Instant::now();
        let buffer = self.queue.buffer_ms();
        self.metrics.set_buffer(buffer);

        if !self.controller.due(now) {
            return;
        }
        match self.controller.tick(now, buffer) {
            Control::DropTo(target) => {
                let Some(newest) = self.queue.newest_pts() else {
                    return;
                };
                let threshold = newest.saturating_sub(target);
                let dropped = self.queue.drop_below(threshold);
                if dropped > 0 {
                    self.metrics.note_drop(dropped as u64);
                }
                // 丢弃的是压缩帧，解码器状态（重叠窗）保留即可，与 spike 一致。
                self.decoded.clear();
                self.offset = 0;
                self.metrics.set_buffer(self.queue.buffer_ms());
            }
            Control::Recovered | Control::Idle => {}
        }
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
                self.offset = 0;
                self.metrics.set_buffer(self.queue.buffer_ms());
                Ok(())
            }
            Err(error) => {
                self.metrics.note_decode_error();
                self.consecutive_decode_errors += 1;
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
        let mut written = 0;
        while written < max_frames {
            if self.offset < self.decoded.len() {
                let available = (self.decoded.len() - self.offset) / channels;
                let frames = available.min(max_frames - written);
                let start = written * channels;
                let end = start + frames * channels;
                out[start..end]
                    .copy_from_slice(&self.decoded[self.offset..self.offset + frames * channels]);
                self.offset += frames * channels;
                written += frames;
                continue;
            }

            self.update_control();

            // 只要队列里还有帧，就必须持续解码喂 150ms ring。低 buffer 时
            // 绝不能阻塞 decode 线程去等回填，否则 cpal 会先 underrun。
            let Some(frame) = self.queue.pop() else {
                if self.queue.is_done() {
                    self.metrics.set_buffer(0);
                    return match self.queue.error() {
                        Some(error) => Err(AudioError::Decode(format!("直播流中断：{error}"))),
                        None => Ok(written),
                    };
                }
                self.metrics.note_queue_empty();
                return Ok(self.fill_silence(out, channels, written));
            };

            // 队列里只会留下 pts 递增的帧（丢帧只从队头删），不存在过期帧。
            self.decode_frame(frame)?;
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

fn producer_loop(mut flv: FlvFrameSource, queue: FrameQueue) {
    loop {
        match flv.next_frame() {
            Ok(Some(frame)) => {
                if !queue.push(frame) {
                    break;
                }
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
        LiveAudioSource::open(Box::new(Cursor::new(bytes))).expect("open live source")
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

    #[test]
    fn startup_keeps_about_the_target_buffer() {
        let (_source, metrics) = open_source(flv_stream(&fixture_frames()));
        let buffer = metrics.buffer_ms();
        assert!(
            (1_800..=2_200).contains(&buffer),
            "startup buffer {buffer}ms should be about {TARGET_BUFFER_MS}ms"
        );
        assert_eq!(
            metrics.drop_events(),
            0,
            "startup trim is not a runtime drop"
        );
    }

    #[test]
    fn direct_decode_produces_pcm_without_errors() {
        let (mut source, metrics) = open_source(flv_stream(&fixture_frames()));
        assert_eq!(source.format().sample_rate, 48_000);
        assert_eq!(source.format().channels, 2);
        assert_eq!(source.duration_ms(), None);
        assert!(!source.is_seekable());

        let (frames, rms, _samples) = read_until_done(&mut source);
        // 启动截到约 2000ms，后面可能还有网络线程继续灌进来的帧。
        assert!(
            frames > 40_000,
            "expected about a second of PCM, got {frames} frames"
        );
        assert!(rms > 1e-4, "decoded PCM is silent (rms {rms})");
        assert_eq!(metrics.decode_errors(), 0);
        assert!(matches!(source.seek(1_000), Err(AudioError::Seek(_))));
    }

    #[test]
    fn buffer_drops_back_to_target_above_the_maximum() {
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

        let (mut source, metrics) =
            LiveAudioSource::open(Box::new(reader)).expect("open gated source");
        // 闸门前的 60 帧全部到队（启动裁剪只保留最新约 2000ms）。
        wait_until(
            || source.queue.newest_pts().unwrap_or(0) >= 59 * PTS_STEP_MS,
            Duration::from_secs(2),
        );
        let newest_before_gate = source.queue.newest_pts().unwrap();
        assert!(newest_before_gate < 60 * PTS_STEP_MS);

        gate.store(true, Ordering::Release);
        wait_until(
            || source.queue.newest_pts().unwrap_or(0) >= 150 * PTS_STEP_MS,
            Duration::from_secs(2),
        );

        let mut out = vec![0.0f32; 1024 * 2];
        let read = source.read_frames(&mut out).expect("decode");
        assert!(read > 0);
        assert_eq!(metrics.drop_events(), 1, "one drop to target");
        assert!(metrics.dropped_frames() >= 60);
        let buffer = metrics.buffer_ms();
        assert!(
            (1_800..=2_200).contains(&buffer),
            "buffer after drop {buffer}ms"
        );
    }

    #[test]
    fn starvation_fills_silence_and_counts_one_underrun() {
        let frames = fixture_frames();
        let gate = Arc::new(AtomicBool::new(false));
        let reader = StallReader {
            data: Cursor::new(flv_stream(&frames[..60])),
            release: gate.clone(),
        };
        let (mut source, metrics) =
            LiveAudioSource::open(Box::new(reader)).expect("open stalling source");

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
        let error = LiveAudioSource::open(Box::new(Cursor::new(bytes)))
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
