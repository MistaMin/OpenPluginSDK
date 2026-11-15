//! Downmix and upmix matrices between SDK speaker layouts and stereo or mono.
//!
//! Every fold works by channel *role* (the `SURROUND_*` ids of
//! `openplugin_core::layout`, which are the CLAP surround ids), never by
//! index. A layout contributes only its list of roles, in channel order, so a
//! layout added later (5.1.2, 5.1.4, 7.1.2, 9.1.6) folds correctly with no
//! change here as long as it is made of known roles. A role this module does
//! not know is silent in a downmix and ignored in an upmix.
//!
//! The matrix is built when the object is made or when the settings change
//! ([`Downmix::set_settings`], [`Upmix::set_settings`], which do not allocate
//! after construction). `process` is a plain matrix multiply: no allocation,
//! no locks, no branches on the role.
//!
//! # Coefficients and sources
//!
//! `c`, `s`, `h` and `l` are the centre, surround, height and LFE levels
//! ([`MixSettings`]); `g` is -3 dB, `0.70710678` (exactly 1/sqrt 2).
//!
//! | Source role | To L | To R | Source |
//! |---|---|---|---|
//! | `FL` / `FR` | 1 / 0 | 0 / 1 | ITU-R BS.775-3, Annex 4 |
//! | `FC` | `c` | `c` | BS.775-3 (`c` default -3 dB); ATSC A/52 `cmixlev` |
//! | `BL`, `SL` / `BR`, `SR` | `s` / 0 | 0 / `s` | BS.775-3 (`Ls`, `Rs` at -3 dB); A/52 `surmixlev` |
//! | `LFE` | `l` | `l` | BS.775-3 and A/52 leave LFE out; default dropped |
//! | `FLC` / `FRC` | `c` / 0 | 0 / `c` | not standardised, chosen here |
//! | `BC` (6.1) | `s*g` | `s*g` | not standardised; equal-power split of the back centre |
//! | `TFL`, `TBL` / `TFR`, `TBR` | `h` / 0 | 0 / `h` | not standardised, chosen here |
//! | `TC`, `TFC`, `TBC` | `h*g` | `h*g` | not standardised; equal-power split |
//!
//! Plain facts about the sources. BS.775-3 gives the 3/2 (5.x) to 2/0 fold
//! `L0 = L + 0.7071 C + 0.7071 Ls` (and the right-hand mirror); it allows
//! other surround coefficients (0.5 or 0) but 0.7071 is the one this module
//! defaults to. ATSC A/52 section 7.8 gives the same fold with `cmixlev` and
//! `surmixlev` of 0.707 (-3 dB), 0.595 (-4.5 dB) or 0.500 (-6 dB), and
//! scales the sum so it cannot clip; [`Normalize::Peak`] does the same.
//! Neither standard defines 6.1, 7.1 or height channels. The 7.1 side and back
//! pairs are each folded to their own side at the surround level (so a side
//! and a back that carry the same signal add by +6 dB at the output; use
//! normalisation or lower `surround_db` if that matters), which matches the
//! common practice of treating 7.1 as 5.1 with the surround pair doubled.
//! Heights follow the same "same side, -3 dB" habit used by Atmos-style
//! renderers, but the exact value is a choice, not a standard.
//!
//! Mono is the average of the two stereo outputs, `M = (L0 + R0) / 2`, so a
//! signal that is identical in both channels keeps its level. A true mono
//! source (layout `Mono`) is sent to both stereo outputs at unity.
//!
//! # Normalisation
//!
//! With [`Normalize::Off`] the coefficients are used as listed and a loud
//! multichannel input can exceed full scale. With [`Normalize::Peak`] every
//! coefficient is multiplied by `1 / max(1, worst)`, where `worst` is the
//! largest row sum of absolute coefficients. That is the exact gain at which
//! full-scale input on every channel at once still peaks at or below full
//! scale. It only ever attenuates, and for 5.0 at -3 dB it equals ATSC A/52's
//! own scaling, `1 / (1 + 0.7071 + 0.7071) = 0.4142`.
//!
//! # Upmix
//!
//! [`Upmix`] is the passive inverse for stereo to any layout. `FL` and `FR`
//! take `L` and `R` at unity (a mono target takes `(L + R) / 2`). The centre
//! can be fed from the mid signal `(L + R) / 2` at a level in dB (silent by
//! default). Surround channels (`BL`, `BR`, `SL`, `SR`) are silent by default,
//! or with [`SurroundFill::Side`] fed from the side signal `(L - R) / 2`
//! (left surrounds get `+S`, right surrounds `-S`). LFE, heights and all other
//! roles stay silent. With the defaults, an upmix followed by a stereo
//! downmix is the identity.

use openplugin_core::layout::{
    SURROUND_BC, SURROUND_BL, SURROUND_BR, SURROUND_FC, SURROUND_FL, SURROUND_FLC, SURROUND_FR,
    SURROUND_FRC, SURROUND_LFE, SURROUND_SL, SURROUND_SR, SURROUND_TBC, SURROUND_TBL, SURROUND_TBR,
    SURROUND_TC, SURROUND_TFC, SURROUND_TFL, SURROUND_TFR, SURROUND_TSL, SURROUND_TSR, SURROUND_FLW,
    SURROUND_FRW, SpeakerLayout,
};

/// Role of the single channel of a `Mono` layout. Not a CLAP surround id.
pub const ROLE_MONO: u8 = 255;

const G: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// -3.0103 dB, the level whose amplitude is exactly `1/sqrt(2)` (half power).
/// The defaults use this so "-3 dB" in BS.775 and A/52 is the 0.7071 they print.
pub const HALF_POWER_DB: f32 = -3.0103;

/// How the matrix is scaled against clipping. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Normalize {
    /// Coefficients as listed. Loud input can clip.
    #[default]
    Off,
    /// Divide by the worst-case absolute row sum when it exceeds one.
    Peak,
}

/// What the surround channels of an upmix carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurroundFill {
    /// Surrounds are silent.
    #[default]
    Silent,
    /// Surrounds carry the side signal `(L - R) / 2`, at `surround_db`.
    Side,
}

/// Levels in dB for a downmix. `f32::NEG_INFINITY` (or anything at or below
/// -120 dB, or NaN) means silent. Levels are clamped to at most +24 dB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixSettings {
    /// Centre (and front-centre-of-left/right) level. Default -3 dB.
    pub centre_db: f32,
    /// Surround level (sides, backs, back centre). Default -3 dB.
    pub surround_db: f32,
    /// Height level. Default -3 dB.
    pub height_db: f32,
    /// LFE level. Default silent: BS.775 and A/52 drop the LFE.
    pub lfe_db: f32,
    /// Clipping protection. Default off.
    pub normalize: Normalize,
}

impl Default for MixSettings {
    fn default() -> Self {
        Self {
            centre_db: HALF_POWER_DB,
            surround_db: HALF_POWER_DB,
            height_db: HALF_POWER_DB,
            lfe_db: f32::NEG_INFINITY,
            normalize: Normalize::Off,
        }
    }
}

/// Settings for an [`Upmix`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpmixSettings {
    /// Level of the mid signal sent to `FC`. Default silent.
    pub centre_db: f32,
    /// What the surrounds carry. Default silent.
    pub surround_fill: SurroundFill,
    /// Level of the side signal when `surround_fill` is `Side`. Default -3 dB.
    pub surround_db: f32,
}

impl Default for UpmixSettings {
    fn default() -> Self {
        Self {
            centre_db: f32::NEG_INFINITY,
            surround_fill: SurroundFill::Silent,
            surround_db: HALF_POWER_DB,
        }
    }
}

fn lin(db: f32) -> f32 {
    if db.is_nan() || db <= -120.0 {
        0.0
    } else {
        10f32.powf(db.min(24.0) / 20.0)
    }
}

/// Roles of a layout in channel order. `Mono` is `[ROLE_MONO]`; `Stereo` is
/// `FL FR`. `None` for layouts without a speaker map (ambisonic, discrete,
/// absent).
pub fn roles_of(layout: SpeakerLayout) -> Option<&'static [u8]> {
    match layout {
        SpeakerLayout::Mono => Some(&[ROLE_MONO]),
        SpeakerLayout::Stereo => Some(&[SURROUND_FL, SURROUND_FR]),
        other => other.surround_map(),
    }
}

/// Gain of one role into (left, right) of a stereo fold.
fn stereo_gains(role: u8, s: &MixSettings) -> (f32, f32) {
    let c = lin(s.centre_db);
    let sur = lin(s.surround_db);
    let h = lin(s.height_db);
    let l = lin(s.lfe_db);
    match role {
        ROLE_MONO => (1.0, 1.0),
        SURROUND_FL => (1.0, 0.0),
        SURROUND_FR => (0.0, 1.0),
        SURROUND_FC => (c, c),
        SURROUND_FLC => (c, 0.0),
        SURROUND_FRC => (0.0, c),
        SURROUND_LFE => (l, l),
        SURROUND_BL | SURROUND_SL => (sur, 0.0),
        SURROUND_BR | SURROUND_SR => (0.0, sur),
        SURROUND_BC => (sur * G, sur * G),
        SURROUND_TFL | SURROUND_TBL | SURROUND_TSL => (h, 0.0),
        SURROUND_TFR | SURROUND_TBR | SURROUND_TSR => (0.0, h),
        // Front wides sit between the fronts and the sides: half power to their own side.
        // Not standardised; the SDK's choice.
        SURROUND_FLW => (G, 0.0),
        SURROUND_FRW => (0.0, G),
        SURROUND_TC | SURROUND_TFC | SURROUND_TBC => (h * G, h * G),
        _ => (0.0, 0.0),
    }
}

fn is_left(role: u8) -> bool {
    matches!(
        role,
        SURROUND_FL | SURROUND_FLC | SURROUND_BL | SURROUND_SL | SURROUND_TFL | SURROUND_TBL
            | SURROUND_TSL | SURROUND_FLW
    )
}

fn is_surround(role: u8) -> bool {
    matches!(role, SURROUND_BL | SURROUND_BR | SURROUND_SL | SURROUND_SR)
}

/// Row-major `out x in` matrix applied sample by sample.
#[derive(Clone, Debug)]
struct Matrix {
    inputs: usize,
    outputs: usize,
    coef: Vec<f32>,
}

impl Matrix {
    fn new(outputs: usize, inputs: usize) -> Self {
        Self {
            inputs,
            outputs,
            coef: vec![0.0; outputs * inputs],
        }
    }

    fn normalize_peak(&mut self) {
        let worst = self
            .coef
            .chunks(self.inputs.max(1))
            .map(|row| row.iter().map(|v| v.abs()).sum::<f32>())
            .fold(0.0f32, f32::max);
        if worst > 1.0 {
            let k = 1.0 / worst;
            for v in &mut self.coef {
                *v *= k;
            }
        }
    }

    fn process(&self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        let mut frames = usize::MAX;
        for ch in input.iter().take(self.inputs) {
            frames = frames.min(ch.len());
        }
        for ch in output.iter().take(self.outputs) {
            frames = frames.min(ch.len());
        }
        if frames == usize::MAX {
            return;
        }
        for (o, out) in output.iter_mut().enumerate().take(self.outputs) {
            let out = &mut out[..frames];
            out.fill(0.0);
            let row = &self.coef[o * self.inputs..(o + 1) * self.inputs];
            for (i, &k) in row.iter().enumerate() {
                if k == 0.0 {
                    continue;
                }
                let Some(src) = input.get(i) else { continue };
                for (d, s) in out.iter_mut().zip(&src[..frames]) {
                    *d += k * *s;
                }
            }
        }
    }
}

/// Where a downmix goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownmixTarget {
    /// Two channels, L then R.
    Stereo,
    /// One channel, `(L0 + R0) / 2` of the stereo fold.
    Mono,
}

/// A layout folded to stereo or mono.
#[derive(Clone, Debug)]
pub struct Downmix {
    roles: Vec<u8>,
    target: DownmixTarget,
    settings: MixSettings,
    matrix: Matrix,
}

impl Downmix {
    /// Fold a layout with a speaker map (including `Mono` and `Stereo`).
    /// `None` for ambisonic, discrete or absent layouts.
    pub fn for_layout(
        layout: SpeakerLayout,
        target: DownmixTarget,
        settings: MixSettings,
    ) -> Option<Self> {
        Some(Self::from_roles(roles_of(layout)?, target, settings))
    }

    /// Fold an arbitrary list of channel roles, in channel order.
    pub fn from_roles(roles: &[u8], target: DownmixTarget, settings: MixSettings) -> Self {
        let outputs = match target {
            DownmixTarget::Stereo => 2,
            DownmixTarget::Mono => 1,
        };
        let mut me = Self {
            roles: roles.to_vec(),
            target,
            settings,
            matrix: Matrix::new(outputs, roles.len()),
        };
        me.rebuild();
        me
    }

    /// Change the levels. Does not allocate.
    pub fn set_settings(&mut self, settings: MixSettings) {
        self.settings = settings;
        self.rebuild();
    }

    pub fn settings(&self) -> MixSettings {
        self.settings
    }

    pub fn input_channels(&self) -> usize {
        self.matrix.inputs
    }

    pub fn output_channels(&self) -> usize {
        self.matrix.outputs
    }

    /// Matrix coefficient for output `out` from input channel `inp`.
    pub fn coefficient(&self, out: usize, inp: usize) -> f32 {
        self.matrix.coef[out * self.matrix.inputs + inp]
    }

    fn rebuild(&mut self) {
        let n = self.roles.len();
        for (i, &role) in self.roles.iter().enumerate() {
            let (l, r) = stereo_gains(role, &self.settings);
            match self.target {
                DownmixTarget::Stereo => {
                    self.matrix.coef[i] = l;
                    self.matrix.coef[n + i] = r;
                }
                DownmixTarget::Mono => self.matrix.coef[i] = 0.5 * (l + r),
            }
        }
        if self.settings.normalize == Normalize::Peak {
            self.matrix.normalize_peak();
        }
    }

    /// Multiply `input` (one slice per source channel) into `output` (one
    /// slice per target channel). Processes the shortest slice length.
    /// Allocation-free.
    pub fn process(&self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.matrix.process(input, output);
    }
}

/// Stereo spread to a layout. See the module docs.
#[derive(Clone, Debug)]
pub struct Upmix {
    roles: Vec<u8>,
    settings: UpmixSettings,
    matrix: Matrix,
}

impl Upmix {
    /// Upmix to a layout with a speaker map (including `Mono` and `Stereo`).
    pub fn for_layout(layout: SpeakerLayout, settings: UpmixSettings) -> Option<Self> {
        Some(Self::from_roles(roles_of(layout)?, settings))
    }

    /// Upmix to an arbitrary list of channel roles, in channel order.
    pub fn from_roles(roles: &[u8], settings: UpmixSettings) -> Self {
        let mut me = Self {
            roles: roles.to_vec(),
            settings,
            matrix: Matrix::new(roles.len(), 2),
        };
        me.rebuild();
        me
    }

    /// Change the levels. Does not allocate.
    pub fn set_settings(&mut self, settings: UpmixSettings) {
        self.settings = settings;
        self.rebuild();
    }

    pub fn settings(&self) -> UpmixSettings {
        self.settings
    }

    pub fn output_channels(&self) -> usize {
        self.matrix.outputs
    }

    /// Matrix coefficient for output channel `out` from input `inp` (0 = L, 1 = R).
    pub fn coefficient(&self, out: usize, inp: usize) -> f32 {
        self.matrix.coef[out * 2 + inp]
    }

    fn rebuild(&mut self) {
        let centre = lin(self.settings.centre_db);
        let sur = match self.settings.surround_fill {
            SurroundFill::Silent => 0.0,
            SurroundFill::Side => lin(self.settings.surround_db),
        };
        for (o, &role) in self.roles.iter().enumerate() {
            // (from L, from R)
            let (l, r) = match role {
                ROLE_MONO => (0.5, 0.5),
                SURROUND_FL => (1.0, 0.0),
                SURROUND_FR => (0.0, 1.0),
                SURROUND_FC => (0.5 * centre, 0.5 * centre),
                r if is_surround(r) => {
                    if is_left(r) {
                        (0.5 * sur, -0.5 * sur)
                    } else {
                        (-0.5 * sur, 0.5 * sur)
                    }
                }
                _ => (0.0, 0.0),
            };
            self.matrix.coef[o * 2] = l;
            self.matrix.coef[o * 2 + 1] = r;
        }
    }

    /// `input` is `[L, R]`; `output` has one slice per target channel.
    /// Allocation-free.
    pub fn process(&self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        self.matrix.process(input, output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-6;

    fn fold(dm: &Downmix, ins: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let refs: Vec<&[f32]> = ins.iter().map(|v| v.as_slice()).collect();
        let n = ins[0].len();
        let mut outs = vec![vec![0.0f32; n]; dm.output_channels()];
        let mut mrefs: Vec<&mut [f32]> = outs.iter_mut().map(|v| v.as_mut_slice()).collect();
        dm.process(&refs, &mut mrefs);
        outs
    }

    fn impulse(ch: usize, idx: usize) -> Vec<Vec<f32>> {
        let mut v = vec![vec![0.0f32; 1]; ch];
        v[idx][0] = 1.0;
        v
    }

    fn dm(layout: SpeakerLayout, target: DownmixTarget, s: MixSettings) -> Downmix {
        Downmix::for_layout(layout, target, s).unwrap()
    }

    #[test]
    fn bs775_five_one_coefficients() {
        let d = dm(
            SpeakerLayout::Surround51,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        // L R C LFE Ls Rs
        let expect_l = [1.0, 0.0, G, 0.0, G, 0.0];
        let expect_r = [0.0, 1.0, G, 0.0, 0.0, G];
        for i in 0..6 {
            assert!((d.coefficient(0, i) - expect_l[i]).abs() < 1e-5, "L {i}");
            assert!((d.coefficient(1, i) - expect_r[i]).abs() < 1e-5, "R {i}");
        }
        let out = fold(&d, &impulse(6, 2));
        assert!((out[0][0] - G).abs() < 1e-5 && (out[1][0] - G).abs() < 1e-5);
    }

    #[test]
    fn lfe_dropped_by_default_and_optional() {
        let mut d = dm(
            SpeakerLayout::Surround51,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        let out = fold(&d, &impulse(6, 3));
        assert_eq!((out[0][0], out[1][0]), (0.0, 0.0));
        d.set_settings(MixSettings {
            lfe_db: 0.0,
            ..MixSettings::default()
        });
        let out = fold(&d, &impulse(6, 3));
        assert!((out[0][0] - 1.0).abs() < EPS && (out[1][0] - 1.0).abs() < EPS);
    }

    #[test]
    fn levels_are_in_db() {
        let s = MixSettings {
            centre_db: -6.0,
            surround_db: -4.5,
            ..MixSettings::default()
        };
        let d = dm(SpeakerLayout::Surround50, DownmixTarget::Stereo, s);
        assert!((d.coefficient(0, 2) - 0.501187).abs() < 1e-5);
        assert!((d.coefficient(0, 3) - 0.595662).abs() < 1e-5); // A/52 surmixlev
    }

    #[test]
    fn six_one_seven_one_and_heights() {
        let s = MixSettings::default();
        let d = dm(SpeakerLayout::Surround61, DownmixTarget::Stereo, s);
        // BC is last (index 6): -3 dB then an equal-power split.
        assert!((d.coefficient(0, 6) - 0.5).abs() < 1e-5);
        assert!((d.coefficient(1, 6) - 0.5).abs() < 1e-5);
        let d = dm(SpeakerLayout::Surround71, DownmixTarget::Stereo, s);
        // L R C LFE SL SR BL BR: sides and backs fold to their own side.
        assert!((d.coefficient(0, 4) - G).abs() < 1e-5);
        assert_eq!(d.coefficient(1, 4), 0.0);
        assert!((d.coefficient(1, 7) - G).abs() < 1e-5);
        let d = dm(SpeakerLayout::Surround714, DownmixTarget::Stereo, s);
        // heights TFL TFR TBL TBR at indices 8..12
        assert!((d.coefficient(0, 8) - G).abs() < 1e-5);
        assert!((d.coefficient(1, 9) - G).abs() < 1e-5);
        assert!((d.coefficient(0, 10) - G).abs() < 1e-5);
        assert!((d.coefficient(1, 11) - G).abs() < 1e-5);
    }

    #[test]
    fn mono_target_and_mono_source() {
        let d = dm(
            SpeakerLayout::Stereo,
            DownmixTarget::Mono,
            MixSettings::default(),
        );
        let out = fold(&d, &[vec![0.6], vec![0.2]]);
        assert!((out[0][0] - 0.4).abs() < EPS);
        let d = dm(
            SpeakerLayout::Mono,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        let out = fold(&d, &[vec![0.5]]);
        assert_eq!((out[0][0], out[1][0]), (0.5, 0.5));
        let d = dm(
            SpeakerLayout::Surround50,
            DownmixTarget::Mono,
            MixSettings::default(),
        );
        let out = fold(&d, &impulse(5, 2));
        assert!((out[0][0] - G).abs() < 1e-5);
    }

    #[test]
    fn non_speaker_layouts_have_no_fold() {
        for l in [
            SpeakerLayout::Absent,
            SpeakerLayout::Ambisonic(1),
            SpeakerLayout::Discrete(4),
        ] {
            assert!(
                Downmix::for_layout(l, DownmixTarget::Stereo, MixSettings::default()).is_none()
            );
            assert!(Upmix::for_layout(l, UpmixSettings::default()).is_none());
        }
    }

    #[test]
    fn role_only_map_works_for_any_role_list() {
        // A layout that does not exist in the SDK, in an unusual order, plus
        // an unknown role that must stay silent.
        let roles = [
            SURROUND_TFL,
            SURROUND_FR,
            200,
            SURROUND_SR,
            SURROUND_FC,
            SURROUND_LFE,
            SURROUND_FL,
        ];
        let s = MixSettings {
            lfe_db: -6.0,
            ..MixSettings::default()
        };
        let d = Downmix::from_roles(&roles, DownmixTarget::Stereo, s);
        assert_eq!(d.input_channels(), 7);
        assert!((d.coefficient(0, 0) - G).abs() < 1e-5); // height to left
        assert_eq!(d.coefficient(1, 0), 0.0);
        assert_eq!((d.coefficient(0, 2), d.coefficient(1, 2)), (0.0, 0.0));
        assert!((d.coefficient(1, 3) - G).abs() < 1e-5);
        assert!((d.coefficient(0, 4) - G).abs() < 1e-5);
        assert!((d.coefficient(0, 5) - 0.501187).abs() < 1e-5);
        assert_eq!(d.coefficient(0, 6), 1.0);
        assert_eq!(d.coefficient(1, 6), 0.0);
    }

    #[test]
    fn peak_normalisation_prevents_clipping() {
        let s = MixSettings {
            normalize: Normalize::Peak,
            ..MixSettings::default()
        };
        let d = dm(SpeakerLayout::Surround714, DownmixTarget::Stereo, s);
        let ins = vec![vec![1.0f32]; 12];
        let out = fold(&d, &ins);
        assert!(out[0][0] <= 1.0 + 1e-5 && out[1][0] <= 1.0 + 1e-5);
        assert!(
            (out[0][0] - 1.0).abs() < 1e-5,
            "worst row lands on full scale"
        );
        let d = dm(SpeakerLayout::Surround50, DownmixTarget::Stereo, s);
        assert!((d.coefficient(0, 0) - 1.0 / (1.0 + 2.0 * G)).abs() < 1e-5);
        let d = dm(
            SpeakerLayout::Surround714,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        assert!(fold(&d, &ins)[0][0] > 1.0);
        let d = dm(SpeakerLayout::Stereo, DownmixTarget::Stereo, s);
        assert_eq!(d.coefficient(0, 0), 1.0);
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            // uniform in [-sqrt(3), sqrt(3)]: unit variance
            (((self.0 >> 40) as f32) / (1u64 << 24) as f32 * 2.0 - 1.0) * 3f32.sqrt()
        }
    }

    #[test]
    fn uncorrelated_equal_level_five_oh_folds_to_expected_power() {
        let n = 200_000;
        let mut rng = Lcg(7);
        let ins: Vec<Vec<f32>> = (0..5)
            .map(|_| (0..n).map(|_| rng.next()).collect())
            .collect();
        let d = dm(
            SpeakerLayout::Surround50,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        let out = fold(&d, &ins);
        let power = |v: &[f32]| v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len() as f64;
        assert!((power(&ins[0]) - 1.0).abs() < 0.02);
        // L0 = L + g C + g Ls: power 1 + 0.5 + 0.5 = 2 (+3 dB).
        for ch in &out {
            let p = power(ch);
            assert!((p - 2.0).abs() < 0.06, "power {p}");
        }
    }

    #[test]
    fn stereo_upmix_then_downmix_is_identity() {
        let l: Vec<f32> = (0..64).map(|i| (i as f32 * 0.31).sin()).collect();
        let r: Vec<f32> = (0..64).map(|i| (i as f32 * 0.17).cos() * 0.5).collect();
        for layout in [
            SpeakerLayout::Stereo,
            SpeakerLayout::Lcr,
            SpeakerLayout::Quad,
            SpeakerLayout::Surround51,
            SpeakerLayout::Surround71,
            SpeakerLayout::Surround714,
        ] {
            let up = Upmix::for_layout(layout, UpmixSettings::default()).unwrap();
            let mut mid = vec![vec![0.0f32; 64]; up.output_channels()];
            {
                let mut m: Vec<&mut [f32]> = mid.iter_mut().map(|v| v.as_mut_slice()).collect();
                up.process(&[&l, &r], &mut m);
            }
            let d = dm(layout, DownmixTarget::Stereo, MixSettings::default());
            let out = fold(&d, &mid);
            for k in 0..64 {
                assert!((out[0][k] - l[k]).abs() < EPS && (out[1][k] - r[k]).abs() < EPS);
            }
        }
    }

    #[test]
    fn upmix_centre_and_side_fill() {
        let s = UpmixSettings {
            centre_db: HALF_POWER_DB,
            surround_fill: SurroundFill::Side,
            surround_db: 0.0,
        };
        let up = Upmix::for_layout(SpeakerLayout::Surround51, s).unwrap();
        // L R C LFE Ls Rs
        let mut o = vec![vec![0.0f32; 1]; 6];
        {
            let mut m: Vec<&mut [f32]> = o.iter_mut().map(|v| v.as_mut_slice()).collect();
            up.process(&[&[1.0], &[0.0]], &mut m);
        }
        assert_eq!(o[0][0], 1.0);
        assert_eq!(o[1][0], 0.0);
        assert!((o[2][0] - 0.5 * G).abs() < 1e-5); // mid = 0.5, at -3 dB
        assert_eq!(o[3][0], 0.0); // LFE silent
        assert_eq!((o[4][0], o[5][0]), (0.5, -0.5)); // side = 0.5
        let up = Upmix::for_layout(SpeakerLayout::Mono, UpmixSettings::default()).unwrap();
        let mut m1 = vec![0.0f32; 1];
        up.process(&[&[1.0], &[0.0]], &mut [m1.as_mut_slice()]);
        assert_eq!(m1[0], 0.5);
    }

    #[test]
    fn short_slices_do_not_panic() {
        let d = dm(
            SpeakerLayout::Surround51,
            DownmixTarget::Stereo,
            MixSettings::default(),
        );
        let a = [1.0f32; 4];
        let b = [1.0f32; 2];
        let mut l = [9.0f32; 4];
        let mut r = [9.0f32; 3];
        d.process(&[&a, &b, &a, &a, &a, &a], &mut [&mut l, &mut r]);
        assert_eq!(l[3], 9.0); // only the shortest length was written
        d.process(&[], &mut []);
    }

    #[test]
    fn atmos_beds_fold_every_channel() {
        for layout in [
            SpeakerLayout::Surround512,
            SpeakerLayout::Surround514,
            SpeakerLayout::Surround712,
            SpeakerLayout::Surround916,
        ] {
            let d = dm(layout, DownmixTarget::Stereo, MixSettings::default());
            for (i, role) in layout.roles().iter().enumerate() {
                if *role == SURROUND_LFE {
                    continue;
                }
                let l = d.coefficient(0, i);
                let r = d.coefficient(1, i);
                assert!(l > 0.0 || r > 0.0, "{layout:?} channel {i} (role {role}) is dropped");
            }
        }
    }
}
