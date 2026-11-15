//! White and pink noise.
//!
//! **White noise** is a 64 bit xorshift generator (Marsaglia, "Xorshift RNGs",
//! J. Stat. Software 8(14), 2003, with the `*` multiplier of Vigna, "An
//! experimental exploration of Marsaglia's xorshift generators, scrambled",
//! ACM TOMS 42(4), 2016) turned into a uniform value in `[-1, 1)`. A fixed seed
//! gives the same stream every time.
//!
//! **Pink noise** (power density proportional to `1/f`, -3 dB per octave) is
//! white noise through a pole/zero ladder. A filter whose poles and zeros
//! alternate on a logarithmic frequency axis, each zero half a spacing above
//! its pole, falls at -6 dB per octave between a pole and its zero and is flat
//! elsewhere, so on average it falls at -3 dB per octave in power. This is the
//! construction behind Paul Kellet's well known "pink-ish" filter (musicdsp.org
//! archive, "Filter to make pink noise from white", refined version), where the
//! poles and gains are fixed numbers for 44.1 kHz. Here they are computed from
//! the real sample rate by the bilinear transform with every pole and zero
//! pre-warped, so the slope holds at 48 kHz, 96 kHz and 384 kHz alike. The
//! stepped "sum of rows" alternative is the Voss-McCartney algorithm (Gardner,
//! "White and brown music, fractal curves and one-over-f fluctuations",
//! Scientific American 238(4), 1978); it is not used because its spectral
//! ripple is larger than this ladder's.
//!
//! The low corner is [`PINK_LOW_HZ`]: below it the spectrum levels off, so the
//! output has no unbounded low-frequency drift. The output power is normalised
//! to [`PINK_RMS`].
//!
//! Nothing here allocates. The input is bounded, so the state cannot reach
//! NaN, and a sustained input keeps it far from denormals.

/// Output RMS of [`PinkNoise`] (about -14 dBFS).
pub const PINK_RMS: f32 = 0.2;
/// Frequency below which the pink spectrum stops rising.
pub const PINK_LOW_HZ: f32 = 4.0;

const MAX_SECTIONS: usize = 40;
/// Pole spacing as a frequency ratio. Zeros sit `sqrt(RHO)` above their pole.
const RHO: f64 = 2.0;

/// Seeded uniform white noise in `[-1, 1)`.
#[derive(Clone, Copy, Debug)]
pub struct WhiteNoise {
    state: u64,
}

impl WhiteNoise {
    /// Any seed works; the state is never zero (xorshift needs that).
    pub const fn new(seed: u64) -> Self {
        let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
        Self {
            state: if mixed == 0 { 0x2545_F491_4F6C_DD1D } else { mixed },
        }
    }

    /// Next sample, uniform in `[-1, 1)`, mean 0, variance 1/3.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Top 24 bits -> [0, 2^24) -> [-1, 1).
        ((r >> 40) as f32) * (1.0 / 8_388_608.0) - 1.0
    }

    /// Fill a slice.
    pub fn fill(&mut self, out: &mut [f32]) {
        for v in out {
            *v = self.next();
        }
    }
}

impl Default for WhiteNoise {
    fn default() -> Self {
        Self::new(1)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Section {
    b0: f64,
    b1: f64,
    a1: f64,
    z: f64,
}

/// Pink noise: white noise through a sample-rate-aware pole/zero ladder.
#[derive(Clone, Copy, Debug)]
pub struct PinkNoise {
    white: WhiteNoise,
    sections: [Section; MAX_SECTIONS],
    count: usize,
    gain: f64,
    sample_rate: f32,
}

/// Analog angular frequency that the bilinear transform maps onto `f_hz`.
fn warp(f_hz: f64, sr: f64) -> f64 {
    2.0 * sr * (std::f64::consts::PI * f_hz / sr).tan()
}

impl PinkNoise {
    /// A generator for `sample_rate` (clamped to 1 kHz..2 MHz) and `seed`.
    pub fn new(sample_rate: f32, seed: u64) -> Self {
        let mut p = Self {
            white: WhiteNoise::new(seed),
            sections: [Section::default(); MAX_SECTIONS],
            count: 0,
            gain: 1.0,
            sample_rate: 48_000.0,
        };
        p.set_sample_rate(sample_rate);
        p
    }

    /// Redesign for a new rate. Keeps the filter state (a short transient only).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = if sample_rate.is_finite() {
            sample_rate.clamp(1_000.0, 2_000_000.0)
        } else {
            48_000.0
        };
        self.sample_rate = sr;
        let srd = f64::from(sr);
        let nyq = 0.5 * srd;
        let mut count = 0;
        let mut p_hz = f64::from(PINK_LOW_HZ);
        // Poles up to just under Nyquist; a zero above that is pulled to 0.98 of Nyquist.
        while p_hz < 0.98 * nyq && count < MAX_SECTIONS {
            let wp = warp(p_hz, srd);
            let cp = 2.0 * srd / wp;
            // Each pole-zero pair must drop the amplitude by exactly sqrt(RHO) (3.01 dB
            // in power for RHO = 2) per ratio RHO. The bilinear map stretches high
            // frequencies, so the zero is placed on the warped axis, not the digital one.
            let cz = 2.0 * srd / (wp * RHO.sqrt());
            // H(z) = ((1+cz) + (1-cz) z^-1) / ((1+cp) + (1-cp) z^-1), unity gain at DC.
            let a0 = 1.0 + cp;
            self.sections[count] = Section {
                b0: (1.0 + cz) / a0,
                b1: (1.0 - cz) / a0,
                a1: (1.0 - cp) / a0,
                z: self.sections[count].z,
            };
            count += 1;
            p_hz *= RHO;
        }
        self.count = count;
        // Normalise the white-noise power gain of the ladder to PINK_RMS.
        self.gain = 1.0;
        let power = self.power_gain();
        let target = f64::from(PINK_RMS) * f64::from(PINK_RMS) / (1.0 / 3.0);
        self.gain = (target / power.max(1e-30)).sqrt();
    }

    /// Mean of `|H|^2` over 0..Nyquist (power gain for white noise).
    fn power_gain(&self) -> f64 {
        const N: usize = 16_384;
        let mut acc = 0.0;
        for k in 0..N {
            let w = std::f64::consts::PI * (k as f64 + 0.5) / N as f64;
            acc += self.magnitude_sq(w);
        }
        acc / N as f64
    }

    fn magnitude_sq(&self, w: f64) -> f64 {
        let c1 = w.cos();
        let mut m = self.gain * self.gain;
        for s in &self.sections[..self.count] {
            // |b0 + b1 e^-jw|^2 / |1 + a1 e^-jw|^2
            let num = s.b0 * s.b0 + s.b1 * s.b1 + 2.0 * s.b0 * s.b1 * c1;
            let den = 1.0 + s.a1 * s.a1 + 2.0 * s.a1 * c1;
            m *= num / den;
        }
        m
    }

    /// Power response of the ladder in dB at `hz`, including the output
    /// normalisation. It falls 3 dB per octave from a few times
    /// [`PINK_LOW_HZ`] up to near Nyquist; used to verify the design.
    pub fn response_db(&self, hz: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * f64::from(hz) / f64::from(self.sample_rate);
        (10.0 * self.magnitude_sq(w).max(1e-300).log10()) as f32
    }

    /// Clear the filter state. The noise stream continues.
    pub fn reset(&mut self) {
        for s in &mut self.sections[..self.count] {
            s.z = 0.0;
        }
    }

    /// Next sample (RMS [`PINK_RMS`]).
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let mut x = f64::from(self.white.next());
        for s in &mut self.sections[..self.count] {
            // Transposed direct form II.
            let y = s.b0 * x + s.z;
            s.z = s.b1 * x - s.a1 * y;
            x = y;
        }
        (x * self.gain) as f32
    }

    /// Fill a slice.
    pub fn fill(&mut self, out: &mut [f32]) {
        for v in out {
            *v = self.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_is_uniform_zero_mean_and_repeatable() {
        let mut a = WhiteNoise::new(7);
        let mut b = WhiteNoise::new(7);
        let n = 400_000;
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        for _ in 0..n {
            let x = a.next();
            assert_eq!(x, b.next());
            assert!((-1.0..1.0).contains(&x));
            sum += f64::from(x);
            sq += f64::from(x) * f64::from(x);
        }
        assert!((sum / n as f64).abs() < 0.005);
        assert!((sq / n as f64 - 1.0 / 3.0).abs() < 0.005);
        assert_ne!(WhiteNoise::new(0).next(), 0.0);
    }

    #[test]
    fn pink_design_is_minus_three_db_per_octave() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0, 384_000.0] {
            let p = PinkNoise::new(sr, 1);
            let mut prev = p.response_db(40.0);
            let mut f = 80.0;
            while f < 0.3 * sr {
                let now = p.response_db(f);
                let slope = now - prev;
                assert!((slope + 3.0).abs() < 0.5, "sr {sr} at {f} Hz: {slope} dB/oct");
                prev = now;
                f *= 2.0;
            }
        }
    }

    #[test]
    fn pink_rms_and_finite() {
        let mut p = PinkNoise::new(48_000.0, 3);
        let n = 480_000;
        let mut sq = 0.0f64;
        for _ in 0..n {
            let x = p.next();
            assert!(x.is_finite());
            sq += f64::from(x) * f64::from(x);
        }
        let rms = (sq / n as f64).sqrt();
        assert!((rms - f64::from(PINK_RMS)).abs() < 0.03, "{rms}");
    }
}
