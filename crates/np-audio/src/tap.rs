//! 音频样本只读 tap：单生产者（音频回调）写、多消费者读。
//!
//! 回调路径只做无锁原子写；不分配、不格式化。UI 用 `latest()` 取最近样本。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// 默认保留最近 8192 帧（L/R）。
pub const TAP_CAPACITY: usize = 8192;

struct TapShared {
    samples: Box<[AtomicU64]>,
    write: AtomicUsize,
    generation: AtomicU64,
    sample_rate: u32,
}

/// 可克隆的只读样本句柄。
#[derive(Clone)]
pub struct SampleTapHandle {
    shared: Arc<TapShared>,
}

/// [`SampleTapHandle`] 的生产端；由音频回调持有。
pub(crate) struct SampleTapWriter {
    shared: Arc<TapShared>,
}

/// 创建一个 tap，返回写端与读端。
pub(crate) fn create(sample_rate: u32) -> (SampleTapWriter, SampleTapHandle) {
    let shared = Arc::new(TapShared {
        samples: (0..TAP_CAPACITY * 2)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        write: AtomicUsize::new(0),
        generation: AtomicU64::new(0),
        sample_rate,
    });
    (
        SampleTapWriter {
            shared: Arc::clone(&shared),
        },
        SampleTapHandle { shared },
    )
}

impl SampleTapWriter {
    /// 音频回调内调用：保存 L/R；单声道两边写相同值。
    pub(crate) fn write_interleaved(&self, samples: &[f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }
        let frames = samples.len() / channels;
        let mut write = self.shared.write.load(Ordering::Relaxed);
        for frame in 0..frames {
            let start = frame * channels;
            let (left, right) = if channels == 1 {
                (samples[start], samples[start])
            } else {
                (samples[start], samples[start + 1])
            };
            let slot = write * 2;
            self.shared.samples[slot].store(u64::from(left.to_bits()), Ordering::Relaxed);
            self.shared.samples[slot + 1].store(u64::from(right.to_bits()), Ordering::Relaxed);
            write += 1;
            if write == TAP_CAPACITY {
                write = 0;
            }
        }
        self.shared.write.store(write, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }
}

impl SampleTapHandle {
    /// 当前样本率。
    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate
    }

    /// 写入代数；调用方可用它判断是否有新样本。
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }

    /// 读取最近 `out.len()` 个单声道样本（L/R 平均）；不足时用 0 填充。
    pub fn latest(&self, out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let write = self.shared.write.load(Ordering::Acquire);
        let count = out.len().min(TAP_CAPACITY);
        let start = (write + TAP_CAPACITY - count) % TAP_CAPACITY;
        for (index, slot) in out.iter_mut().enumerate() {
            if index < count {
                let frame = (start + index) % TAP_CAPACITY;
                let left = self.shared.samples[frame * 2].load(Ordering::Relaxed);
                let right = self.shared.samples[frame * 2 + 1].load(Ordering::Relaxed);
                let left = f32::from_bits(left as u32);
                let right = f32::from_bits(right as u32);
                *slot = (left + right) * 0.5;
            } else {
                *slot = 0.0;
            }
        }
    }

    /// 读取最近 L/R 样本，返回实际写入的帧数；不足部分两边用 0 填充。
    pub fn latest_stereo(&self, left: &mut [f32], right: &mut [f32]) -> usize {
        let count = left.len().min(right.len()).min(TAP_CAPACITY);
        if count == 0 {
            return 0;
        }
        let write = self.shared.write.load(Ordering::Acquire);
        let start = (write + TAP_CAPACITY - count) % TAP_CAPACITY;
        for index in 0..count {
            let frame = (start + index) % TAP_CAPACITY;
            let l = self.shared.samples[frame * 2].load(Ordering::Relaxed);
            let r = self.shared.samples[frame * 2 + 1].load(Ordering::Relaxed);
            left[index] = f32::from_bits(l as u32);
            right[index] = f32::from_bits(r as u32);
        }
        for slot in &mut left[count..] {
            *slot = 0.0;
        }
        for slot in &mut right[count..] {
            *slot = 0.0;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_latest_mono_samples() {
        let (writer, handle) = create(48_000);
        writer.write_interleaved(&[1.0, 1.0, 2.0, 2.0, 3.0, 3.0], 2);
        let mut out = [0.0; 2];
        handle.latest(&mut out);
        assert_eq!(out, [2.0, 3.0]);
        assert_eq!(handle.generation(), 1);
    }

    #[test]
    fn stereo_survives_left_and_right() {
        let (writer, handle) = create(48_000);
        writer.write_interleaved(&[0.25, -0.25, 0.5, -0.5], 2);
        let mut left = [0.0; 2];
        let mut right = [0.0; 2];
        assert_eq!(handle.latest_stereo(&mut left, &mut right), 2);
        assert_eq!(left, [0.25, 0.5]);
        assert_eq!(right, [-0.25, -0.5]);
        let mut mono = [0.0; 2];
        handle.latest(&mut mono);
        assert_eq!(mono, [0.0, 0.0]);
    }

    #[test]
    fn ring_overwrites_oldest_samples() {
        let (writer, handle) = create(48_000);
        let mut samples = Vec::with_capacity(TAP_CAPACITY + 2);
        for index in 0..(TAP_CAPACITY + 2) {
            samples.push(index as f32);
        }
        writer.write_interleaved(&samples, 1);
        let mut out = [0.0; 2];
        handle.latest(&mut out);
        assert_eq!(out, [TAP_CAPACITY as f32, (TAP_CAPACITY + 1) as f32]);
    }
}
