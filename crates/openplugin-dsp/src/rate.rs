//! Host rate and oversampling factor check.
//!
//! The rate planner refuses a zero host rate, a factor other than 1, 2, or 4,
//! and a product that overflows. This is only the rate arithmetic a plugin
//! needs before `prepare`.

/// The rate a DSP block computes its coefficients for: a finite `sample_rate`
/// clamped to the SDK's range ([`openplugin_core::MIN_SAMPLE_RATE`] 1 kHz to
/// [`openplugin_core::MAX_SAMPLE_RATE`] 768 kHz), and 48 kHz for NaN or
/// infinity. Every block in this crate that takes a rate goes through this, so
/// a rate the format bridges accept is the rate the DSP uses.
pub fn effective_rate(sample_rate: f32) -> f32 {
    use openplugin_core::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
    if sample_rate.is_finite() {
        sample_rate.clamp(MIN_SAMPLE_RATE, MAX_SAMPLE_RATE)
    } else {
        48_000.0
    }
}

/// A checked host rate and oversampling factor, from [`plan_rate`].
///
/// # Example
///
/// ```
/// use openplugin_dsp::{plan_rate, RateError};
///
/// let plan = plan_rate(48_000, 4).unwrap();
/// assert_eq!(plan.internal_hz, 192_000);
/// assert_eq!(plan_rate(48_000, 3), Err(RateError::UnsupportedFactor));
/// assert_eq!(plan_rate(0, 2), Err(RateError::InvalidRate));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatePlan {
    /// The host's sample rate, Hz.
    pub host_hz: u32,
    /// The oversampling factor: 1, 2 or 4.
    pub factor: u32,
    /// `host_hz * factor`, the rate the oversampled DSP runs at, Hz.
    pub internal_hz: u32,
}

/// Why [`plan_rate`] refused a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RateError {
    /// The host rate is zero.
    InvalidRate,
    /// The factor is not 1, 2 or 4.
    UnsupportedFactor,
    /// `host_hz * factor` does not fit in a `u32`.
    Overflow,
}

/// The common studio rates, Hz: 44.1, 48, 88.2, 96, 176.4 and 192 kHz.
pub const STUDIO_RATES_HZ: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Whether `host_hz` is one of [`STUDIO_RATES_HZ`]. Other rates still work;
/// this is for a warning in a user interface.
pub fn is_studio_rate(host_hz: u32) -> bool {
    STUDIO_RATES_HZ.contains(&host_hz)
}

/// Checks a host rate and an oversampling factor and returns the internal
/// rate. Fails with [`RateError`] for a zero rate, a factor other than 1, 2 or
/// 4, or an overflow. Pure arithmetic: no allocation, any thread.
pub fn plan_rate(host_hz: u32, factor: u32) -> Result<RatePlan, RateError> {
    if host_hz == 0 {
        return Err(RateError::InvalidRate);
    }
    if ![1, 2, 4].contains(&factor) {
        return Err(RateError::UnsupportedFactor);
    }
    let internal_hz = host_hz.checked_mul(factor).ok_or(RateError::Overflow)?;
    Ok(RatePlan {
        host_hz,
        factor,
        internal_hz,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_rates_and_rejections() {
        let plan = plan_rate(48_000, 4).unwrap();
        assert_eq!(plan.internal_hz, 192_000);
        assert!(is_studio_rate(44_100));
        assert!(!is_studio_rate(22_050));
        assert_eq!(plan_rate(0, 2), Err(RateError::InvalidRate));
        assert_eq!(plan_rate(48_000, 3), Err(RateError::UnsupportedFactor));
        assert_eq!(plan_rate(u32::MAX, 4), Err(RateError::Overflow));
    }
}
