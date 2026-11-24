//! Non-linear tape/tube saturation and harmonic drive curves.

/// The curve [`saturate_sample`] applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SaturationMode {
    /// `tanh`: smooth and symmetric (odd harmonics).
    Tape,
    /// `x / (1 + |x|)` for positive input and a softer negative side:
    /// asymmetric, so it adds even harmonics.
    Tube,
    /// Clamps to -1..=1.
    HardClip,
    /// The cubic `x - x^3 / 6.75` up to ±1.5, then ±1.
    SoftClip,
}

/// One sample through `mode` after a gain of `drive` (values below 1 count
/// as 1). The output stays within -1..=1 for finite input; a NaN input gives
/// NaN. Stateless, no allocation, any thread. Not band-limited: oversample
/// the call, or use [`crate::adaa`], to keep aliasing down.
///
/// # Example
///
/// ```
/// use openplugin_dsp::{saturate_sample, SaturationMode};
///
/// assert_eq!(saturate_sample(0.5, 4.0, SaturationMode::HardClip), 1.0);
/// let y = saturate_sample(0.5, 2.0, SaturationMode::Tape);
/// assert!((y - 1.0f32.tanh()).abs() < 1e-6);
/// ```
#[inline]
pub fn saturate_sample(x: f32, drive: f32, mode: SaturationMode) -> f32 {
    let driven = x * drive.max(1.0);
    match mode {
        SaturationMode::Tape => {
            // Smooth hyperbolic tangent with odd harmonic compression
            driven.tanh()
        }
        SaturationMode::Tube => {
            // Asymmetric soft saturation producing 2nd-order harmonics
            if driven >= 0.0 {
                driven / (1.0 + driven)
            } else {
                -((-driven) / (1.0 + (-driven * 0.6)))
            }
        }
        SaturationMode::SoftClip => {
            // Classic polynomial soft clipper
            if driven > 1.5 {
                1.0
            } else if driven < -1.5 {
                -1.0
            } else {
                driven - (driven * driven * driven) / 6.75
            }
        }
        SaturationMode::HardClip => driven.clamp(-1.0, 1.0),
    }
}
