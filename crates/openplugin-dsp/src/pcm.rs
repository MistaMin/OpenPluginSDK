//! PCM scale and linear interpolation.
//!
//! `dasp_sample` and `dasp_interpolate` (MIT OR Apache-2.0) convert full-scale
//! integers and blend two neighboring samples. Full scale is the half-open
//! range [-1, 1): the positive integer peak does not fit in the signed type.

const I16_SCALE: f32 = 32768.0;
const I32_SCALE: f32 = 2_147_483_648.0;

pub fn i16_to_f32(sample: i16) -> f32 {
    f32::from(sample) / I16_SCALE
}

pub fn f32_to_i16(sample: f32) -> i16 {
    let scaled = finite(sample) * I16_SCALE;
    scaled.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

pub fn i32_to_f32(sample: i32) -> f32 {
    sample as f32 / I32_SCALE
}

pub fn f32_to_i32(sample: f32) -> i32 {
    let scaled = finite(sample) * I32_SCALE;
    scaled.clamp(i32::MIN as f32, (i32::MAX as f32) - 1.0) as i32
}

/// `previous + (next - previous) * position`. `position` outside 0..1 is clamped.
pub fn lerp(previous: f32, next: f32, position: f32) -> f32 {
    let t = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let a = finite(previous);
    let b = finite(next);
    a + (b - a) * t
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_midpoint_round_trip() {
        assert_eq!(i16_to_f32(0), 0.0);
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), i16::MAX);
        assert!((lerp(0.0, 1.0, 0.25) - 0.25).abs() < 1e-6);
        assert!(i32_to_f32(0).abs() < 1e-8);
    }
}
