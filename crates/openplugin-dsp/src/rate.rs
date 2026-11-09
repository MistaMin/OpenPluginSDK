//! Host rate and oversampling factor check.
//!
//! Water Channel's rate planner refuses a zero host rate, a factor other than
//! 1, 2, or 4, and a product that overflows. The model-file inventory stays in
//! the plugin. This is only the rate arithmetic a plugin needs before `prepare`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatePlan {
    pub host_hz: u32,
    pub factor: u32,
    pub internal_hz: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateError {
    InvalidRate,
    UnsupportedFactor,
    Overflow,
}

pub const STUDIO_RATES_HZ: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

pub fn is_studio_rate(host_hz: u32) -> bool {
    STUDIO_RATES_HZ.contains(&host_hz)
}

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
