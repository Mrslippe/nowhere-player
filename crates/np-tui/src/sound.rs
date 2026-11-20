//! UI 音效：Windows `PlaySoundW` + 内存合成 WAV；零 crate、零资源文件。
//!
//! 4 个音色在首次使用时由 Rust 合成（44.1kHz / mono / 16-bit），非 Windows
//! 平台只保留数据与开关，不发声。音效只做短促提示，不参与播放状态。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 合成与 WAV 头统一使用的采样率。
pub(crate) const SAMPLE_RATE: u32 = 44_100;

/// 连按去抖窗口：35ms 内的第二次提示不再发声。
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(35);

/// attack 包络长度；其余部分是线性 decay。
const ATTACK_MS: u32 = 2;

#[cfg(windows)]
/// `SND_ASYNC | SND_NODEFAULT | SND_MEMORY`：异步、不落默认提示音、播放内存 WAV。
const PLAY_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004;

/// 4 个 UI 音色；只表示语义，不携带播放状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoundCue {
    /// 行 / 视图移动。
    Move,
    /// 焦点切换。
    Focus,
    /// 确认（Enter）。
    Confirm,
    /// 返回（Esc）。
    Back,
}

/// 音效播放器：保存开关与上次发声时间，35ms 去抖。
pub(crate) struct SoundPlayer {
    enabled: bool,
    last_played: Option<Instant>,
}

impl SoundPlayer {
    /// 默认开启；启动时先把 4 个 WAV 合成好，按键路径上不再付合成成本。
    pub(crate) fn new() -> Self {
        let _ = wav_for(SoundCue::Move);
        Self {
            enabled: true,
            last_played: None,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// 这个时刻是否允许发声；允许时占用去抖窗口。
    ///
    /// 单测用显式 `now` 验证 35ms 去抖，不需要真的睡眠或播放。
    pub(crate) fn should_play_at(&mut self, now: Instant) -> bool {
        if !self.enabled {
            return false;
        }
        if self
            .last_played
            .is_some_and(|last| now.saturating_duration_since(last) < DEBOUNCE)
        {
            return false;
        }
        self.last_played = Some(now);
        true
    }

    /// 播放一个提示音；关闭或去抖窗口内静默。
    pub(crate) fn play(&mut self, cue: SoundCue) {
        if self.should_play_at(Instant::now()) {
            emit(cue);
        }
    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
#[link(name = "winmm")]
unsafe extern "system" {
    /// `SND_MEMORY` 时 `pszSound` 指向内存中的 WAV 字节。
    fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, flags: u32) -> i32;
}

fn emit(cue: SoundCue) {
    #[cfg(windows)]
    {
        if !cfg!(test) {
            let wav = wav_for(cue);
            // SAFETY: WAV 字节放在模块级 `OnceLock` 里，第一次合成后不再修改、
            // 活到进程结束；`SND_ASYNC` 异步播放期间只读这块内存，不会悬垂。
            unsafe {
                let _ = PlaySoundW(wav.as_ptr().cast::<u16>(), std::ptr::null_mut(), PLAY_FLAGS);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cue;
    }
}

/// 一次合成 4 个音色并缓存在 `OnceLock` 中。
fn wav_for(cue: SoundCue) -> &'static [u8] {
    static BANK: OnceLock<SoundBank> = OnceLock::new();
    let bank = BANK.get_or_init(SoundBank::synthesise);
    match cue {
        SoundCue::Move => &bank.move_,
        SoundCue::Focus => &bank.focus,
        SoundCue::Confirm => &bank.confirm,
        SoundCue::Back => &bank.back,
    }
}

struct SoundBank {
    move_: Vec<u8>,
    focus: Vec<u8>,
    confirm: Vec<u8>,
    back: Vec<u8>,
}

impl SoundBank {
    fn synthesise() -> Self {
        Self {
            // 低振幅、短促：峰值约为 i16 上限的 14%–20%。
            move_: tone_wav(&[Tone::new(1_800.0, 12)], 0.14),
            focus: tone_wav(&[Tone::new(1_200.0, 18)], 0.18),
            // 确认音两段：880Hz → 1320Hz，各 35ms，共约 70ms。
            confirm: tone_wav(&[Tone::new(880.0, 35), Tone::new(1_320.0, 35)], 0.20),
            back: tone_wav(&[Tone::new(600.0, 18)], 0.18),
        }
    }
}

struct Tone {
    hz: f32,
    milliseconds: u32,
}

impl Tone {
    const fn new(hz: f32, milliseconds: u32) -> Self {
        Self { hz, milliseconds }
    }
}

fn tone_wav(tones: &[Tone], amplitude: f32) -> Vec<u8> {
    let mut samples = Vec::new();
    for tone in tones {
        append_tone(&mut samples, tone.hz, tone.milliseconds, amplitude);
    }
    wav_bytes(&samples)
}

fn append_tone(samples: &mut Vec<i16>, hz: f32, milliseconds: u32, amplitude: f32) {
    let frames = frames_for(milliseconds);
    let attack = frames_for(ATTACK_MS).max(1);
    let decay = frames.saturating_sub(attack).max(1);
    for index in 0..frames {
        let envelope = if index < attack {
            index as f32 / attack as f32
        } else {
            1.0 - (index - attack) as f32 / decay as f32
        };
        let wave = (std::f32::consts::TAU * hz * index as f32 / SAMPLE_RATE as f32).sin();
        let value = amplitude * envelope * wave * f32::from(i16::MAX);
        samples.push(value.round().clamp(-32_767.0, 32_767.0) as i16);
    }
}

fn frames_for(milliseconds: u32) -> usize {
    let frames = SAMPLE_RATE as f32 * milliseconds as f32 / 1000.0;
    frames.round() as usize
}

/// 44 字节 RIFF/WAVE 头 + PCM data；44.1kHz、mono、16-bit。
fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    #[test]
    fn every_cue_is_a_pcm_riff_wav_with_matching_data_size() {
        for (cue, milliseconds) in [
            (SoundCue::Move, 12),
            (SoundCue::Focus, 18),
            (SoundCue::Confirm, 70),
            (SoundCue::Back, 18),
        ] {
            let bytes = wav_for(cue);
            assert_eq!(&bytes[0..4], b"RIFF", "{cue:?}");
            assert_eq!(&bytes[8..12], b"WAVE", "{cue:?}");
            assert_eq!(&bytes[12..16], b"fmt ", "{cue:?}");
            assert_eq!(u32_at(bytes, 16), 16, "{cue:?} fmt size");
            assert_eq!(u16_at(bytes, 20), 1, "{cue:?} PCM");
            assert_eq!(u16_at(bytes, 22), 1, "{cue:?} mono");
            assert_eq!(u32_at(bytes, 24), SAMPLE_RATE, "{cue:?} sample rate");
            assert_eq!(u32_at(bytes, 28), SAMPLE_RATE * 2, "{cue:?} byte rate");
            assert_eq!(u16_at(bytes, 32), 2, "{cue:?} block align");
            assert_eq!(u16_at(bytes, 34), 16, "{cue:?} bits");
            assert_eq!(&bytes[36..40], b"data", "{cue:?}");
            assert_eq!(
                u32_at(bytes, 4) as usize + 8,
                bytes.len(),
                "{cue:?} RIFF size"
            );
            let data_len = u32_at(bytes, 40) as usize;
            assert_eq!(data_len, bytes.len() - 44, "{cue:?} data size");
            assert_eq!(data_len % 2, 0, "{cue:?} 16-bit samples");
            let actual_ms = (data_len / 2) as f32 * 1000.0 / SAMPLE_RATE as f32;
            assert!(
                (actual_ms - milliseconds as f32).abs() <= 1.0,
                "{cue:?} {actual_ms} ms"
            );
        }
    }

    #[test]
    fn samples_are_audible_but_far_from_clipping() {
        for cue in [
            SoundCue::Move,
            SoundCue::Focus,
            SoundCue::Confirm,
            SoundCue::Back,
        ] {
            let bytes = wav_for(cue);
            let peak = bytes[44..]
                .chunks(2)
                .map(|pair| i32::from(i16::from_le_bytes([pair[0], pair[1]])).abs())
                .max()
                .unwrap_or(0);
            assert!(peak > 0, "{cue:?} must not be silent");
            assert!(peak < 10_000, "{cue:?} peak {peak} too close to i16::MAX");
        }
    }

    #[test]
    fn player_debounces_within_35ms() {
        let mut player = SoundPlayer::new();
        let start = Instant::now();
        assert!(player.should_play_at(start));
        assert!(!player.should_play_at(start + Duration::from_millis(34)));
        assert!(player.should_play_at(start + DEBOUNCE));
        assert!(!player.should_play_at(start + DEBOUNCE));
    }

    #[test]
    fn disabled_player_stays_silent_and_reenables() {
        let mut player = SoundPlayer::new();
        let start = Instant::now();
        player.set_enabled(false);
        assert!(!player.should_play_at(start));
        assert!(!player.should_play_at(start + Duration::from_secs(1)));
        player.set_enabled(true);
        assert!(player.should_play_at(start + Duration::from_secs(2)));
        // 播放路径在单测里只走去抖判断，不真的发声。
        player.play(SoundCue::Confirm);
    }
}
