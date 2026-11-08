//! Lock-free single-producer / single-consumer ring buffer.
//!
//! This is the only hand-off between the decode thread (producer) and the audio
//! callback (consumer). Neither side ever blocks on a lock: the producer drops
//! samples when the buffer is full and the consumer gets silence when it is
//! empty, which is exactly the behaviour the playback pipeline wants.

use ringbuf::{
    HeapCons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

/// Creates a ring buffer holding `capacity_samples` interleaved samples.
///
/// Capacity is expressed in samples, not frames: for 300 ms of stereo at
/// 48 kHz that is `0.3 * 48_000 * 2 = 28_800` samples.
pub fn spsc(capacity_samples: usize) -> (RingProducer, RingConsumer) {
    let rb = HeapRb::<f32>::new(capacity_samples.max(1));
    let (producer, consumer) = rb.split();
    (
        RingProducer { inner: producer },
        RingConsumer { inner: consumer },
    )
}

/// Producer half of the ring buffer. Lives on the decode thread.
pub struct RingProducer {
    inner: HeapProd<f32>,
}

impl RingProducer {
    /// Pushes as many samples as fit and returns how many were written.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.inner.push_slice(samples)
    }

    /// Number of samples that can currently be pushed without dropping data.
    pub fn vacant(&self) -> usize {
        self.inner.vacant_len()
    }

    /// Number of samples currently buffered.
    pub fn occupied(&self) -> usize {
        self.inner.occupied_len()
    }
}

/// Consumer half of the ring buffer. Lives on the audio thread.
pub struct RingConsumer {
    inner: HeapCons<f32>,
}

impl RingConsumer {
    /// Pops as many samples as available and returns how many were read.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        self.inner.pop_slice(out)
    }

    /// Discards every buffered sample.
    ///
    /// Constant time; called from the audio callback when a seek invalidates
    /// the samples already queued.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Number of samples currently buffered.
    pub fn occupied(&self) -> usize {
        self.inner.occupied_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_wraps_around() {
        let (mut producer, mut consumer) = spsc(8);
        assert_eq!(producer.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 6);

        let mut out = [0.0f32; 4];
        assert_eq!(consumer.pop(&mut out), 4);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);

        // Push past the original fill point so the indices wrap.
        assert_eq!(producer.push(&[7.0, 8.0, 9.0, 10.0]), 4);
        let mut out = [0.0f32; 8];
        assert_eq!(consumer.pop(&mut out), 6);
        assert_eq!(out[..6], [5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
    }

    #[test]
    fn full_buffer_reports_partial_push() {
        let (mut producer, mut consumer) = spsc(3);
        assert_eq!(producer.push(&[1.0, 2.0]), 2);
        assert_eq!(producer.push(&[3.0, 4.0, 5.0]), 1);
        assert_eq!(producer.vacant(), 0);
        assert_eq!(producer.occupied(), 3);

        let mut out = [0.0f32; 2];
        assert_eq!(consumer.pop(&mut out), 2);
        assert_eq!(out, [1.0, 2.0]);
    }

    #[test]
    fn empty_buffer_returns_zero_and_clear_discards() {
        let (mut producer, mut consumer) = spsc(4);
        let mut out = [0.0f32; 4];
        assert_eq!(consumer.pop(&mut out), 0);

        assert_eq!(producer.push(&[1.0, 2.0, 3.0]), 3);
        consumer.clear();
        assert_eq!(consumer.occupied(), 0);
        assert_eq!(consumer.pop(&mut out), 0);
    }
}
