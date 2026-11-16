//! FLV 里的 AAC：解析 AudioSpecificConfig、补 ADTS 头、交给 symphonia 解码。

use std::io::Cursor;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::default::formats::AdtsReader;

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

#[derive(Debug, Clone)]
pub struct AacConfig {
    pub aot: u8,
    pub sf_index: u8,
    pub sample_rate: u32,
    pub channels: u8,
    pub asc: Vec<u8>,
}

impl AacConfig {
    /// 解析 FLV audio tag 里的 AudioSpecificConfig（ISO 14496-3）。
    pub fn parse(asc: &[u8]) -> Result<Self, String> {
        let mut br = BitReader { data: asc, bit: 0 };
        let aot = br.read(5).ok_or("ASC too short (audioObjectType)")?;
        let aot = if aot == 31 {
            32 + br
                .read(6)
                .ok_or("ASC too short (extended audioObjectType)")?
        } else {
            aot
        };
        if aot == 5 || aot == 29 {
            return Err(format!(
                "unsupported AOT {aot} (HE-AAC/SBR); ADTS profile field cannot express it"
            ));
        }
        if !(1..=4).contains(&aot) {
            return Err(format!("unexpected AOT {aot}, expected 2 (AAC-LC)"));
        }

        let sf_index = br.read(4).ok_or("ASC too short (samplingFrequencyIndex)")? as u8;
        let sample_rate = if sf_index == 15 {
            br.read(24)
                .ok_or("ASC too short (explicit samplingFrequency)")?
        } else {
            *SAMPLE_RATES
                .get(sf_index as usize)
                .ok_or_else(|| format!("invalid samplingFrequencyIndex {sf_index}"))?
        };

        let channels = br.read(4).ok_or("ASC too short (channelConfiguration)")? as u8;
        if channels == 0 {
            return Err("channelConfiguration=0 (PCE-defined) not supported".to_string());
        }
        if channels > 7 {
            return Err(format!("invalid channelConfiguration {channels}"));
        }

        Ok(Self {
            aot: aot as u8,
            sf_index,
            sample_rate,
            channels,
            asc: asc.to_vec(),
        })
    }

    pub fn asc_hex(&self) -> String {
        self.asc.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 7 字节无 CRC 的 ADTS header。
    pub fn adts_header(&self, payload_len: usize) -> Result<[u8; 7], String> {
        let frame_len = payload_len + 7;
        if frame_len > 0x1FFF {
            return Err(format!("AAC frame too large for ADTS: {frame_len} bytes"));
        }
        let profile = self.aot - 1;
        let ch = self.channels;
        let fl = frame_len as u32;
        let mut h = [0u8; 7];
        h[0] = 0xFF;
        h[1] = 0xF1; // MPEG-4, layer 0, no CRC
        h[2] = (profile << 6) | ((self.sf_index & 0x0F) << 2) | ((ch >> 2) & 0x01);
        h[3] = ((ch & 0x03) << 6) | ((fl >> 11) as u8 & 0x03);
        h[4] = ((fl >> 3) & 0xFF) as u8;
        h[5] = (((fl & 0x07) << 5) as u8) | 0x1F;
        h[6] = 0xFC;
        Ok(h)
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl BitReader<'_> {
    fn read(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.bit / 8)?;
            let bit = (byte >> (7 - (self.bit % 8))) & 1;
            v = (v << 1) | u32::from(bit);
            self.bit += 1;
        }
        Some(v)
    }
}

#[derive(Debug, Clone)]
pub struct DecodeResult {
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: u64,
    pub packets: u64,
    pub decode_errors: u64,
    pub rms: f64,
    pub peak: f64,
    pub seconds: f64,
}

pub fn decode(
    adts: &[u8],
    fallback_rate: u32,
    fallback_channels: u8,
) -> Result<DecodeResult, String> {
    let source = Cursor::new(adts.to_vec());
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let mut reader = AdtsReader::try_new(mss, FormatOptions::default())
        .map_err(|e| format!("open ADTS: {e}"))?;

    let track = reader
        .tracks()
        .first()
        .ok_or("ADTS reader returned no track")?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .cloned()
        .ok_or("ADTS track has no audio codec parameters")?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(false))
        .map_err(|e| format!("create AAC decoder: {e}"))?;

    let mut sample_rate = params.sample_rate.unwrap_or(fallback_rate);
    let mut channels = params
        .channels
        .as_ref()
        .map(|c| c.count())
        .filter(|c| *c > 0)
        .unwrap_or(fallback_channels as usize);

    let mut frames = 0u64;
    let mut packets = 0u64;
    let mut decode_errors = 0u64;
    let mut sum_sq = 0f64;
    let mut sample_count = 0u64;
    let mut peak = 0f64;
    let mut last_error: Option<String> = None;
    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(format!("read ADTS packet: {e}")),
        };

        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = buf.spec();
                sample_rate = spec.rate();
                let ch = spec.channels().count();
                if ch > 0 {
                    channels = ch;
                }
                frames += buf.frames() as u64;
                packets += 1;

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
            }
            Err(SymphoniaError::DecodeError(m)) => {
                decode_errors += 1;
                last_error = Some(m.to_string());
                if decode_errors > 16 {
                    break;
                }
            }
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => {
                decode_errors += 1;
                last_error = Some(e.to_string());
                if decode_errors > 16 {
                    break;
                }
            }
        }
    }

    if frames == 0 {
        return Err(format!(
            "no PCM frames decoded (packets={packets}, errors={decode_errors}, last={})",
            last_error.unwrap_or_else(|| "none".to_string())
        ));
    }

    let rms = if sample_count > 0 {
        (sum_sq / sample_count as f64).sqrt()
    } else {
        0.0
    };
    let seconds = if sample_rate > 0 {
        frames as f64 / f64::from(sample_rate)
    } else {
        0.0
    };

    Ok(DecodeResult {
        sample_rate,
        channels,
        frames,
        packets,
        decode_errors,
        rms,
        peak,
        seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aac_lc_48k_stereo() {
        // AOT=2, sf=3 (48k), ch=2
        let cfg = AacConfig::parse(&[0x11, 0x90]).unwrap();
        assert_eq!(cfg.aot, 2);
        assert_eq!(cfg.sf_index, 3);
        assert_eq!(cfg.sample_rate, 48_000);
        assert_eq!(cfg.channels, 2);

        let h = cfg.adts_header(100).unwrap();
        assert_eq!(h[0], 0xFF);
        assert_eq!(h[1], 0xF1);
        assert_eq!(h[2], 0x4C);
        assert_eq!(
            ((h[3] as usize & 0x03) << 11) | ((h[4] as usize) << 3) | ((h[5] as usize) >> 5),
            107
        );
    }
}
