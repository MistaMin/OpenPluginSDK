//! Robert Bristow-Johnson (RBJ) audio EQ cookbook biquad filter.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
}

/// Coefficients and state are `f64`: in `f32` a 20 to 40 Hz shelf or
/// low-pass is off by up to 1 dB at 96 and 192 kHz.
#[derive(Debug, Clone, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    pub fn new() -> Self {
        Self {
            b0: 1.0,
            ..Default::default()
        }
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Computes coefficients based on RBJ formulas.
    pub fn configure(
        &mut self,
        filter_type: FilterType,
        sample_rate: f32,
        freq: f32,
        q: f32,
        gain_db: f32,
    ) {
        // A NaN or infinite parameter keeps the previous coefficients.
        if !(sample_rate.is_finite() && freq.is_finite() && q.is_finite() && gain_db.is_finite()) {
            return;
        }
        let sr = f64::from(sample_rate.max(1000.0));
        let f0 = f64::from(freq).clamp(10.0, sr * 0.499);
        let q = f64::from(q.max(0.01));
        let w0 = 2.0 * PI * f0 / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        // 1 - cos(w0) without the cancellation near w0 = 0.
        let one_minus_cos = 2.0 * (0.5 * w0).sin().powi(2);
        let alpha = sin_w0 / (2.0 * q);
        let a = 10.0_f64.powf(f64::from(gain_db) / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match filter_type {
            FilterType::LowPass => {
                let b1 = one_minus_cos;
                let b0 = b1 * 0.5;
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::HighPass => {
                let b0 = (1.0 + cos_w0) * 0.5;
                let b1 = -(1.0 + cos_w0);
                let b2 = b0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::BandPass => {
                let b0 = alpha;
                let b1 = 0.0;
                let b2 = -alpha;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::Notch => {
                let b0 = 1.0;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::Peaking => {
                let b0 = 1.0 + alpha * a;
                let b1 = -2.0 * cos_w0;
                let b2 = 1.0 - alpha * a;
                let a0 = 1.0 + alpha / a;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha / a;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::LowShelf => {
                let a_plus = a + 1.0;
                let a_minus = a - 1.0;
                let sqrt_a_2_alpha = 2.0 * a.sqrt() * alpha;
                let b0 = a * (a_plus - a_minus * cos_w0 + sqrt_a_2_alpha);
                let b1 = 2.0 * a * (a_minus - a_plus * cos_w0);
                let b2 = a * (a_plus - a_minus * cos_w0 - sqrt_a_2_alpha);
                let a0 = a_plus + a_minus * cos_w0 + sqrt_a_2_alpha;
                let a1 = -2.0 * (a_minus + a_plus * cos_w0);
                let a2 = a_plus + a_minus * cos_w0 - sqrt_a_2_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::HighShelf => {
                let a_plus = a + 1.0;
                let a_minus = a - 1.0;
                let sqrt_a_2_alpha = 2.0 * a.sqrt() * alpha;
                let b0 = a * (a_plus + a_minus * cos_w0 + sqrt_a_2_alpha);
                let b1 = -2.0 * a * (a_minus + a_plus * cos_w0);
                let b2 = a * (a_plus + a_minus * cos_w0 - sqrt_a_2_alpha);
                let a0 = a_plus - a_minus * cos_w0 + sqrt_a_2_alpha;
                let a1 = 2.0 * (a_minus - a_plus * cos_w0);
                let a2 = a_plus - a_minus * cos_w0 - sqrt_a_2_alpha;
                (b0, b1, b2, a0, a1, a2)
            }
        };

        let inv_a0 = 1.0 / a0;
        let c = [b0 * inv_a0, b1 * inv_a0, b2 * inv_a0, a1 * inv_a0, a2 * inv_a0];
        if !c.iter().all(|v| v.is_finite()) {
            return;
        }
        [self.b0, self.b1, self.b2, self.a1, self.a2] = c;
    }

    /// Filters one sample.
    ///
    /// A NaN or infinite input sample is treated as silence, so it never reaches the state.
    /// If the output still leaves the finite range, the state is cleared and 0.0 is returned,
    /// so one bad sample cannot silence the filter until the host resets it.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        if !(y as f32).is_finite() {
            self.reset();
            return 0.0;
        }
        // Denormal prevention: flush every state variable below 1e-30.
        let flush = |v: f64| if v.abs() < 1e-30 { 0.0 } else { v };
        self.x2 = self.x1;
        self.x1 = flush(x);
        self.y2 = self.y1;
        self.y1 = flush(y);
        self.y1 as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> Biquad {
        let mut b = Biquad::new();
        b.configure(FilterType::LowPass, 48000.0, 1000.0, 0.707, 0.0);
        b
    }

    #[test]
    fn non_finite_parameters_keep_previous_coefficients() {
        let good = configured();
        for (sr, f, q, g) in [
            (f32::NAN, 1000.0, 0.7, 0.0),
            (48000.0, f32::NAN, 0.7, 0.0),
            (48000.0, f32::INFINITY, 0.7, 0.0),
            (48000.0, 1000.0, f32::NEG_INFINITY, 0.0),
            (48000.0, 1000.0, 0.7, f32::NAN),
            (f32::INFINITY, 1000.0, 0.7, 6.0),
        ] {
            let mut b = good.clone();
            b.configure(FilterType::Peaking, sr, f, q, g);
            assert_eq!((b.b0, b.b1, b.b2, b.a1, b.a2), (good.b0, good.b1, good.b2, good.a1, good.a2));
        }
    }

    #[test]
    fn extreme_finite_gain_never_installs_non_finite_coefficients() {
        let mut b = configured();
        b.configure(FilterType::LowShelf, 48000.0, 100.0, 0.7, 1.0e30);
        for v in [b.b0, b.b1, b.b2, b.a1, b.a2] {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn tiny_state_is_flushed_for_x_and_y() {
        let mut b = configured();
        b.process_sample(1e-31);
        assert_eq!(b.x1, 0.0);
        assert_eq!(b.y1, 0.0);
        b.process_sample(0.0);
        assert_eq!((b.x1, b.x2, b.y1, b.y2), (0.0, 0.0, 0.0, 0.0));
    }

    /// Steady-state DC gain in dB, from the coefficients.
    fn dc_db(b: &Biquad) -> f64 {
        20.0 * ((b.b0 + b.b1 + b.b2) / (1.0 + b.a1 + b.a2)).abs().log10()
    }

    #[test]
    fn low_frequency_shelves_and_low_pass_hold_their_dc_gain_at_high_rates() {
        // Measured with f32 coefficients: +6.93, -11.01 and +1.18 dB.
        for (kind, rate, freq, gain, want) in [
            (FilterType::LowShelf, 192_000.0, 40.0, 6.0, 6.0),
            (FilterType::LowShelf, 48_000.0, 30.0, -12.0, -12.0),
            (FilterType::LowShelf, 192_000.0, 30.0, -12.0, -12.0),
            (FilterType::LowPass, 192_000.0, 20.0, 0.0, 0.0),
            (FilterType::LowPass, 96_000.0, 20.0, 0.0, 0.0),
            (FilterType::Peaking, 192_000.0, 30.0, 6.0, 0.0),
        ] {
            let mut b = Biquad::new();
            b.configure(kind, rate, freq, 0.707, gain);
            let got = dc_db(&b);
            assert!((got - want).abs() < 0.01, "{kind:?} {freq} Hz at {rate}: {got} dB");
            // And through the audio path: a long run of 1.0 settles there.
            let mut y = 0.0;
            for _ in 0..(rate as usize * 2) {
                y = b.process_sample(1.0);
            }
            let audio = 20.0 * f64::from(y).abs().log10();
            assert!((audio - want).abs() < 0.01, "{kind:?} {freq} Hz at {rate}: {audio} dB");
        }
    }

    #[test]
    fn decaying_tail_reaches_exact_zero() {
        let mut b = configured();
        b.process_sample(1.0);
        let mut last = 1.0;
        for _ in 0..200_000 {
            last = b.process_sample(0.0);
            if last == 0.0 {
                break;
            }
        }
        assert_eq!(last, 0.0);
    }
}
