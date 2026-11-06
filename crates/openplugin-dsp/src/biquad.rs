//! Robert Bristow-Johnson (RBJ) audio EQ cookbook biquad filter.

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
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
    pub fn configure(&mut self, filter_type: FilterType, sample_rate: f32, freq: f32, q: f32, gain_db: f32) {
        let sr = sample_rate.max(1000.0);
        let f0 = freq.clamp(10.0, sr * 0.499);
        let q = q.max(0.01);
        let w0 = 2.0 * PI * f0 / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);
        let a = 10.0_f32.powf(gain_db / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match filter_type {
            FilterType::LowPass => {
                let b1 = 1.0 - cos_w0;
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
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Denormal prevention
        self.y1 = if y.abs() < 1e-25 { 0.0 } else { y };
        self.y1
    }
}
