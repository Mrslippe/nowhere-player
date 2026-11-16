//! 最小 FLV demux：只保留 type=8 音频 tag，丢弃视频 tag。

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use symphonia::core::io::MediaSource;

use crate::aac::AacConfig;
use crate::api::{ORIGIN, REFERER, StageError};

const TARGET_AAC_FRAMES: u32 = 400;
const MAX_WALL: Duration = Duration::from_secs(14);
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub bytes_total: u64,
    pub flv_flags: u8,
    pub tags_total: u64,
    pub audio_tags: u64,
    pub video_tags: u64,
    pub script_tags: u64,
    pub other_tags: u64,
    pub audio_payload_bytes: u64,
    pub video_payload_bytes: u64,
    pub script_payload_bytes: u64,
    pub sequence_headers: u64,
    pub raw_audio_frames: u64,
    pub frames_before_asc: u64,
    pub non_aac_audio_tags: u64,
    pub malformed_audio_tags: u64,
    pub unknown_aac_packet_type: u64,
    pub adts_bytes: u64,
    pub first_audio_ts_ms: Option<u32>,
    pub last_audio_ts_ms: Option<u32>,
    pub eof_before_target: bool,
    pub wall_limit_hit: bool,
    pub byte_budget_hit: bool,
}

#[derive(Debug)]
pub struct Capture {
    pub config: AacConfig,
    pub adts: Vec<u8>,
    pub frames: u32,
    pub stats: Stats,
    pub connect_ms: u128,
    pub read_ms: u128,
    pub elapsed_ms: u128,
}

struct AudioAccumulator {
    config: Option<AacConfig>,
    adts: Vec<u8>,
    frames: u32,
    first_ts: Option<u32>,
    last_ts: Option<u32>,
}

impl AudioAccumulator {
    fn new() -> Self {
        Self {
            config: None,
            adts: Vec::new(),
            frames: 0,
            first_ts: None,
            last_ts: None,
        }
    }

    fn push(&mut self, data: &[u8], ts: u32, stats: &mut Stats) -> Result<(), StageError> {
        if data.len() < 2 {
            stats.malformed_audio_tags += 1;
            return Ok(());
        }
        let sound_format = data[0] >> 4;
        if sound_format != 10 {
            stats.non_aac_audio_tags += 1;
            return Ok(());
        }
        match data[1] {
            0 => {
                stats.sequence_headers += 1;
                if self.config.is_none() {
                    let asc = &data[2..];
                    let cfg = AacConfig::parse(asc).map_err(|e| {
                        StageError::new(
                            "ADTS",
                            format!("invalid AAC AudioSpecificConfig ({} bytes): {e}", asc.len()),
                        )
                    })?;
                    self.config = Some(cfg);
                }
            }
            1 => {
                let Some(cfg) = &self.config else {
                    stats.frames_before_asc += 1;
                    return Ok(());
                };
                let raw = &data[2..];
                let header = cfg
                    .adts_header(raw.len())
                    .map_err(|e| StageError::new("ADTS", e))?;
                self.adts.extend_from_slice(&header);
                self.adts.extend_from_slice(raw);
                self.frames += 1;
                self.first_ts.get_or_insert(ts);
                self.last_ts = Some(ts);
                stats.raw_audio_frames += 1;
                stats.adts_bytes = self.adts.len() as u64;
            }
            other => {
                stats.unknown_aac_packet_type += 1;
                if stats.unknown_aac_packet_type <= 3 {
                    eprintln!("note: unknown AAC packet type {other} in audio tag");
                }
            }
        }
        Ok(())
    }
}

fn read_upto<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match reader.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn u24(b: &[u8]) -> usize {
    (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2])
}

fn flv_err(msg: impl Into<String>) -> StageError {
    StageError::new("FLV", msg)
}

/// 打开直播 FLV，读取到 400 个 AAC 帧 / 14 秒上限，返回 ADTS 字节流。
pub fn capture(client: &Client, url: &str) -> Result<Capture, StageError> {
    let started = Instant::now();
    let resp = client
        .get(url)
        .header("Referer", REFERER)
        .header("Origin", ORIGIN)
        .header("Accept", "*/*")
        .send()
        .map_err(|e| flv_err(format!("open stream: {}", e.without_url())))?;
    let connect_ms = started.elapsed().as_millis();

    let status = resp.status();
    if !status.is_success() {
        return Err(flv_err(format!("flv http status {status}")));
    }

    let mut stats = Stats::default();
    let mut reader = resp;

    let mut header = [0u8; 9];
    let n =
        read_upto(&mut reader, &mut header).map_err(|e| flv_err(format!("read header: {e}")))?;
    if n != 9 {
        return Err(flv_err(format!("short FLV header: {n} bytes")));
    }
    if &header[0..3] != b"FLV" {
        return Err(flv_err(format!(
            "not an FLV stream (magic {:02x?})",
            &header[0..3]
        )));
    }
    stats.flv_flags = header[4];
    let data_offset = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if !(9..=4096).contains(&data_offset) {
        return Err(flv_err(format!("bad FLV data offset {data_offset}")));
    }
    if data_offset > 9 {
        let mut skip = vec![0u8; data_offset - 9];
        let n = read_upto(&mut reader, &mut skip)
            .map_err(|e| flv_err(format!("read extended header: {e}")))?;
        if n != skip.len() {
            return Err(flv_err("short extended FLV header"));
        }
    }
    let mut prev_size = [0u8; 4];
    let n = read_upto(&mut reader, &mut prev_size)
        .map_err(|e| flv_err(format!("read PreviousTagSize0: {e}")))?;
    if n != 4 {
        return Err(flv_err("short PreviousTagSize0"));
    }

    let mut acc = AudioAccumulator::new();
    let mut tag_header = [0u8; 11];
    let mut data: Vec<u8> = Vec::new();
    let mut total_read: u64 = (data_offset + 4) as u64;

    loop {
        if acc.frames >= TARGET_AAC_FRAMES {
            break;
        }
        if started.elapsed() >= MAX_WALL {
            stats.wall_limit_hit = true;
            break;
        }
        if total_read > MAX_TOTAL_BYTES {
            stats.byte_budget_hit = true;
            break;
        }

        let n = read_upto(&mut reader, &mut tag_header)
            .map_err(|e| flv_err(format!("read tag header: {e}")))?;
        if n == 0 {
            stats.eof_before_target = true;
            break;
        }
        if n != 11 {
            return Err(flv_err(format!("truncated FLV tag header: {n}/11 bytes")));
        }

        let tag_type = tag_header[0];
        let data_size = u24(&tag_header[1..4]);
        let ts = (u24(&tag_header[4..7]) as u32) | ((tag_header[7] as u32) << 24);
        total_read += 11;

        if data_size > 2 * 1024 * 1024 {
            return Err(flv_err(format!("implausible FLV tag size {data_size}")));
        }
        data.resize(data_size, 0);
        let n = read_upto(&mut reader, &mut data)
            .map_err(|e| flv_err(format!("read tag payload: {e}")))?;
        if n != data_size {
            return Err(flv_err(format!(
                "truncated FLV tag payload: {n}/{data_size} bytes (type {tag_type})"
            )));
        }
        total_read += data_size as u64;

        // PreviousTagSizeN
        let n = read_upto(&mut reader, &mut prev_size)
            .map_err(|e| flv_err(format!("read PreviousTagSize: {e}")))?;
        if n != 4 {
            return Err(flv_err("truncated PreviousTagSize"));
        }
        total_read += 4;

        stats.tags_total += 1;
        match tag_type {
            8 => {
                stats.audio_tags += 1;
                stats.audio_payload_bytes += data_size as u64;
                acc.push(&data, ts, &mut stats)?;
            }
            9 => {
                // 视频只计数，绝不进入解码链路。
                stats.video_tags += 1;
                stats.video_payload_bytes += data_size as u64;
            }
            18 => {
                stats.script_tags += 1;
                stats.script_payload_bytes += data_size as u64;
            }
            _ => stats.other_tags += 1,
        }
    }

    stats.bytes_total = total_read;
    stats.first_audio_ts_ms = acc.first_ts;
    stats.last_audio_ts_ms = acc.last_ts;
    stats.adts_bytes = acc.adts.len() as u64;

    let config = acc.config.ok_or_else(|| {
        StageError::new(
            "ADTS",
            format!(
                "no AAC sequence header in {} audio tags (non_aac_audio_tags={})",
                stats.audio_tags, stats.non_aac_audio_tags
            ),
        )
    })?;
    if acc.frames == 0 || acc.adts.is_empty() {
        return Err(StageError::new(
            "ADTS",
            format!(
                "no raw AAC frames wrapped (frames_before_asc={}, audio_tags={})",
                stats.frames_before_asc, stats.audio_tags
            ),
        ));
    }

    Ok(Capture {
        config,
        adts: acc.adts,
        frames: acc.frames,
        stats,
        connect_ms,
        read_ms: started.elapsed().as_millis().saturating_sub(connect_ms),
        elapsed_ms: started.elapsed().as_millis(),
    })
}

/// 流式模式统计：网络读取、FLV tag 计数、缓冲水位与阻塞观测。
#[derive(Debug, Default, Clone)]
pub struct StreamStats {
    pub bytes_total: u64,
    pub flv_flags: u8,
    pub tags_total: u64,
    pub audio_tags: u64,
    pub video_tags: u64,
    pub script_tags: u64,
    pub other_tags: u64,
    pub audio_payload_bytes: u64,
    pub video_payload_bytes: u64,
    pub script_payload_bytes: u64,
    pub sequence_headers: u64,
    pub raw_audio_frames: u64,
    pub frames_before_asc: u64,
    pub non_aac_audio_tags: u64,
    pub malformed_audio_tags: u64,
    pub unknown_aac_packet_type: u64,
    pub adts_bytes: u64,
    pub asc_hex: Option<String>,
    pub aac_aot: Option<u8>,
    pub aac_sample_rate: Option<u32>,
    pub aac_channels: Option<u8>,
    pub max_pending_bytes: usize,
    pub max_tag_payload_bytes: usize,
    pub tag_reads: u64,
    pub total_tag_read_ms: u128,
    pub max_tag_read_ms: u128,
    pub slow_tag_reads_gt_100ms: u64,
    pub max_tag_gap_ms: u128,
    pub first_audio_frame_at: Option<Instant>,
    pub last_audio_frame_at: Option<Instant>,
    pub first_audio_ts_ms: Option<u32>,
    pub last_audio_ts_ms: Option<u32>,
    pub underruns: u64,
    pub eof: bool,
    pub fatal: Option<String>,
    pub fatal_stage: Option<&'static str>,
}

pub struct OpenedStream {
    pub source: FlvAdtsSource,
    pub stats: Arc<Mutex<StreamStats>>,
    pub started: Arc<AtomicBool>,
    pub opened_at: Instant,
    pub connect_ms: u128,
    pub header_ms: u128,
}

/// 自定义 `MediaSource`：`Read` 时按需从 HTTP FLV 拉 tag，边收边产出 ADTS 字节。
///
/// 对 symphonia 报告 `byte_len=None` / `is_seekable=false`，避免 AdtsReader 在
/// 构造时做需要 EOF/seek 的时长估算；实际 `AdtsReader` 只顺序读取。
pub struct FlvAdtsSource {
    reader: Mutex<reqwest::blocking::Response>,
    pending: VecDeque<u8>,
    config: Option<AacConfig>,
    deadline: Instant,
    stats: Arc<Mutex<StreamStats>>,
    started: Arc<AtomicBool>,
    eof: bool,
    tag_buf: Vec<u8>,
    last_tag_done: Option<Instant>,
}

fn io_invalid(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// 建立 FLV 连接并读完 header；返回可直接塞给 MediaSourceStream 的 source。
pub fn open_stream_source(
    client: &Client,
    url: &str,
    duration: Duration,
) -> Result<OpenedStream, StageError> {
    let request_started = Instant::now();
    let resp = client
        .get(url)
        .header("Referer", REFERER)
        .header("Origin", ORIGIN)
        .header("Accept", "*/*")
        .send()
        .map_err(|e| flv_err(format!("open stream: {}", e.without_url())))?;
    let opened_at = Instant::now();
    let connect_ms = opened_at.duration_since(request_started).as_millis();

    let status = resp.status();
    if !status.is_success() {
        return Err(flv_err(format!("flv http status {status}")));
    }

    let mut reader = resp;
    let mut header = [0u8; 9];
    let n =
        read_upto(&mut reader, &mut header).map_err(|e| flv_err(format!("read header: {e}")))?;
    if n != 9 {
        return Err(flv_err(format!("short FLV header: {n} bytes")));
    }
    if &header[0..3] != b"FLV" {
        return Err(flv_err(format!(
            "not an FLV stream (magic {:02x?})",
            &header[0..3]
        )));
    }
    let data_offset = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if !(9..=4096).contains(&data_offset) {
        return Err(flv_err(format!("bad FLV data offset {data_offset}")));
    }
    if data_offset > 9 {
        let mut skip = vec![0u8; data_offset - 9];
        let n = read_upto(&mut reader, &mut skip)
            .map_err(|e| flv_err(format!("read extended header: {e}")))?;
        if n != skip.len() {
            return Err(flv_err("short extended FLV header"));
        }
    }
    let mut prev_size = [0u8; 4];
    let n = read_upto(&mut reader, &mut prev_size)
        .map_err(|e| flv_err(format!("read PreviousTagSize0: {e}")))?;
    if n != 4 {
        return Err(flv_err("short PreviousTagSize0"));
    }

    let header_ms = opened_at.elapsed().as_millis();
    let stats = Arc::new(Mutex::new(StreamStats {
        flv_flags: header[4],
        bytes_total: (data_offset + 4) as u64,
        ..Default::default()
    }));
    let started = Arc::new(AtomicBool::new(false));

    Ok(OpenedStream {
        source: FlvAdtsSource {
            reader: Mutex::new(reader),
            pending: VecDeque::new(),
            config: None,
            deadline: opened_at + duration,
            stats: stats.clone(),
            started: started.clone(),
            eof: false,
            tag_buf: Vec::new(),
            last_tag_done: None,
        },
        stats,
        started,
        opened_at,
        connect_ms,
        header_ms,
    })
}

impl FlvAdtsSource {
    /// 读一个 FLV tag；false 表示干净 EOF。音频 tag 的 ADTS 字节进入 pending。
    fn pull_tag(&mut self) -> std::io::Result<bool> {
        let mut reader = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let mut th = [0u8; 11];
        let n = read_upto(&mut *reader, &mut th)?;
        if n == 0 {
            return Ok(false);
        }
        if n != 11 {
            return Err(io_invalid(format!(
                "truncated FLV tag header: {n}/11 bytes"
            )));
        }

        let tag_type = th[0];
        let data_size = u24(&th[1..4]);
        let ts = (u24(&th[4..7]) as u32) | ((th[7] as u32) << 24);
        if data_size > 2 * 1024 * 1024 {
            return Err(io_invalid(format!("implausible FLV tag size {data_size}")));
        }

        self.tag_buf.resize(data_size, 0);
        let n = read_upto(&mut *reader, &mut self.tag_buf)?;
        if n != data_size {
            return Err(io_invalid(format!(
                "truncated FLV tag payload: {n}/{data_size} bytes (type {tag_type})"
            )));
        }
        let mut prev = [0u8; 4];
        let n = read_upto(&mut *reader, &mut prev)?;
        if n != 4 {
            return Err(io_invalid("truncated PreviousTagSize"));
        }
        drop(reader);

        let data = self.tag_buf[..data_size].to_vec();
        let stats = self.stats.clone();
        let mut st = stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        st.tags_total += 1;
        st.bytes_total += (11 + data_size + 4) as u64;
        st.max_tag_payload_bytes = st.max_tag_payload_bytes.max(data_size);

        match tag_type {
            8 => {
                st.audio_tags += 1;
                st.audio_payload_bytes += data_size as u64;
                self.handle_audio_tag(&data, ts, &mut st)?;
            }
            9 => {
                st.video_tags += 1;
                st.video_payload_bytes += data_size as u64;
            }
            18 => {
                st.script_tags += 1;
                st.script_payload_bytes += data_size as u64;
            }
            _ => st.other_tags += 1,
        }

        Ok(true)
    }

    fn handle_audio_tag(
        &mut self,
        data: &[u8],
        ts: u32,
        st: &mut StreamStats,
    ) -> std::io::Result<()> {
        if data.len() < 2 {
            st.malformed_audio_tags += 1;
            return Ok(());
        }
        let sound_format = data[0] >> 4;
        if sound_format != 10 {
            st.non_aac_audio_tags += 1;
            return Ok(());
        }
        match data[1] {
            0 => {
                st.sequence_headers += 1;
                if self.config.is_none() {
                    match AacConfig::parse(&data[2..]) {
                        Ok(cfg) => {
                            st.asc_hex = Some(cfg.asc_hex());
                            st.aac_aot = Some(cfg.aot);
                            st.aac_sample_rate = Some(cfg.sample_rate);
                            st.aac_channels = Some(cfg.channels);
                            self.config = Some(cfg);
                        }
                        Err(e) => {
                            let msg = format!(
                                "invalid AAC AudioSpecificConfig ({} bytes): {e}",
                                data.len().saturating_sub(2)
                            );
                            st.fatal = Some(msg.clone());
                            st.fatal_stage = Some("ADTS");
                            return Err(io_invalid(msg));
                        }
                    }
                }
            }
            1 => {
                let Some(cfg) = &self.config else {
                    st.frames_before_asc += 1;
                    return Ok(());
                };
                let raw = &data[2..];
                let header = match cfg.adts_header(raw.len()) {
                    Ok(h) => h,
                    Err(e) => {
                        st.fatal = Some(e.clone());
                        st.fatal_stage = Some("ADTS");
                        return Err(io_invalid(e));
                    }
                };
                self.pending.extend(header);
                self.pending.extend(raw);
                st.raw_audio_frames += 1;
                st.adts_bytes = st.adts_bytes.saturating_add(7 + raw.len() as u64);
                st.max_pending_bytes = st.max_pending_bytes.max(self.pending.len());
                let now = Instant::now();
                st.first_audio_frame_at.get_or_insert(now);
                st.last_audio_frame_at = Some(now);
                st.first_audio_ts_ms.get_or_insert(ts);
                st.last_audio_ts_ms = Some(ts);
            }
            other => {
                st.unknown_aac_packet_type += 1;
                if st.unknown_aac_packet_type <= 3 {
                    eprintln!("note: unknown AAC packet type {other} in audio tag");
                }
            }
        }
        Ok(())
    }
}

impl Read for FlvAdtsSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        while self.pending.is_empty() && !self.eof {
            if Instant::now() >= self.deadline {
                self.eof = true;
                let stats = self.stats.clone();
                let mut st = stats
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                st.eof = true;
                if self.config.is_none() && st.fatal.is_none() {
                    st.fatal = Some("no AAC sequence header before deadline".to_string());
                    st.fatal_stage = Some("ADTS");
                }
                break;
            }

            let wait_start = Instant::now();
            let result = self.pull_tag();
            let wait_ms = wait_start.elapsed().as_millis();

            let stats = self.stats.clone();
            let mut st = stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            st.tag_reads += 1;
            st.total_tag_read_ms += wait_ms;
            st.max_tag_read_ms = st.max_tag_read_ms.max(wait_ms);
            if wait_ms > 100 {
                st.slow_tag_reads_gt_100ms += 1;
                if self.started.load(Ordering::Relaxed) {
                    st.underruns += 1;
                }
            }
            if let Some(prev) = self.last_tag_done {
                st.max_tag_gap_ms = st.max_tag_gap_ms.max(prev.elapsed().as_millis());
            }
            self.last_tag_done = Some(Instant::now());
            drop(st);

            match result {
                Ok(true) => {}
                Ok(false) => {
                    self.eof = true;
                    let stats = self.stats.clone();
                    let mut st = stats
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    st.eof = true;
                    if self.config.is_none() && st.fatal.is_none() {
                        st.fatal = Some("stream ended before AAC sequence header".to_string());
                        st.fatal_stage = Some("ADTS");
                    }
                    break;
                }
                Err(e) => {
                    self.eof = true;
                    let stats = self.stats.clone();
                    let mut st = stats
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    st.eof = true;
                    if st.fatal.is_none() {
                        st.fatal = Some(e.to_string());
                        st.fatal_stage = Some("FLV");
                    }
                    return Err(e);
                }
            }
        }

        if self.pending.is_empty() {
            return Ok(0);
        }
        let n = buf.len().min(self.pending.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.pending.pop_front().expect("pending is non-empty");
        }
        Ok(n)
    }
}

impl MediaSource for FlvAdtsSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

impl Seek for FlvAdtsSource {
    fn seek(&mut self, _pos: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "live FLV stream is not seekable",
        ))
    }
}
