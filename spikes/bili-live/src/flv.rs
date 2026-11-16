//! 最小 FLV demux：只保留 type=8 音频 tag，丢弃视频 tag。

use std::io::{ErrorKind, Read};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;

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
