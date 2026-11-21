//! UI 音效：Windows `PlaySoundW` + 内存合成 WAV；零 crate、零资源文件。
//!
//! 4 个音色在首次使用时由 Rust 合成（44.1kHz / mono / 16-bit），全部来自
//! 老式打字机的机械声：确定性 LCG 噪声 + 一阶低通 / 高通 + 指数衰减，再叠
//! 一层低频 thump 或高频金属共振。非 Windows 平台只保留数据与开关，不发声。
//! 音效只做短促提示，不参与播放状态。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 合成与 WAV 头统一使用的采样率。
pub(crate) const SAMPLE_RATE: u32 = 44_100;

/// 连按去抖窗口：35ms 内的第二次提示不再发声。
pub(crate) const DEBOUNCE: Duration = Duration::from_millis(35);

/// 起音包络：1.5ms 线性上升，避免波形从 0 跳到峰值产生爆点。
const ATTACK_SECONDS: f32 = 0.0015;

/// 归一化后的峰值必须落在 i16 上限的 15%–25%：听得见，但不削顶。
const PEAK_LOW: f32 = 0.15;
const PEAK_HIGH: f32 = 0.25;

/// 各 cue 的总时长（毫秒）：按键 25–35ms、确认 100–140ms。
const MOVE_MS: u32 = 32;
const FOCUS_MS: u32 = 21;
const CONFIRM_MS: u32 = 130;
const BACK_MS: u32 = 40;

/// 各 cue 的归一化目标：Focus 最轻，Confirm 最饱满。
const MOVE_PEAK: f32 = 0.20;
const FOCUS_PEAK: f32 = 0.16;
const CONFIRM_PEAK: f32 = 0.22;
const BACK_PEAK: f32 = 0.18;

#[cfg(windows)]
/// `SND_ASYNC | SND_NODEFAULT | SND_MEMORY`：异步、不落默认提示音、播放内存 WAV。
const PLAY_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004;

/// 音效主题：同一组 cue 的不同机械音色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SoundTheme {
    /// 沉闷的扳动按钮声，默认。
    #[default]
    Switch,
    /// 原有打字机音色，完整保留。
    Typewriter,
    /// 更轻、更短的低频点击。
    Soft,
}

impl SoundTheme {
    pub(crate) const ALL: [Self; 3] = [Self::Switch, Self::Typewriter, Self::Soft];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Switch => "switch",
            Self::Typewriter => "typewriter",
            Self::Soft => "soft",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Switch => "扳动开关",
            Self::Typewriter => "打字机",
            Self::Soft => "轻触",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "switch" => Some(Self::Switch),
            "typewriter" => Some(Self::Typewriter),
            "soft" => Some(Self::Soft),
            _ => None,
        }
    }
}

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

/// 音效播放器：保存开关、主题与上次发声时间，35ms 去抖。
pub(crate) struct SoundPlayer {
    enabled: bool,
    theme: SoundTheme,
    last_played: Option<Instant>,
}

impl SoundPlayer {
    /// 默认开启；启动时先把默认主题的 Move WAV 合成好。
    pub(crate) fn new() -> Self {
        let theme = SoundTheme::default();
        let _ = wav_for(theme, SoundCue::Move);
        Self {
            enabled: true,
            theme,
            last_played: None,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(crate) fn set_theme(&mut self, theme: SoundTheme) {
        self.theme = theme;
        // 预合成，避免第一次切主题时在按键路径上卡一下。
        let _ = wav_for(theme, SoundCue::Move);
    }

    #[cfg(test)]
    pub(crate) fn theme(&self) -> SoundTheme {
        self.theme
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
            emit(cue, self.theme);
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

fn emit(cue: SoundCue, theme: SoundTheme) {
    #[cfg(windows)]
    {
        if !cfg!(test) {
            let wav = wav_for(theme, cue);
            // SAFETY: WAV 字节放在模块级 `OnceLock` 里，第一次合成后不再修改、
            // 活到进程结束；`SND_ASYNC` 异步播放期间只读这块内存，不会悬垂。
            unsafe {
                let _ = PlaySoundW(wav.as_ptr().cast::<u16>(), std::ptr::null_mut(), PLAY_FLAGS);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (cue, theme);
    }
}

/// 三个主题各合成 4 个音色并缓存在 `OnceLock` 中。
fn wav_for(theme: SoundTheme, cue: SoundCue) -> &'static [u8] {
    static BANKS: OnceLock<SoundBanks> = OnceLock::new();
    let banks = BANKS.get_or_init(SoundBanks::synthesise);
    let bank = match theme {
        SoundTheme::Switch => &banks.switch,
        SoundTheme::Typewriter => &banks.typewriter,
        SoundTheme::Soft => &banks.soft,
    };
    match cue {
        SoundCue::Move => &bank.move_,
        SoundCue::Focus => &bank.focus,
        SoundCue::Confirm => &bank.confirm,
        SoundCue::Back => &bank.back,
    }
}

struct SoundBanks {
    switch: SoundBank,
    typewriter: SoundBank,
    soft: SoundBank,
}

impl SoundBanks {
    fn synthesise() -> Self {
        Self {
            switch: SoundBank::switch(),
            typewriter: SoundBank::typewriter(),
            soft: SoundBank::soft(),
        }
    }
}

struct SoundBank {
    move_: Vec<u8>,
    focus: Vec<u8>,
    confirm: Vec<u8>,
    back: Vec<u8>,
}

impl SoundBank {
    fn typewriter() -> Self {
        Self {
            // 键帽咔哒：宽带噪声点击 + 2.4kHz 金属短鸣。
            move_: typewriter_wav(
                MOVE_MS,
                MOVE_PEAK,
                &[Hit {
                    start_ms: 0,
                    milliseconds: MOVE_MS,
                    gain: 1.0,
                    lowpass_hz: 6_500.0,
                    highpass_hz: 1_400.0,
                    resonance_hz: 2_400.0,
                    resonance_gain: 0.5,
                    decay_ms: 5.0,
                    seed: 0x00C0_FFEE,
                }],
            ),
            // 轻触：更短、更细，中高频为主。
            focus: typewriter_wav(
                FOCUS_MS,
                FOCUS_PEAK,
                &[Hit {
                    start_ms: 0,
                    milliseconds: FOCUS_MS,
                    gain: 1.0,
                    lowpass_hz: 9_000.0,
                    highpass_hz: 2_600.0,
                    resonance_hz: 3_200.0,
                    resonance_gain: 0.4,
                    decay_ms: 3.2,
                    seed: 0x0FAC_0DE5,
                }],
            ),
            // 回车：三下字车棘轮 + 55ms 起的 2.8kHz 打字机铃，总长 130ms。
            confirm: typewriter_wav(
                CONFIRM_MS,
                CONFIRM_PEAK,
                &[
                    Hit {
                        start_ms: 0,
                        milliseconds: 12,
                        gain: 0.8,
                        lowpass_hz: 7_000.0,
                        highpass_hz: 1_600.0,
                        resonance_hz: 1_900.0,
                        resonance_gain: 0.5,
                        decay_ms: 3.0,
                        seed: 0x1A2B_3C4D,
                    },
                    Hit {
                        start_ms: 20,
                        milliseconds: 12,
                        gain: 0.7,
                        lowpass_hz: 7_000.0,
                        highpass_hz: 1_600.0,
                        resonance_hz: 1_900.0,
                        resonance_gain: 0.5,
                        decay_ms: 3.0,
                        seed: 0x2B3C_4D5E,
                    },
                    Hit {
                        start_ms: 40,
                        milliseconds: 12,
                        gain: 0.75,
                        lowpass_hz: 7_000.0,
                        highpass_hz: 1_600.0,
                        resonance_hz: 1_900.0,
                        resonance_gain: 0.5,
                        decay_ms: 3.0,
                        seed: 0x3C4D_5E6F,
                    },
                    Hit {
                        start_ms: 55,
                        milliseconds: 75,
                        gain: 1.0,
                        lowpass_hz: 8_000.0,
                        highpass_hz: 1_800.0,
                        resonance_hz: 2_800.0,
                        resonance_gain: 1.6,
                        decay_ms: 20.0,
                        seed: 0x5E6F_7081,
                    },
                ],
            ),
            // 退格：低频 thump + 一点闷噪声，像字车回弹。
            back: typewriter_wav(
                BACK_MS,
                BACK_PEAK,
                &[
                    Hit {
                        start_ms: 0,
                        milliseconds: BACK_MS,
                        gain: 1.0,
                        lowpass_hz: 1_200.0,
                        highpass_hz: 160.0,
                        resonance_hz: 190.0,
                        resonance_gain: 1.2,
                        decay_ms: 9.0,
                        seed: 0x6F70_8192,
                    },
                    Hit {
                        start_ms: 0,
                        milliseconds: 18,
                        gain: 0.6,
                        lowpass_hz: 3_000.0,
                        highpass_hz: 900.0,
                        resonance_hz: 1_200.0,
                        resonance_gain: 0.25,
                        decay_ms: 4.0,
                        seed: 0x7081_92A3,
                    },
                ],
            ),
        }
    }

    /// 沉闷的扳动开关：低频 thump 为主，噪声只留一点点“咔”的质感。
    fn switch() -> Self {
        Self {
            move_: typewriter_wav(
                38,
                0.21,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 38,
                    gain: 1.2,
                    lowpass_hz: 900.0,
                    highpass_hz: 90.0,
                    resonance_hz: 110.0,
                    resonance_gain: 1.4,
                    decay_ms: 12.0,
                    seed: 0x51A7_C4E1,
                }],
            ),
            focus: typewriter_wav(
                28,
                0.17,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 28,
                    gain: 0.9,
                    lowpass_hz: 1_200.0,
                    highpass_hz: 140.0,
                    resonance_hz: 170.0,
                    resonance_gain: 1.0,
                    decay_ms: 9.0,
                    seed: 0x62B8_D5F2,
                }],
            ),
            // 两段式扳动：先落锁，再回弹。
            confirm: typewriter_wav(
                120,
                0.22,
                &[
                    Hit {
                        start_ms: 0,
                        milliseconds: 55,
                        gain: 1.3,
                        lowpass_hz: 800.0,
                        highpass_hz: 80.0,
                        resonance_hz: 95.0,
                        resonance_gain: 1.5,
                        decay_ms: 15.0,
                        seed: 0x73C9_E603,
                    },
                    Hit {
                        start_ms: 48,
                        milliseconds: 72,
                        gain: 0.9,
                        lowpass_hz: 1_500.0,
                        highpass_hz: 180.0,
                        resonance_hz: 320.0,
                        resonance_gain: 0.7,
                        decay_ms: 10.0,
                        seed: 0x84DA_F714,
                    },
                ],
            ),
            back: typewriter_wav(
                48,
                0.18,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 48,
                    gain: 1.1,
                    lowpass_hz: 700.0,
                    highpass_hz: 70.0,
                    resonance_hz: 85.0,
                    resonance_gain: 1.3,
                    decay_ms: 16.0,
                    seed: 0x95EB_0825,
                }],
            ),
        }
    }

    /// 轻触：短、干、低音量，不抢音乐。
    fn soft() -> Self {
        Self {
            move_: typewriter_wav(
                22,
                0.16,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 22,
                    gain: 0.7,
                    lowpass_hz: 1_200.0,
                    highpass_hz: 220.0,
                    resonance_hz: 450.0,
                    resonance_gain: 0.35,
                    decay_ms: 5.0,
                    seed: 0xA6FC_1936,
                }],
            ),
            focus: typewriter_wav(
                16,
                0.17,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 16,
                    gain: 0.5,
                    lowpass_hz: 1_500.0,
                    highpass_hz: 280.0,
                    resonance_hz: 600.0,
                    resonance_gain: 0.25,
                    decay_ms: 3.5,
                    seed: 0xB70D_2A47,
                }],
            ),
            confirm: typewriter_wav(
                60,
                0.18,
                &[
                    Hit {
                        start_ms: 0,
                        milliseconds: 28,
                        gain: 0.8,
                        lowpass_hz: 1_100.0,
                        highpass_hz: 200.0,
                        resonance_hz: 380.0,
                        resonance_gain: 0.4,
                        decay_ms: 7.0,
                        seed: 0xC81E_3B58,
                    },
                    Hit {
                        start_ms: 24,
                        milliseconds: 36,
                        gain: 0.55,
                        lowpass_hz: 1_400.0,
                        highpass_hz: 260.0,
                        resonance_hz: 520.0,
                        resonance_gain: 0.3,
                        decay_ms: 5.0,
                        seed: 0xD92F_4C69,
                    },
                ],
            ),
            back: typewriter_wav(
                28,
                0.16,
                &[Hit {
                    start_ms: 0,
                    milliseconds: 28,
                    gain: 0.6,
                    lowpass_hz: 900.0,
                    highpass_hz: 160.0,
                    resonance_hz: 300.0,
                    resonance_gain: 0.35,
                    decay_ms: 6.0,
                    seed: 0xEA40_5D7A,
                }],
            ),
        }
    }
}

/// 一次机械敲击：噪声点击 + 一个共振峰，各自按指数衰减。
struct Hit {
    /// 在整段音效中的起始偏移（毫秒）。
    start_ms: u32,
    /// 持续长度（毫秒）；末尾必须已衰减到听不见。
    milliseconds: u32,
    /// 该次敲击的峰值权重。
    gain: f32,
    /// 噪声的低通截止频率：越低越闷。
    lowpass_hz: f32,
    /// 噪声的高通截止频率：越高越脆。
    highpass_hz: f32,
    /// 机械共振频率（低频 thump 或高频金属声）。
    resonance_hz: f32,
    /// 共振相对噪声的权重；0 表示纯噪声。
    resonance_gain: f32,
    /// 指数衰减时间常数（毫秒）。
    decay_ms: f32,
    /// 每次敲击独立的 LCG 种子，保证音色固定可复现。
    seed: u32,
}

/// 确定性 LCG：不用 rand，也不让每次启动的噪声音色漂移。
struct Lcg(u32);

impl Lcg {
    const fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// 归一化到 `-1..=1` 的白噪声。
    fn noise(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// 合成一整段打字机音效，并归一化到 `target_peak`（夹在 15%–25% 内）。
fn typewriter_wav(total_ms: u32, target_peak: f32, hits: &[Hit]) -> Vec<u8> {
    let frames = frames_for(total_ms);
    let mut mix = vec![0.0_f32; frames];
    for hit in hits {
        render_hit(&mut mix, hit);
    }
    normalise(&mut mix, target_peak);
    let samples: Vec<i16> = mix
        .iter()
        .map(|value| (value * f32::from(i16::MAX)).round() as i16)
        .collect();
    wav_bytes(&samples)
}

fn render_hit(mix: &mut [f32], hit: &Hit) {
    let start = frames_for(hit.start_ms);
    let end = start
        .saturating_add(frames_for(hit.milliseconds))
        .min(mix.len());
    if start >= end {
        return;
    }
    let sample_rate = SAMPLE_RATE as f32;
    let low_alpha = 1.0 - (-std::f32::consts::TAU * hit.lowpass_hz / sample_rate).exp();
    let high_rc = 1.0 / (std::f32::consts::TAU * hit.highpass_hz.max(1.0));
    let high_alpha = high_rc / (high_rc + 1.0 / sample_rate);
    let attack = attack_frames();
    let decay_seconds = (hit.decay_ms / 1000.0).max(0.001);
    let mut rng = Lcg::new(hit.seed);
    let mut low_state = 0.0_f32;
    let mut high_input = 0.0_f32;
    let mut high_state = 0.0_f32;
    for (offset, frame) in mix[start..end].iter_mut().enumerate() {
        let noise = rng.noise();
        low_state += low_alpha * (noise - low_state);
        high_state = high_alpha * (high_state + low_state - high_input);
        high_input = low_state;
        let attack_ramp = (offset.min(attack) as f32) / attack as f32;
        let decay = (-(offset as f32 / sample_rate) / decay_seconds).exp();
        let resonance =
            (std::f32::consts::TAU * hit.resonance_hz * offset as f32 / sample_rate).sin();
        let envelope = attack_ramp * decay * hit.gain;
        *frame += (high_state + resonance * hit.resonance_gain) * envelope;
    }
}

fn attack_frames() -> usize {
    (SAMPLE_RATE as f32 * ATTACK_SECONDS).round().max(1.0) as usize
}

fn normalise(mix: &mut [f32], target_peak: f32) {
    let peak = mix
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    if peak <= f32::EPSILON {
        return;
    }
    let target = target_peak.clamp(PEAK_LOW, PEAK_HIGH);
    let scale = target / peak;
    for sample in mix.iter_mut() {
        *sample *= scale;
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

    fn samples_of(bytes: &[u8]) -> Vec<i16> {
        bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect()
    }

    #[test]
    fn every_theme_and_cue_is_a_pcm_riff_wav_with_matching_data_size() {
        for theme in SoundTheme::ALL {
            for cue in [
                SoundCue::Move,
                SoundCue::Focus,
                SoundCue::Confirm,
                SoundCue::Back,
            ] {
                let bytes = wav_for(theme, cue);
                let label = format!("{theme:?}/{cue:?}");
                assert_eq!(&bytes[0..4], b"RIFF", "{label}");
                assert_eq!(&bytes[8..12], b"WAVE", "{label}");
                assert_eq!(&bytes[12..16], b"fmt ", "{label}");
                assert_eq!(u32_at(bytes, 16), 16, "{label} fmt size");
                assert_eq!(u16_at(bytes, 20), 1, "{label} PCM");
                assert_eq!(u16_at(bytes, 22), 1, "{label} mono");
                assert_eq!(u32_at(bytes, 24), SAMPLE_RATE, "{label} sample rate");
                assert_eq!(u32_at(bytes, 28), SAMPLE_RATE * 2, "{label} byte rate");
                assert_eq!(u16_at(bytes, 32), 2, "{label} block align");
                assert_eq!(u16_at(bytes, 34), 16, "{label} bits");
                assert_eq!(&bytes[36..40], b"data", "{label}");
                assert_eq!(
                    u32_at(bytes, 4) as usize + 8,
                    bytes.len(),
                    "{label} RIFF size"
                );
                let data_len = u32_at(bytes, 40) as usize;
                assert_eq!(data_len, bytes.len() - 44, "{label} data size");
                assert_eq!(data_len % 2, 0, "{label} 16-bit samples");
                let actual_ms = (data_len / 2) as f32 * 1000.0 / SAMPLE_RATE as f32;
                assert!((1.0..=200.0).contains(&actual_ms), "{label} {actual_ms} ms");
            }
        }
    }

    #[test]
    fn themes_round_trip_and_default_to_switch() {
        assert_eq!(SoundTheme::default(), SoundTheme::Switch);
        for theme in SoundTheme::ALL {
            assert_eq!(SoundTheme::from_id(theme.id()), Some(theme));
            assert!(!theme.label().is_empty());
        }
        assert_eq!(SoundTheme::from_id("missing"), None);
    }

    #[test]
    fn samples_are_audible_but_far_from_clipping() {
        for theme in SoundTheme::ALL {
            for cue in [
                SoundCue::Move,
                SoundCue::Focus,
                SoundCue::Confirm,
                SoundCue::Back,
            ] {
                let samples = samples_of(wav_for(theme, cue));
                let peak = samples
                    .iter()
                    .map(|sample| i32::from(*sample).abs())
                    .max()
                    .unwrap_or(0);
                assert!(peak > 0, "{theme:?}/{cue:?} must not be silent");
                assert_eq!(samples[0], 0, "{theme:?}/{cue:?} attack starts at zero");
                let ratio = peak as f32 / f32::from(i16::MAX);
                assert!(
                    (0.15..=0.25).contains(&ratio),
                    "{theme:?}/{cue:?} peak {peak} ({:.1}%) outside 15%–25%",
                    ratio * 100.0
                );
                assert!(peak < i32::from(i16::MAX), "{theme:?}/{cue:?} clipping");
            }
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
