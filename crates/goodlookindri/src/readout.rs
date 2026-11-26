//! Sample-timed display aggregation, inspired by DualSone's readable loudness/crest displays.
//! Supply already measured LUFS and crest, not raw audio. This is not a loudness detector.
//! Single-thread owned; publish the returned values to the GUI using host-owned atomics.
const BINS: usize = 40;
#[derive(Clone, Copy)]
struct Bin {
    energy: f64,
    heard: u64,
    crest: f32,
    samples: u64,
}
impl Default for Bin {
    fn default() -> Self {
        Self {
            energy: 0.0,
            heard: 0,
            crest: -100.0,
            samples: 0,
        }
    }
}
pub struct ReadoutWindow {
    bins: [Bin; BINS],
    current: usize,
    bin_samples: u64,
}
impl Default for ReadoutWindow {
    fn default() -> Self {
        Self::new(48000.0)
    }
}
impl ReadoutWindow {
    /// Forty rounded 50 ms bins: retained history is 1.95–2 seconds at typical rates.
    /// Invalid rates fall back to 48 kHz. No allocations in construction or update.
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48000.0
        };
        Self {
            bins: [Bin::default(); BINS],
            current: 0,
            bin_samples: (rate.clamp(1.0, 768000.0) * 0.05).round().max(1.0) as u64,
        }
    }
    pub fn reset(&mut self) {
        self.bins.fill(Bin::default());
        self.current = 0;
    }
    pub fn add(&mut self, lufs: f32, crest_db: f32, mut samples: u64) {
        // Invalid and implausibly large measurements cannot poison the history.
        let energy = if lufs.is_finite() && lufs > -70.0 && lufs <= 100.0 {
            Some(10.0_f64.powf(lufs as f64 * 0.1))
        } else {
            None
        };
        while samples > 0 {
            let b = &mut self.bins[self.current];
            let take = samples.min(self.bin_samples - b.samples);
            if let Some(e) = energy {
                b.energy += e * take as f64;
                b.heard += take;
            }
            if crest_db.is_finite() {
                b.crest = b.crest.max(crest_db);
            }
            b.samples += take;
            samples -= take;
            if b.samples == self.bin_samples {
                self.current = (self.current + 1) % BINS;
                self.bins[self.current] = Bin::default();
            }
        }
    }
    /// Energy mean, excluding measurements at or below the -70 LUFS gate; -200 if empty.
    pub fn loudness(&self) -> f32 {
        let energy: f64 = self.bins.iter().map(|b| b.energy).sum();
        let heard: u64 = self.bins.iter().map(|b| b.heard).sum();
        if heard > 0 {
            (10.0 * (energy / heard as f64).log10()) as f32
        } else {
            -200.0
        }
    }
    /// Highest supplied crest in the retained history; -100 if empty.
    pub fn crest(&self) -> f32 {
        self.bins.iter().fold(-100.0_f32, |v, b| v.max(b.crest))
    }
}
