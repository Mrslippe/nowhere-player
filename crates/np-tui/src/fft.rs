//! 2048 点 radix-2 FFT（手写，避免引入 rustfft）。

/// 视觉频谱的低切：去掉 50Hz 以下的隆隆声。
const MIN_HZ: f32 = 110.0;
/// 视觉频谱的高切：10kHz 以上对“音乐形状”贡献小。
const MAX_HZ: f32 = 10_000.0;
/// 本帧最强频段只到 0.85，留出顶部 headroom。
const HEADROOM: f32 = 0.85;
/// 相对最大峰值低于 4% 的频段视为噪声。
const NOISE_GATE: f32 = 0.04;

/// 返回 16 个对数频段的相对能量（0..=1）。
pub fn spectrum(samples: &[f32], sample_rate: u32, bands: usize) -> Vec<f32> {
    if samples.is_empty() || bands == 0 {
        return vec![0.0; bands];
    }
    // 一阶高通：让 30Hz 之类的低频真正衰减，而不是只从 FFT band 里跳过。
    // FFT 泄漏会把带外能量溅进 band，仅靠改变 band 起点不够。
    let sample_rate = sample_rate.max(1) as f32;
    let rc = 1.0 / (std::f32::consts::TAU * MIN_HZ);
    let alpha = rc / (rc + 1.0 / sample_rate);
    let mut previous_in = 0.0f32;
    let mut previous_out = 0.0f32;
    let mut values: Vec<(f32, f32)> = samples
        .iter()
        .take(2048)
        .enumerate()
        .map(|(index, &x)| {
            let filtered = alpha * (previous_out + x - previous_in);
            previous_in = x;
            previous_out = filtered;
            // Hann 窗：抑制 FFT 泄漏，代价只是每样本一次乘法。
            let window = 0.5
                - 0.5
                    * (2.0 * std::f32::consts::PI * index as f32
                        / (samples.len().clamp(2, 2048) as f32 - 1.0))
                        .cos();
            (filtered * window, 0.0)
        })
        .collect();
    values.resize(2048, (0.0, 0.0));
    fft(&mut values);
    let bins = values.len() / 2;
    let mut out = vec![0.0; bands];
    let max_hz = (sample_rate / 2.0).clamp(MIN_HZ * 2.0, MAX_HZ);
    for (band, slot) in out.iter_mut().enumerate() {
        let low = MIN_HZ * (max_hz / MIN_HZ).powf(band as f32 / bands as f32);
        let high = MIN_HZ * (max_hz / MIN_HZ).powf((band + 1) as f32 / bands as f32);
        let start = ((low / sample_rate) * 2048.0).floor() as usize;
        let end = (((high / sample_rate) * 2048.0).ceil() as usize)
            .max(start + 1)
            .min(bins);
        let count = end.saturating_sub(start.min(bins));
        if count == 0 {
            continue;
        }
        // 平均能量比单 bin 峰值稳定，避免一根低频 bin 决定整段高度。
        let mut sum = 0.0f32;
        for value in &values[start.min(bins)..end] {
            sum += (value.0 * value.0 + value.1 * value.1).sqrt();
        }
        *slot = sum / count as f32;
    }
    let max = out.iter().copied().fold(0.0f32, f32::max);
    if max < 1.0e-4 {
        return vec![0.0; bands];
    }
    for value in &mut out {
        let relative = (*value / max).clamp(0.0, 1.0);
        if relative < NOISE_GATE {
            *value = 0.0;
            continue;
        }
        let db = 20.0 * (relative + 1.0e-6).log10();
        *value = ((db + 60.0) / 60.0).clamp(0.0, 1.0) * HEADROOM;
    }
    out
}

fn fft(values: &mut [(f32, f32)]) {
    let n = values.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            values.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let (sin, cos) = angle.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for offset in 0..len / 2 {
                let (ur, ui) = values[start + offset];
                let (vr, vi) = values[start + offset + len / 2];
                let tr = vr * wr - vi * wi;
                let ti = vr * wi + vi * wr;
                values[start + offset] = (ur + tr, ui + ti);
                values[start + offset + len / 2] = (ur - tr, ui - ti);
                let next_wr = wr * cos - wi * sin;
                wi = wr * sin + wi * cos;
                wr = next_wr;
            }
        }
        len <<= 1;
    }
}

/// 把 0..=1 映射到八级字符。
#[allow(dead_code)]
pub fn level_char(value: f32) -> char {
    let index = (value.clamp(0.0, 1.0) * 7.0).round() as usize;
    crate::glyphs::BLOCK_LEVELS[index.min(7)]
        .chars()
        .next()
        .unwrap_or(' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_440_peaks_in_the_matching_band() {
        let sample_rate = 48_000;
        let samples: Vec<f32> = (0..2048)
            .map(|index| {
                (2.0 * std::f32::consts::PI * 440.0 * index as f32 / sample_rate as f32).sin()
            })
            .collect();
        let bands = spectrum(&samples, sample_rate, 16);
        let peak = bands
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert!(bands[peak] > 0.5, "{bands:?}");
        assert!(peak <= 10, "440Hz should be in a low band: {peak}");
    }

    #[test]
    fn low_cut_suppresses_thirty_hz_against_440_hz() {
        let sample_rate = 48_000;
        let samples: Vec<f32> = (0..2048)
            .map(|index| {
                let time = index as f32 / sample_rate as f32;
                let low = (2.0 * std::f32::consts::PI * 30.0 * time).sin();
                let mid = (2.0 * std::f32::consts::PI * 440.0 * time).sin();
                low + mid * 0.2
            })
            .collect();
        let bands = spectrum(&samples, sample_rate, 16);
        let peak = bands
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert!(peak > 3, "30Hz must not dominate after low cut: {bands:?}");
        assert!(
            bands[0] < bands[peak] * 0.5,
            "low band should be well below the 440Hz band: {bands:?}"
        );
    }

    #[test]
    fn level_char_covers_eight_steps() {
        assert_eq!(
            level_char(0.0),
            crate::glyphs::BLOCK_LEVELS[0].chars().next().unwrap()
        );
        assert_eq!(
            level_char(1.0),
            crate::glyphs::BLOCK_LEVELS[7].chars().next().unwrap()
        );
        assert_ne!(level_char(0.5), level_char(0.4));
    }
}
