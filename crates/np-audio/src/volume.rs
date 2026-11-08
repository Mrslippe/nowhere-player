//! Volume mapping.
//!
//! Volume is perceptual: the linear gain applied to samples is `v^3`
//! (开发方案 v0.1 §5.2). This lives in its own module because both the player
//! and future hardware-volume backends need the same curve.

/// Clamps a user-supplied volume into `0.0..=1.0`.
pub fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        return 0.0;
    }
    volume.clamp(0.0, 1.0)
}

/// Perceptual volume to linear gain: `gain = v^3`.
///
/// `1.0` maps to exactly `1.0`, which keeps the bit-perfect path honest: when
/// the plan disables software gain, samples are passed through untouched.
pub fn gain_for_volume(volume: f32) -> f32 {
    let v = clamp_volume(volume);
    v * v * v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_is_cubic() {
        assert_eq!(gain_for_volume(0.0), 0.0);
        assert!((gain_for_volume(0.5) - 0.125).abs() < f32::EPSILON);
        assert_eq!(gain_for_volume(1.0), 1.0);
    }

    #[test]
    fn gain_clamps_out_of_range_input() {
        assert_eq!(gain_for_volume(-1.0), 0.0);
        assert_eq!(gain_for_volume(2.0), 1.0);
        assert_eq!(clamp_volume(f32::NAN), 0.0);
    }
}
