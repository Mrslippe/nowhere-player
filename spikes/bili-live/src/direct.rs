//! B1.2a：direct AAC 解码 + 有界帧队列 + 300ms 目标缓冲控制器。
//!
//! 不接真实音频设备；用已输出 PCM 帧数推进 played 媒体时钟，按 1x 墙钟节流，
//! 以便在 10s 窗口里观察真实 buffer 行为。

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use symphonia::core::audio::Channels;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, Timestamp};
// symphonia::default::codecs::AacDecoder 就是 symphonia_codec_aac::AacDecoder 的 re-export，
// 避免在 Cargo.toml 里新增直接依赖。
use symphonia::default::codecs::AacDecoder;

use crate::aac::AacConfig;
use crate::flv::{RawAacFrame, RawFrameSource, RawNetStats};

pub const TARGET_MS: u64 = 300;
pub const MAX_MS: u64 = 800;
pub const MIN_MS: u64 = 100;
pub const CONTROL_MS: u64 = 250;
pub const COOLDOWN_MS: u64 = 500;
pub const PREBUFFER_MS: u64 = 300;
pub const QUEUE_CAP_FRAMES: usize = 512;
const STARTUP_TIMEOUT: StdDuration = StdDuration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct PcmStats {
    pub frames: u64,
    pub seconds: f64,
    pub packets: u64,
    pub decode_errors: u64,
    pub rms: f64,
    pub peak: f64,
}

#[derive(Debug, Clone)]
pub struct DirectResult {
    pub pcm: PcmStats,
    pub first_pcm_at: Option<Instant>,
    pub decoder_init_ms: u128,
    pub decode_cpu_us: u128,
    pub playback_ms: u128,
    pub wall_ms: u128,
    pub queue_peak_frames: usize,
    pub queue_peak_bytes: usize,
    pub queue_peak_span_ms: u64,
    pub queue_final_frames: usize,
    pub max_queue_wait_ms: u128,
    pub no_drop: bool,
    pub drop_events: u64,
    pub dropped_frames: u64,
    pub dropped_bytes: u64,
    pub dropped_media_ms: u64,
    pub startup_trim_frames: u64,
    pub startup_trim_ms: u64,
    pub underrun_events: u64,
    pub underrun_ms: u64,
    pub stale_frames: u64,
    pub control_ticks: u64,
    pub avg_buffer_ms: f64,
    pub max_buffer_ms: u64,
    pub min_buffer_ms: u64,
    pub producer_overflow_waits: u64,
    pub producer_error: Option<String>,
    pub last_error: Option<String>,
    pub net: RawNetStats,
}

struct QueueState {
    frames: VecDeque<RawAacFrame>,
    newest_pts: Option<u64>,
    queued_bytes: usize,
    done: bool,
    stop: bool,
    error: Option<String>,
    pushed_frames: u64,
    pushed_bytes: u64,
    peak_frames: usize,
    peak_bytes: usize,
    peak_span_ms: u64,
    overflow_waits: u64,
}

struct SharedQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

impl SharedQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                frames: VecDeque::new(),
                newest_pts: None,
                queued_bytes: 0,
                done: false,
                stop: false,
                error: None,
                pushed_frames: 0,
                pushed_bytes: 0,
                peak_frames: 0,
                peak_bytes: 0,
                peak_span_ms: 0,
                overflow_waits: 0,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn spawn_producer(mut source: RawFrameSource, queue: Arc<SharedQueue>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match source.next_frame() {
                Ok(Some(frame)) => {
                    let mut st = queue.lock();
                    while st.frames.len() >= QUEUE_CAP_FRAMES && !st.stop {
                        st.overflow_waits += 1;
                        let (guard, _) = queue
                            .cv
                            .wait_timeout(st, StdDuration::from_millis(20))
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        st = guard;
                    }
                    if st.stop {
                        break;
                    }
                    let len = frame.raw.len();
                    st.newest_pts = Some(frame.pts_ms);
                    st.queued_bytes += len;
                    st.pushed_frames += 1;
                    st.pushed_bytes += len as u64;
                    st.frames.push_back(frame);
                    st.peak_frames = st.peak_frames.max(st.frames.len());
                    st.peak_bytes = st.peak_bytes.max(st.queued_bytes);
                    if let Some(front) = st.frames.front()
                        && let Some(newest) = st.newest_pts
                    {
                        st.peak_span_ms = st.peak_span_ms.max(newest.saturating_sub(front.pts_ms));
                    }
                    queue.cv.notify_all();
                }
                Ok(None) => break,
                Err(e) => {
                    let mut st = queue.lock();
                    st.error = Some(e.to_string());
                    break;
                }
            }
        }
        let mut st = queue.lock();
        st.done = true;
        queue.cv.notify_all();
    })
}

fn build_decoder(cfg: &AacConfig) -> Result<AacDecoder, String> {
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(cfg.sample_rate)
        .with_channels(Channels::Discrete(u16::from(cfg.channels)))
        .with_extra_data(cfg.asc.clone().into_boxed_slice());
    AacDecoder::try_new(&params, &AudioDecoderOptions::default().gapless(false))
        .map_err(|e| format!("AacDecoder::try_new failed: {e}"))
}

fn packet_for(frame: &RawAacFrame) -> Packet {
    Packet::new(
        0,
        Timestamp::new(frame.pts_ms as i64),
        Duration::new(1024),
        frame.raw.clone(),
    )
}

/// 运行 direct 路径：producer 线程读 FLV → 有界队列；当前线程做 1x 播放与控制器。
pub fn run(source: RawFrameSource, seconds: u64) -> Result<DirectResult, String> {
    let cfg = source.config().clone();
    let opened_at = source.opened_at;
    let net_handle = source.stats_handle();
    let no_drop = std::env::var_os("NP_DIRECT_NO_DROP").is_some();

    let decoder_started = Instant::now();
    let mut decoder = build_decoder(&cfg)?;
    let decoder_init_ms = decoder_started.elapsed().as_millis();

    let queue = Arc::new(SharedQueue::new());
    let producer = spawn_producer(source, queue.clone());

    // 启动预缓冲：等 300ms，或超时/EOF 后用现有数据启动。
    let wait_started = Instant::now();
    let mut startup_trim_frames = 0u64;
    let mut startup_trim_ms = 0u64;
    let mut played_pts;
    {
        let mut st = queue.lock();
        loop {
            if st.done && st.frames.is_empty() {
                break;
            }
            if let (Some(newest), Some(front)) = (st.newest_pts, st.frames.front()) {
                let span = newest.saturating_sub(front.pts_ms);
                if span >= PREBUFFER_MS || wait_started.elapsed() >= STARTUP_TIMEOUT {
                    break;
                }
            } else if st.done {
                break;
            }
            let (guard, _) = queue
                .cv
                .wait_timeout(st, StdDuration::from_millis(20))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            st = guard;
        }

        if st.frames.is_empty() {
            drop(st);
            {
                let mut st = queue.lock();
                st.stop = true;
                queue.cv.notify_all();
            }
            let _ = producer.join();
            return Err("no raw AAC frames arrived during startup".to_string());
        }

        let newest = st.newest_pts.unwrap_or(0);
        let front = st.frames.front().unwrap().pts_ms;
        if newest.saturating_sub(front) > TARGET_MS {
            // 启动裁剪：只保留约 300ms 新音频；单独计数，不算运行期 drop。
            let threshold = newest.saturating_sub(TARGET_MS);
            while let Some(f) = st.frames.front() {
                if f.pts_ms >= threshold {
                    break;
                }
                let f = st.frames.pop_front().expect("front checked");
                st.queued_bytes = st.queued_bytes.saturating_sub(f.raw.len());
                startup_trim_frames += 1;
            }
            startup_trim_ms = threshold.saturating_sub(front);
            played_pts = threshold as f64;
        } else {
            played_pts = front as f64;
        }
    }

    let playback_started = Instant::now();
    let mut next_output_at = playback_started;
    let mut last_control = playback_started
        .checked_sub(StdDuration::from_millis(CONTROL_MS))
        .unwrap_or(playback_started);
    let mut last_drop = playback_started
        .checked_sub(StdDuration::from_millis(COOLDOWN_MS))
        .unwrap_or(playback_started);

    let mut pcm = PcmStats::default();
    let mut first_pcm_at = None;
    let mut last_error = None;
    let mut decode_errors = 0u64;
    let mut sum_sq = 0f64;
    let mut sample_count = 0u64;
    let mut peak = 0f64;
    let mut samples: Vec<f32> = Vec::new();
    let mut samples_rate = cfg.sample_rate;

    let mut drop_events = 0u64;
    let mut dropped_frames = 0u64;
    let mut dropped_bytes = 0u64;
    let mut dropped_media_ms = 0u64;
    let mut stale_frames = 0u64;
    let mut max_queue_wait_ms = 0u128;
    let mut decode_cpu_us = 0u128;
    let mut underrun_events = 0u64;
    let mut underrun_ms = 0u64;
    let mut paused = false;
    let mut pause_started: Option<Instant> = None;
    let mut control_ticks = 0u64;
    let mut buffer_sum = 0f64;
    let mut max_buffer_ms = 0u64;
    let mut min_buffer_ms = u64::MAX;

    while playback_started.elapsed() < StdDuration::from_secs(seconds) {
        let now = Instant::now();

        if now.duration_since(last_control) >= StdDuration::from_millis(CONTROL_MS) {
            last_control = now;
            let mut st = queue.lock();
            let newest = st.newest_pts;
            let buffer = newest
                .map(|n| n.saturating_sub(played_pts.max(0.0) as u64))
                .unwrap_or(0);
            control_ticks += 1;
            buffer_sum += buffer as f64;
            max_buffer_ms = max_buffer_ms.max(buffer);
            min_buffer_ms = min_buffer_ms.min(buffer);
            if std::env::var_os("NP_DIRECT_DEBUG").is_some() {
                eprintln!(
                    "t={}ms buf={} played={:.0} newest={:?} q={} paused={} drops={} under={}",
                    playback_started.elapsed().as_millis(),
                    buffer,
                    played_pts,
                    newest,
                    st.frames.len(),
                    paused,
                    drop_events,
                    underrun_events
                );
            }

            if !no_drop
                && buffer > MAX_MS
                && now.duration_since(last_drop) >= StdDuration::from_millis(COOLDOWN_MS)
            {
                let threshold = newest.unwrap_or(0).saturating_sub(TARGET_MS);
                let mut dropped_now = 0u64;
                let mut bytes_now = 0u64;
                while let Some(front) = st.frames.front() {
                    if front.pts_ms >= threshold || st.frames.len() <= 1 {
                        break;
                    }
                    let f = st.frames.pop_front().expect("front checked");
                    st.queued_bytes = st.queued_bytes.saturating_sub(f.raw.len());
                    dropped_now += 1;
                    bytes_now += f.raw.len() as u64;
                }
                let old_played = played_pts;
                if threshold as f64 > played_pts {
                    played_pts = threshold as f64;
                    dropped_media_ms = dropped_media_ms
                        .saturating_add((played_pts - old_played).max(0.0).round() as u64);
                }
                if dropped_now > 0 || threshold as f64 > old_played {
                    drop_events += 1;
                    dropped_frames += dropped_now;
                    dropped_bytes += bytes_now;
                    last_drop = now;
                }
            }

            if buffer < MIN_MS && !paused {
                paused = true;
                pause_started = Some(now);
                underrun_events += 1;
            } else if paused && buffer >= MIN_MS {
                paused = false;
                if let Some(start) = pause_started.take() {
                    underrun_ms =
                        underrun_ms.saturating_add(now.duration_since(start).as_millis() as u64);
                }
            }
        }

        if paused {
            let done = {
                let st = queue.lock();
                st.done && st.frames.is_empty()
            };
            if done {
                break;
            }
            thread::sleep(StdDuration::from_millis(10));
            continue;
        }

        let frame = {
            let mut st = queue.lock();
            let f = st.frames.pop_front();
            if let Some(ref f) = f {
                st.queued_bytes = st.queued_bytes.saturating_sub(f.raw.len());
            }
            f
        };
        let Some(frame) = frame else {
            let done = {
                let st = queue.lock();
                st.done
            };
            if done {
                break;
            }
            if !paused {
                paused = true;
                pause_started = Some(Instant::now());
                underrun_events += 1;
            }
            thread::sleep(StdDuration::from_millis(10));
            continue;
        };

        if (frame.pts_ms as f64) + 1.0 < played_pts {
            stale_frames += 1;
            continue;
        }
        max_queue_wait_ms = max_queue_wait_ms.max(
            Instant::now()
                .saturating_duration_since(frame.arrival)
                .as_millis(),
        );

        let decode_started = Instant::now();
        let packet = packet_for(&frame);
        match decoder.decode(&packet) {
            Ok(buf) => {
                decode_cpu_us += decode_started.elapsed().as_micros();
                let spec = buf.spec();
                samples_rate = spec.rate();
                pcm.packets += 1;
                let frames = buf.frames() as u64;
                pcm.frames += frames;
                buf.copy_to_vec_interleaved::<f32>(&mut samples);
                for s in &samples {
                    let x = f64::from(*s);
                    sum_sq += x * x;
                    let a = x.abs();
                    if a > peak {
                        peak = a;
                    }
                }
                sample_count += samples.len() as u64;
                if samples_rate > 0 {
                    let media_ms = frames as f64 * 1000.0 / f64::from(samples_rate);
                    played_pts += media_ms;
                    next_output_at += StdDuration::from_secs_f64(media_ms / 1000.0);
                }
                if first_pcm_at.is_none() {
                    first_pcm_at = Some(Instant::now());
                }
            }
            Err(e) => {
                decode_cpu_us += decode_started.elapsed().as_micros();
                decode_errors += 1;
                last_error = Some(e.to_string());
            }
        }

        // 1x 播放节流：按输出 PCM 时长推进，不使用被 drop 跳变的 played_pts，
        // 避免“drop 后越等越慢”的正反馈。
        let now = Instant::now();
        if next_output_at > now {
            thread::sleep(next_output_at - now);
        } else if now.saturating_duration_since(next_output_at) > StdDuration::from_millis(100) {
            // 暂停/长阻塞后不回放旧缓冲，避免追赶风暴。
            next_output_at = now;
        }
    }

    {
        let mut st = queue.lock();
        st.stop = true;
        queue.cv.notify_all();
    }
    let _ = producer.join();

    let net = net_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let (peak_frames, peak_bytes, peak_span_ms, final_frames, overflow_waits, done) = {
        let st = queue.lock();
        (
            st.peak_frames,
            st.peak_bytes,
            st.peak_span_ms,
            st.frames.len(),
            st.overflow_waits,
            st.done,
        )
    };

    pcm.decode_errors = decode_errors;
    pcm.seconds = if samples_rate > 0 {
        pcm.frames as f64 / f64::from(samples_rate)
    } else {
        0.0
    };
    pcm.rms = if sample_count > 0 {
        (sum_sq / sample_count as f64).sqrt()
    } else {
        0.0
    };
    pcm.peak = peak;

    if pcm.frames == 0 {
        return Err(format!(
            "direct decode produced no PCM (packets={}, errors={}, last={})",
            pcm.packets,
            decode_errors,
            last_error.unwrap_or_else(|| "none".to_string())
        ));
    }

    Ok(DirectResult {
        pcm,
        first_pcm_at,
        decoder_init_ms,
        decode_cpu_us,
        playback_ms: playback_started.elapsed().as_millis(),
        wall_ms: opened_at.elapsed().as_millis(),
        queue_peak_frames: peak_frames,
        queue_peak_bytes: peak_bytes,
        queue_peak_span_ms: peak_span_ms,
        queue_final_frames: final_frames,
        max_queue_wait_ms,
        no_drop,
        drop_events,
        dropped_frames,
        dropped_bytes,
        dropped_media_ms,
        startup_trim_frames,
        startup_trim_ms,
        underrun_events,
        underrun_ms,
        stale_frames,
        control_ticks,
        avg_buffer_ms: if control_ticks > 0 {
            buffer_sum / control_ticks as f64
        } else {
            0.0
        },
        max_buffer_ms,
        min_buffer_ms: if min_buffer_ms == u64::MAX {
            0
        } else {
            min_buffer_ms
        },
        producer_overflow_waits: overflow_waits,
        producer_error: if done {
            queue.lock().error.clone()
        } else {
            Some("producer still running".to_string())
        },
        last_error,
        net,
    })
}
