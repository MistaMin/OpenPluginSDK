//! A small deterministic generator (SplitMix64). The kit never uses the clock
//! or the OS for randomness, so a seed reproduces a run exactly.

/// SplitMix64 generator: cheap, small state, good enough for test input.
///
/// The same seed gives the same sequence on every platform. Not for cryptography.
///
/// ```
/// use openplugin_testing::rng::Rng;
///
/// let mut a = Rng::new(7);
/// let mut b = Rng::new(7);
/// assert_eq!(a.next_u64(), b.next_u64());
///
/// let x = a.range(-24.0, 24.0);
/// assert!((-24.0..24.0).contains(&x));
/// assert!(a.below(10) < 10);
/// ```
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// A generator whose sequence is fixed by `seed`. Every seed, zero included, is valid.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 uniformly distributed bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[-1, 1)`.
    pub fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// Uniform in `[lo, hi)`. With `lo > hi` the result lies in `(hi, lo]`; with `lo == hi`
    /// it is `lo`. Non-finite bounds give non-finite results.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    /// Uniform in `0..n` (`n` of zero is treated as one).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    /// True with probability `p` (0..=1): never for `p <= 0` or NaN, always for `p >= 1`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }
}

/// FNV-1a over the label, mixed with the master seed: one seed per named check, so adding a
/// check does not change the random input of the others.
///
/// ```
/// use openplugin_testing::rng::derive_seed;
///
/// assert_eq!(derive_seed(1, "fuzz"), derive_seed(1, "fuzz"));
/// assert_ne!(derive_seed(1, "fuzz"), derive_seed(1, "state"));
/// ```
pub fn derive_seed(seed: u64, label: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for b in label.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Rng::new(h).next_u64()
}
