//! 2048 点 radix-2 FFT（手写，避免引入 rustfft）。

/// 返回 16 个对数频段的相对能量（0..=1）。
pub fn spectrum(samples: &[f32], sample_rate: u32, bands: usize) -> Vec<f32> {
    if samples.is_empty() || bands == 0 {
        return vec![0.0; bands];
    }
    let mut values: Vec<(f32, f32)> = samples
        .iter()
        .take(2048)
        .enumerate()
        .map(|(index, &x)| {
            // Hann 窗：抑制 FFT 泄漏，代价只是每样本一次乘法。
            let window = 0.5
                - 0.5
                    * (2.0 * std::f32::consts::PI * index as f32
                        / (samples.len().clamp(2, 2048) as f32 - 1.0))
                        .cos();
            (x * window, 0.0)
        })
        .collect();
    values.resize(2048, (0.0, 0.0));
    fft(&mut values);
    let bins = values.len() / 2;
    let mut out = vec![0.0; bands];
    let min_hz = 20.0f32;
    let max_hz = (sample_rate as f32 / 2.0).max(min_hz * 2.0);
    for (band, slot) in out.iter_mut().enumerate() {
        let low = min_hz * (max_hz / min_hz).powf(band as f32 / bands as f32);
        let high = min_hz * (max_hz / min_hz).powf((band + 1) as f32 / bands as f32);
        let start = ((low / sample_rate as f32) * 2048.0).floor() as usize;
        let end = (((high / sample_rate as f32) * 2048.0).ceil() as usize)
            .max(start + 1)
            .min(bins);
        let mut peak: f32 = 0.0;
        for value in &values[start.min(bins)..end] {
            let magnitude = (value.0 * value.0 + value.1 * value.1).sqrt();
            peak = peak.max(magnitude);
        }
        // 对数幅度：-60dB..0dB 映射到 0..1，避免线性谱全挤在左侧。
        let db = 20.0 * (peak + 1.0e-6).log10();
        *slot = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
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
