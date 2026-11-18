//! WSOLA 追帧：保持音高的 1.00–1.10 倍速时间缩放（T-0093）。
//!
//! 分析窗 20ms、合成 hop 10ms；每帧在新的名义位置 ±5ms 内做归一化互相关，
//! 让相邻帧的波形相位对齐，再做 Hann 窗重叠相加。窗口权重单独累加并在输出时
//! 归一化，因此合成 hop 不恰好是半个窗时也不会起伏或削波。
//!
//! 直播只往「变快」方向用（1.00–1.10），不降速。1.00x 时输出与输入逐样本
//! 一致，只多约一个分析窗的延迟；因此播放链路可以常开，不必在 1.00x 切换。

use std::collections::VecDeque;

/// 分析窗长度（毫秒）。
pub const WINDOW_MS: u32 = 20;
/// 合成 hop 长度（毫秒）。
pub const HOP_MS: u32 = 10;
/// 相位搜索半径（毫秒）。
pub const TOLERANCE_MS: u32 = 5;
/// 最小速度（不降速）。
pub const MIN_SPEED: f32 = 1.0;
/// 最大速度。
pub const MAX_SPEED: f32 = 1.10;

/// 一路 PCM 的流式 WSOLA 变速器。
///
/// 输入 / 输出都是交错 f32；调用方按任意块大小 `push`，再用 [`take`] 取走
/// 已经合成好的样本。内部保留一个分析窗 + 搜索半径的输入，延迟约 30ms。
///
/// [`take`]: WsolaTempo::take
#[derive(Debug)]
pub struct WsolaTempo {
    sample_rate: u32,
    channels: usize,
    window: usize,
    hop: usize,
    tolerance: usize,
    speed: f32,
    /// 下一帧的名义分析位置（绝对帧号）。
    analysis: u64,
    /// 下一帧的合成位置（绝对帧号）。
    synthesis: u64,
    /// 上一帧实际选中的分析位置；用于计算相位模板。
    last_analysis: Option<u64>,
    /// 尚未消费的输入（交错）。
    input: Vec<f32>,
    /// `input[0]` 对应的绝对帧号。
    input_base: u64,
    /// 已合成、等待调用方取走的输出。
    ready: VecDeque<f32>,
    /// 正在重叠相加的样本（交错），起点是 `acc_base`。
    acc: Vec<f32>,
    /// `acc` 每个输出帧的窗口权重和。
    weight: Vec<f32>,
    acc_base: u64,
    started: bool,
    finished: bool,
}

impl WsolaTempo {
    /// 按秒采样率和通道数创建；速度先为 1.00。
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let sample_rate = sample_rate.max(1);
        let channels = usize::from(channels.max(1));
        let window = ms_to_frames(sample_rate, WINDOW_MS).max(2);
        let hop = ms_to_frames(sample_rate, HOP_MS).clamp(1, window - 1);
        let tolerance = ms_to_frames(sample_rate, TOLERANCE_MS);
        Self {
            sample_rate,
            channels,
            window,
            hop,
            tolerance,
            speed: MIN_SPEED,
            analysis: 0,
            synthesis: 0,
            last_analysis: None,
            input: Vec::new(),
            input_base: 0,
            ready: VecDeque::new(),
            acc: Vec::new(),
            weight: Vec::new(),
            acc_base: 0,
            started: false,
            finished: false,
        }
    }

    /// 采样率。
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 通道数。
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// 当前速度；构造后与 [`set_speed`](Self::set_speed) 一致。
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// 设置速度，夹到 [`MIN_SPEED`]–[`MAX_SPEED`]；非有限值回落到 1.00。
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = if speed.is_finite() {
            speed.clamp(MIN_SPEED, MAX_SPEED)
        } else {
            MIN_SPEED
        };
    }

    /// 当前已合成、可直接取走的样本数（含所有通道）。
    pub fn pending(&self) -> usize {
        self.ready.len()
    }

    /// 清空全部状态，回到 1.00x 的起点；已合成但未取走的输出也会丢弃。
    pub fn reset(&mut self) {
        self.ready.clear();
        self.input.clear();
        self.acc.clear();
        self.weight.clear();
        self.analysis = 0;
        self.synthesis = 0;
        self.input_base = 0;
        self.acc_base = 0;
        self.last_analysis = None;
        self.started = false;
        self.finished = false;
        self.speed = MIN_SPEED;
    }

    /// 追加交错 PCM；长度必须是通道数的整数倍。
    pub fn push(&mut self, input: &[f32]) {
        if self.finished || input.is_empty() {
            return;
        }
        debug_assert_eq!(input.len() % self.channels, 0, "interleaved block");
        self.input.extend_from_slice(input);
        self.generate();
    }

    /// 把已合成的样本写进 `out`，返回写入的**样本数**（含所有通道）。
    pub fn take(&mut self, out: &mut [f32]) -> usize {
        let count = out.len().min(self.ready.len());
        for (slot, sample) in out.iter_mut().zip(self.ready.drain(..count)) {
            *slot = sample;
        }
        count
    }

    /// 流结束：把剩余重叠区归一化后交出。之后 [`push`](Self::push) 被忽略。
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.generate();
        self.flush_ready(self.weight_len());
        self.finished = true;
    }

    /// 当前绝对输入帧号（下一个待分析位置之前已消费的部分）。
    pub fn consumed_input_frames(&self) -> u64 {
        self.analysis
    }

    fn speed_is_one(&self) -> bool {
        (self.speed - MIN_SPEED).abs() < f32::EPSILON
    }

    /// 合成 hop：分析 hop 固定 10ms，输出侧按速度缩短。
    fn synthesis_hop(&self) -> usize {
        let hop = self.hop as f64 / f64::from(self.speed);
        (hop.round() as usize).max(1)
    }

    fn input_frames(&self) -> usize {
        self.input.len() / self.channels
    }

    fn input_end(&self) -> u64 {
        self.input_base + self.input_frames() as u64
    }

    fn weight_len(&self) -> usize {
        self.weight.len()
    }

    fn generate(&mut self) {
        loop {
            // 只要够一个分析窗就合成一帧；相位搜索自己会按可用输入缩短。
            // 这样流结束时不会把最后 20ms 永远留在输入缓冲里。
            let need = self.analysis + self.window as u64;
            if self.input_end() < need {
                break;
            }
            self.emit_frame();
        }
    }

    fn emit_frame(&mut self) {
        let nominal = self.analysis;
        let offset = if self.started && !self.speed_is_one() {
            self.best_offset(nominal)
        } else {
            0
        };
        let start = (nominal as i64 + offset) as u64;
        let synthesis_offset = (self.synthesis - self.acc_base) as usize;
        self.ensure_acc(synthesis_offset + self.window);

        for n in 0..self.window {
            let weight = window_weight(n, self.window);
            self.weight[synthesis_offset + n] += weight;
            let src = (start - self.input_base) as usize + n;
            let dst = (synthesis_offset + n) * self.channels;
            for channel in 0..self.channels {
                self.acc[dst + channel] += weight * self.input[src * self.channels + channel];
            }
        }

        self.last_analysis = Some(start);
        self.analysis += self.hop as u64;
        self.synthesis += self.synthesis_hop() as u64;
        self.flush_ready(self.synthesis.saturating_sub(self.acc_base) as usize);
        self.trim_input();
        self.started = true;
    }

    /// 在 `nominal ± tolerance` 里挑与上一帧播放内容最相似的位置（0 优先）。
    fn best_offset(&self, nominal: u64) -> i64 {
        let Some(last) = self.last_analysis else {
            return 0;
        };
        let template_start = last + self.synthesis_hop() as u64;
        let available = self.input_end();
        let length = (self.window - self.hop)
            .min(self.hop)
            .min(available.saturating_sub(template_start) as usize);
        if length == 0 {
            return 0;
        }
        if template_start < self.input_base {
            return 0;
        }

        let mut best_score = f64::NEG_INFINITY;
        let mut best_delta = 0i64;
        let tolerance = self.tolerance as i64;
        // 先试 0，再向两侧展开：同分时保持 0，1.00x 才能逐样本还原。
        for step in 0..=tolerance {
            let candidates: [i64; 2] = if step == 0 { [0, 0] } else { [-step, step] };
            for delta in candidates {
                let start = nominal as i64 + delta;
                if start < self.input_base as i64 {
                    continue;
                }
                let start = start as u64;
                if start + self.window as u64 > available {
                    continue;
                }
                let score = self.correlation(template_start, start, length);
                if score > best_score {
                    best_score = score;
                    best_delta = delta;
                }
            }
        }
        best_delta
    }

    /// 两个等长片段（取第 0 通道）的归一化互相关。
    fn correlation(&self, left: u64, right: u64, length: usize) -> f64 {
        let left = (left - self.input_base) as usize;
        let right = (right - self.input_base) as usize;
        let mut num = 0.0f64;
        let mut energy_left = 0.0f64;
        let mut energy_right = 0.0f64;
        for n in 0..length {
            let a = f64::from(self.input[(left + n) * self.channels]);
            let b = f64::from(self.input[(right + n) * self.channels]);
            num += a * b;
            energy_left += a * a;
            energy_right += b * b;
        }
        let denominator = (energy_left * energy_right).sqrt();
        if denominator <= f64::EPSILON {
            0.0
        } else {
            num / denominator
        }
    }

    fn ensure_acc(&mut self, frames: usize) {
        if self.weight.len() < frames {
            self.weight.resize(frames, 0.0);
            self.acc.resize(frames * self.channels, 0.0);
        }
    }

    /// 把 `count` 个输出帧归一化后交出去，并丢掉对应累加区。
    fn flush_ready(&mut self, count: usize) {
        let frames = count.min(self.weight.len());
        if frames == 0 {
            return;
        }
        let channels = self.channels;
        for n in 0..frames {
            let weight = self.weight[n];
            let base = n * channels;
            if weight > 0.0 {
                for channel in 0..channels {
                    self.ready.push_back(self.acc[base + channel] / weight);
                }
            } else {
                for _ in 0..channels {
                    self.ready.push_back(0.0);
                }
            }
        }
        self.acc.drain(0..frames * channels);
        self.weight.drain(0..frames);
        self.acc_base += frames as u64;
    }

    /// 丢掉不再需要的前缀输入；保留 `analysis - tolerance` 之后的部分。
    fn trim_input(&mut self) {
        let keep_from = self.analysis.saturating_sub(self.tolerance as u64);
        if keep_from <= self.input_base {
            return;
        }
        let drop = (keep_from - self.input_base) as usize;
        let drop = drop.min(self.input_frames());
        if drop == 0 {
            return;
        }
        self.input.drain(0..drop * self.channels);
        self.input_base += drop as u64;
    }
}

fn ms_to_frames(sample_rate: u32, ms: u32) -> usize {
    ((u64::from(sample_rate) * u64::from(ms)) / 1_000) as usize
}

/// 半个样本偏移的 Hann 窗：端点也不为 0，权重归一化时不会放大噪声。
fn window_weight(index: usize, window: usize) -> f32 {
    let phase = 2.0 * std::f64::consts::PI * (index as f64 + 0.5) / window as f64;
    (0.5 - 0.5 * phase.cos()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const TONE_HZ: f64 = 1_000.0;

    fn sine(frames: usize, channels: usize, amplitude: f32) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames * channels);
        for n in 0..frames {
            let value = amplitude
                * (2.0 * std::f64::consts::PI * TONE_HZ * n as f64 / f64::from(RATE)).sin() as f32;
            for _ in 0..channels {
                samples.push(value);
            }
        }
        samples
    }

    fn stretch(input: &[f32], channels: u16, speed: f32, block: usize) -> Vec<f32> {
        let mut tempo = WsolaTempo::new(RATE, channels);
        tempo.set_speed(speed);
        let mut output = Vec::new();
        let mut scratch = vec![0.0f32; 4096];
        for chunk in input.chunks(block) {
            tempo.push(chunk);
            loop {
                let written = tempo.take(&mut scratch);
                if written == 0 {
                    break;
                }
                output.extend_from_slice(&scratch[..written]);
            }
        }
        tempo.finish();
        loop {
            let written = tempo.take(&mut scratch);
            if written == 0 {
                break;
            }
            output.extend_from_slice(&scratch[..written]);
        }
        output
    }

    /// 过零率估算基频（纯正弦足够）。
    fn estimated_frequency(samples: &[f32], channels: usize) -> f64 {
        let frames = samples.len() / channels;
        let skip = RATE as usize / 20; // 跳过首 50ms，避开启动窗
        let end = frames.saturating_sub(RATE as usize / 20);
        if end <= skip {
            return 0.0;
        }
        let mut crossings = 0usize;
        let mut previous = samples[skip * channels];
        for frame in (skip + 1)..end {
            let current = samples[frame * channels];
            if (previous <= 0.0 && current > 0.0) || (previous >= 0.0 && current < 0.0) {
                crossings += 1;
            }
            previous = current;
        }
        let seconds = (end - skip) as f64 / f64::from(RATE);
        crossings as f64 / (2.0 * seconds)
    }

    #[test]
    fn speed_is_clamped_to_the_live_range() {
        let mut tempo = WsolaTempo::new(RATE, 2);
        assert_eq!(tempo.speed(), 1.0);
        tempo.set_speed(1.5);
        assert_eq!(tempo.speed(), MAX_SPEED);
        tempo.set_speed(0.5);
        assert_eq!(tempo.speed(), MIN_SPEED);
        tempo.set_speed(f32::NAN);
        assert_eq!(tempo.speed(), MIN_SPEED);
    }

    #[test]
    fn tempo_105_shrinks_duration_and_keeps_1khz_pitch() {
        let input = sine(RATE as usize, 1, 0.5);
        let output = stretch(&input, 1, 1.05, 1024);
        let ratio = output.len() as f64 / input.len() as f64;
        assert!(
            (0.93..=0.97).contains(&ratio),
            "1.05x should keep about 95% of the frames, got {ratio}"
        );
        let frequency = estimated_frequency(&output, 1);
        assert!(
            (frequency - TONE_HZ).abs() < 5.0,
            "pitch should stay near 1kHz, got {frequency:.1}Hz"
        );
        let peak = output
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak <= 0.51, "overlap-add must not clip, peak {peak}");
    }

    #[test]
    fn tempo_110_keeps_pitch_without_clipping() {
        let input = sine(RATE as usize / 2, 2, 0.6);
        let output = stretch(&input, 2, 1.10, 1024);
        let ratio = output.len() as f64 / input.len() as f64;
        assert!(
            (0.88..=0.93).contains(&ratio),
            "1.10x should keep about 91% of the frames, got {ratio}"
        );
        let frequency = estimated_frequency(&output, 2);
        assert!(
            (frequency - TONE_HZ).abs() < 5.0,
            "pitch should stay near 1kHz, got {frequency:.1}Hz"
        );
        let peak = output
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak <= 0.61, "overlap-add must not clip, peak {peak}");
    }

    #[test]
    fn speed_one_is_a_sample_exact_passthrough() {
        let input = sine(RATE as usize / 4, 2, 0.5);
        let output = stretch(&input, 2, 1.0, 1024);
        let frames = output.len() / 2;
        assert!(
            frames + 2 * RATE as usize / 100 >= input.len() / 2,
            "1.00x must not drop audible frames"
        );
        // 去掉分析窗延迟后逐样本比较（输出前 25ms 是启动窗）。
        let skip = (WINDOW_MS + TOLERANCE_MS) as usize * RATE as usize / 1_000;
        let compare = frames.min(input.len() / 2) - skip - 10;
        let mut max_error = 0.0f32;
        for frame in 0..compare {
            let expected = input[(skip + frame) * 2];
            let actual = output[(skip + frame) * 2];
            max_error = max_error.max((expected - actual).abs());
        }
        assert!(
            max_error < 1e-3,
            "1.00x should reconstruct the tone, max error {max_error}"
        );
    }

    #[test]
    fn reset_returns_to_a_clean_one_x_state() {
        let mut tempo = WsolaTempo::new(RATE, 1);
        tempo.push(&sine(4_800, 1, 0.5));
        assert!(tempo.pending() > 0);
        tempo.set_speed(1.1);
        tempo.reset();
        assert_eq!(tempo.speed(), 1.0);
        assert_eq!(tempo.pending(), 0);
        assert_eq!(tempo.consumed_input_frames(), 0);
    }

    #[test]
    fn empty_push_and_take_are_noops() {
        let mut tempo = WsolaTempo::new(RATE, 2);
        tempo.push(&[]);
        assert_eq!(tempo.pending(), 0);
        let mut out = [0.0f32; 8];
        assert_eq!(tempo.take(&mut out), 0);
    }
}
