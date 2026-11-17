//! Wave digital filter (WDF) building blocks and a diode clipper example.
//!
//! References: Alfred Fettweis, "Wave digital filters: theory and practice",
//! Proc. IEEE 74(2), 1986 (waves, adaptors, bilinear-transformed reactances);
//! Kurt Werner et al., "An Improved and Generalized Diode Clipper Model for
//! Wave Digital Filters", 139th AES Convention, 2015 (diode pair as a
//! nonlinear root, solved in closed form with the Wright omega function).
//!
//! # Conventions
//!
//! Voltage waves at a port with reference resistance `R`, port voltage `v` and
//! current `i` flowing into the element: `a = v + R i` (incident),
//! `b = v - R i` (reflected). Elements:
//!
//! | element | `R` | `b` |
//! | --- | --- | --- |
//! | [`Resistor`] | `R` | `0` |
//! | [`Capacitor`] | `1 / (2 C fs)` | `z` (the last incident wave) |
//! | [`Inductor`] | `2 L fs` | `-z` |
//! | [`ResistiveVoltageSource`] | `Rs` | `e` |
//!
//! Three-port adaptors are adapted at port 0 (towards the root), so their
//! upward reflection does not depend on the incident wave from the root:
//!
//! * [`Series`]: `R0 = R1 + R2`, `b0 = -(a1 + a2)`, `b_k = a_k - (R_k/R0)(a0 + a1 + a2)`.
//! * [`Parallel`]: `G0 = G1 + G2`, `b0 = (G1 a1 + G2 a2)/G0`, `b_k = a0 + b0 - a_k`.
//! * [`Inverter`]: ideal polarity inverter (two-port): `b0 = -a1`, `b1 = -a0`.
//!
//! A tree is evaluated per sample as: `a = tree.reflect()` (waves travel up to
//! the root), the root computes `b` from `a`, then `tree.incident(b)` (waves
//! travel down). [`DiodePair::process`] and [`SingleDiode::process`] do that.
//! Trees are built from generic types, so there is no allocation or dynamic
//! dispatch. After changing a component value call [`WdfPort::refresh`] on the
//! tree top so the adaptors recompute their coefficients.
//!
//! # Diode roots
//!
//! For an antiparallel pair `i = 2 Is sinh(v / Vt)` (here `Vt` already includes
//! the ideality factor). Substituting `v = (a+b)/2`, `i = (a-b)/(2R)` and keeping
//! the conducting exponential only gives (Werner et al. 2015, eq. for the pair)
//! `b = a + 2 lambda R Is - 2 lambda Vt W( (R Is / Vt) exp((lambda a + R Is)/Vt) )`
//! with `lambda = sign(a)` and `W` Lambert's function; we evaluate
//! `W(e^x)` as the Wright omega function [`wright_omega`] (no overflow for large
//! arguments). We re-derived this: with `v>0` the equation `(a-v)/R = Is e^{v/Vt}`
//! has the solution `v = a + R Is - Vt W((R Is/Vt) e^{(a+R Is)/Vt})`.
//! The neglected reverse current is of order `Is` and is removed by two Newton steps
//! on the exact `sinh` equation (the steps start within `~Is R`
//! of the root), so [`DiodePair::reflect`] solves the full pair equation.
//! [`SingleDiode`] is the exact closed form for `i = Is (exp(v/Vt) - 1)`.
//!
//! All state is `f64`; a non-finite incident wave is treated as 0.

use crate::rate::effective_rate;
use crate::svf::flush;

const R_MIN: f64 = 1e-9;

/// Wright omega function `omega(x) = W(e^x)` for real `x`: the solution `w > 0`
/// of `w + ln w = x`. Computed by Newton's method on `z = ln w`
/// (`e^z + z = x`, convex and increasing, so the iteration is monotone from the
/// right of the root), from a start that is within a few percent of the root,
/// at most 8 iterations. Accurate to about `1e-15` relative; `x` is clamped to
/// `+-1e300`, NaN gives 0.
pub fn wright_omega(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    let x = x.clamp(-1e300, 1e300);
    if x < -745.0 {
        return 0.0;
    }
    let mut z = if x > 1.0 { (x - x.ln()).ln() } else { x };
    for _ in 0..8 {
        let e = z.exp();
        let step = (e + z - x) / (e + 1.0);
        z -= step;
        if step.abs() < 1e-15 * (1.0 + z.abs()) {
            break;
        }
    }
    z.exp()
}

/// A one-port or a subtree of a wave digital filter, adapted towards its parent.
pub trait WdfPort {
    /// Port resistance seen by the parent.
    fn resistance(&self) -> f64;
    /// Reflected wave `b` going up (reads the children's reflections).
    fn reflect(&mut self) -> f64;
    /// Incident wave `a` coming down; updates states.
    fn incident(&mut self, a: f64);
    /// Sets the sample rate for reactive elements.
    fn set_sample_rate(&mut self, _sample_rate: f32) {}
    /// Recomputes coefficients after a component value changed.
    fn refresh(&mut self) {}
    /// Clears the state.
    fn reset(&mut self) {}
}

/// Resistor; reflects nothing.
#[derive(Debug, Clone)]
pub struct Resistor {
    r: f64,
}
impl Resistor {
    /// Resistance in ohms (clamped to a positive value).
    pub fn new(ohms: f64) -> Self {
        Self { r: ohms.max(R_MIN) }
    }
    /// Changes the resistance (then call `refresh` on the tree).
    pub fn set_resistance(&mut self, ohms: f64) {
        if ohms.is_finite() {
            self.r = ohms.max(R_MIN);
        }
    }
}
impl WdfPort for Resistor {
    fn resistance(&self) -> f64 {
        self.r
    }
    fn reflect(&mut self) -> f64 {
        0.0
    }
    fn incident(&mut self, _a: f64) {}
}

/// Capacitor, bilinear transform: `R = 1 / (2 C fs)`, `b[n] = a[n-1]`.
#[derive(Debug, Clone)]
pub struct Capacitor {
    c: f64,
    fs: f64,
    r: f64,
    z: f64,
}
impl Capacitor {
    /// Capacitance in farads at `sample_rate`.
    pub fn new(farads: f64, sample_rate: f32) -> Self {
        let mut s = Self { c: farads.max(1e-15), fs: f64::from(effective_rate(sample_rate)), r: 1.0, z: 0.0 };
        s.refresh();
        s
    }
    /// Changes the capacitance (then call `refresh` on the tree).
    pub fn set_capacitance(&mut self, farads: f64) {
        if farads.is_finite() {
            self.c = farads.max(1e-15);
            self.refresh();
        }
    }
}
impl WdfPort for Capacitor {
    fn resistance(&self) -> f64 {
        self.r
    }
    fn reflect(&mut self) -> f64 {
        self.z
    }
    fn incident(&mut self, a: f64) {
        self.z = flush(a);
    }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        self.refresh();
    }
    fn refresh(&mut self) {
        self.r = (1.0 / (2.0 * self.c * self.fs)).max(R_MIN);
    }
    fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// Inductor, bilinear transform: `R = 2 L fs`, `b[n] = -a[n-1]`.
#[derive(Debug, Clone)]
pub struct Inductor {
    l: f64,
    fs: f64,
    r: f64,
    z: f64,
}
impl Inductor {
    /// Inductance in henries at `sample_rate`.
    pub fn new(henries: f64, sample_rate: f32) -> Self {
        let mut s = Self { l: henries.max(1e-15), fs: f64::from(effective_rate(sample_rate)), r: 1.0, z: 0.0 };
        s.refresh();
        s
    }
    /// Changes the inductance (then call `refresh` on the tree).
    pub fn set_inductance(&mut self, henries: f64) {
        if henries.is_finite() {
            self.l = henries.max(1e-15);
            self.refresh();
        }
    }
}
impl WdfPort for Inductor {
    fn resistance(&self) -> f64 {
        self.r
    }
    fn reflect(&mut self) -> f64 {
        -self.z
    }
    fn incident(&mut self, a: f64) {
        self.z = flush(a);
    }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.fs = f64::from(effective_rate(sample_rate));
        self.refresh();
    }
    fn refresh(&mut self) {
        self.r = (2.0 * self.l * self.fs).max(R_MIN);
    }
    fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// Voltage source `e` with series resistance `Rs`; adapted, `b = e`.
#[derive(Debug, Clone)]
pub struct ResistiveVoltageSource {
    r: f64,
    e: f64,
}
impl ResistiveVoltageSource {
    /// Source resistance in ohms; the voltage starts at 0.
    pub fn new(ohms: f64) -> Self {
        Self { r: ohms.max(R_MIN), e: 0.0 }
    }
    /// Sets the source voltage (a non-finite value is treated as 0).
    pub fn set_voltage(&mut self, volts: f64) {
        self.e = if volts.is_finite() { volts } else { 0.0 };
    }
    /// Changes the resistance (then call `refresh` on the tree).
    pub fn set_resistance(&mut self, ohms: f64) {
        if ohms.is_finite() {
            self.r = ohms.max(R_MIN);
        }
    }
}
impl WdfPort for ResistiveVoltageSource {
    fn resistance(&self) -> f64 {
        self.r
    }
    fn reflect(&mut self) -> f64 {
        self.e
    }
    fn incident(&mut self, _a: f64) {}
}

/// Series three-port adaptor, adapted at the parent port.
#[derive(Debug, Clone)]
pub struct Series<A: WdfPort, B: WdfPort> {
    /// First child.
    pub a: A,
    /// Second child.
    pub b: B,
    r0: f64,
    ka: f64,
    kb: f64,
    wa: f64,
    wb: f64,
}
impl<A: WdfPort, B: WdfPort> Series<A, B> {
    /// Connects two children in series.
    pub fn new(a: A, b: B) -> Self {
        let mut s = Self { a, b, r0: 1.0, ka: 0.5, kb: 0.5, wa: 0.0, wb: 0.0 };
        s.refresh_local();
        s
    }
    fn refresh_local(&mut self) {
        let (ra, rb) = (self.a.resistance(), self.b.resistance());
        self.r0 = ra + rb;
        self.ka = ra / self.r0;
        self.kb = rb / self.r0;
    }
}
impl<A: WdfPort, B: WdfPort> WdfPort for Series<A, B> {
    fn resistance(&self) -> f64 {
        self.r0
    }
    fn reflect(&mut self) -> f64 {
        self.wa = self.a.reflect();
        self.wb = self.b.reflect();
        -(self.wa + self.wb)
    }
    fn incident(&mut self, a0: f64) {
        let sum = a0 + self.wa + self.wb;
        self.a.incident(self.wa - self.ka * sum);
        self.b.incident(self.wb - self.kb * sum);
    }
    fn set_sample_rate(&mut self, sr: f32) {
        self.a.set_sample_rate(sr);
        self.b.set_sample_rate(sr);
        self.refresh_local();
    }
    fn refresh(&mut self) {
        self.a.refresh();
        self.b.refresh();
        self.refresh_local();
    }
    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// Parallel three-port adaptor, adapted at the parent port.
#[derive(Debug, Clone)]
pub struct Parallel<A: WdfPort, B: WdfPort> {
    /// First child.
    pub a: A,
    /// Second child.
    pub b: B,
    r0: f64,
    pa: f64,
    pb: f64,
    wa: f64,
    wb: f64,
    up: f64,
}
impl<A: WdfPort, B: WdfPort> Parallel<A, B> {
    /// Connects two children in parallel.
    pub fn new(a: A, b: B) -> Self {
        let mut s = Self { a, b, r0: 1.0, pa: 0.5, pb: 0.5, wa: 0.0, wb: 0.0, up: 0.0 };
        s.refresh_local();
        s
    }
    fn refresh_local(&mut self) {
        let (ga, gb) = (1.0 / self.a.resistance(), 1.0 / self.b.resistance());
        let g0 = ga + gb;
        self.r0 = 1.0 / g0;
        self.pa = ga / g0;
        self.pb = gb / g0;
    }
}
impl<A: WdfPort, B: WdfPort> WdfPort for Parallel<A, B> {
    fn resistance(&self) -> f64 {
        self.r0
    }
    fn reflect(&mut self) -> f64 {
        self.wa = self.a.reflect();
        self.wb = self.b.reflect();
        self.up = self.pa * self.wa + self.pb * self.wb;
        self.up
    }
    fn incident(&mut self, a0: f64) {
        let t = a0 + self.up;
        self.a.incident(t - self.wa);
        self.b.incident(t - self.wb);
    }
    fn set_sample_rate(&mut self, sr: f32) {
        self.a.set_sample_rate(sr);
        self.b.set_sample_rate(sr);
        self.refresh_local();
    }
    fn refresh(&mut self) {
        self.a.refresh();
        self.b.refresh();
        self.refresh_local();
    }
    fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// Ideal polarity inverter in front of one child.
#[derive(Debug, Clone)]
pub struct Inverter<A: WdfPort> {
    /// The inverted child.
    pub a: A,
}
impl<A: WdfPort> Inverter<A> {
    /// Wraps a child.
    pub fn new(a: A) -> Self {
        Self { a }
    }
}
impl<A: WdfPort> WdfPort for Inverter<A> {
    fn resistance(&self) -> f64 {
        self.a.resistance()
    }
    fn reflect(&mut self) -> f64 {
        -self.a.reflect()
    }
    fn incident(&mut self, a0: f64) {
        self.a.incident(-a0);
    }
    fn set_sample_rate(&mut self, sr: f32) {
        self.a.set_sample_rate(sr);
    }
    fn refresh(&mut self) {
        self.a.refresh();
    }
    fn reset(&mut self) {
        self.a.reset();
    }
}

/// Diode model parameters: saturation current (A) and `n Vt` (V).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiodeParams {
    /// Saturation current `Is`.
    pub is: f64,
    /// Ideality factor times thermal voltage.
    pub nvt: f64,
}
impl DiodeParams {
    /// 1N4148 as fitted by Werner et al. 2015 (`Is` 2.52 nA, `n` 1.752, `Vt` 25.85 mV).
    pub const D1N4148: DiodeParams = DiodeParams { is: 2.52e-9, nvt: 1.752 * 0.02585 };
}

#[inline]
fn sanitize(a: f64) -> f64 {
    if a.is_finite() {
        a
    } else {
        0.0
    }
}

/// Antiparallel diode pair, the nonlinear root of a tree.
#[derive(Debug, Clone, Copy)]
pub struct DiodePair {
    p: DiodeParams,
}
impl DiodePair {
    /// A pair with the given parameters. A constant that is not finite and
    /// positive is replaced by the 1N4148's.
    pub fn new(p: DiodeParams) -> Self {
        let ok = |v: f64, fallback: f64| if v.is_finite() && v > 0.0 { v } else { fallback };
        let d = DiodeParams::D1N4148;
        Self { p: DiodeParams { is: ok(p.is, d.is), nvt: ok(p.nvt, d.nvt) } }
    }
    /// Reflected wave for incident `a` on a port of resistance `r`.
    pub fn reflect(&self, a: f64, r: f64) -> f64 {
        let a = sanitize(a);
        let (is, vt) = (self.p.is, self.p.nvt);
        let lam = if a < 0.0 { -1.0 } else { 1.0 };
        let ris = r * is;
        let w = wright_omega((ris / vt).ln() + (lam * a + ris) / vt);
        let b = a + 2.0 * lam * ris - 2.0 * lam * vt * w;
        // Two Newton steps on the exact sinh equation f(v) = (a - v)/r - 2 Is sinh(v/Vt).
        let mut v = (0.5 * (a + b)).clamp(-40.0 * vt, 40.0 * vt);
        for _ in 0..2 {
            let f = (a - v) / r - 2.0 * is * (v / vt).sinh();
            let df = -1.0 / r - 2.0 * is / vt * (v / vt).cosh();
            v -= f / df;
        }
        2.0 * v - a
    }
    /// Runs one step of the tree against this root; returns the voltage across the diodes.
    pub fn process<T: WdfPort>(&self, tree: &mut T) -> f64 {
        let a = tree.reflect();
        let b = self.reflect(a, tree.resistance());
        tree.incident(b);
        0.5 * (sanitize(a) + b)
    }
}

/// Single diode, exact closed form for `i = Is (exp(v/Vt) - 1)`.
#[derive(Debug, Clone, Copy)]
pub struct SingleDiode {
    p: DiodeParams,
}
impl SingleDiode {
    /// A diode with the given parameters.
    pub fn new(p: DiodeParams) -> Self {
        Self { p }
    }
    /// Reflected wave for incident `a` on a port of resistance `r`.
    pub fn reflect(&self, a: f64, r: f64) -> f64 {
        let a = sanitize(a);
        let (is, vt) = (self.p.is, self.p.nvt);
        let ris = r * is;
        let w = wright_omega((ris / vt).ln() + (a + ris) / vt);
        a + 2.0 * ris - 2.0 * vt * w
    }
    /// Runs one step of the tree against this root; returns the diode voltage.
    pub fn process<T: WdfPort>(&self, tree: &mut T) -> f64 {
        let a = tree.reflect();
        let b = self.reflect(a, tree.resistance());
        tree.incident(b);
        0.5 * (sanitize(a) + b)
    }
}

/// The classic diode clipper: a voltage source with series resistance driving a
/// capacitor in parallel with an antiparallel diode pair (Werner et al. 2015,
/// Fig. 1). The tree is `Parallel(ResistiveVoltageSource, Capacitor)` with the
/// diode pair as root. Defaults: 2.2 kohm, 10 nF, 1N4148 (corner 7.2 kHz).
#[derive(Debug, Clone)]
pub struct DiodeClipper {
    tree: Parallel<ResistiveVoltageSource, Capacitor>,
    root: DiodePair,
}
impl DiodeClipper {
    /// A clipper at `sample_rate` with the default components.
    pub fn new(sample_rate: f32) -> Self {
        Self::with_components(sample_rate, 2200.0, 10e-9, DiodeParams::D1N4148)
    }
    /// A clipper with chosen resistance (ohms), capacitance (farads) and diodes.
    pub fn with_components(sample_rate: f32, ohms: f64, farads: f64, diode: DiodeParams) -> Self {
        Self {
            tree: Parallel::new(ResistiveVoltageSource::new(ohms), Capacitor::new(farads, sample_rate)),
            root: DiodePair::new(diode),
        }
    }
    /// Sets the host rate (clamped by [`effective_rate`]).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.tree.set_sample_rate(sample_rate);
    }
    /// Sets the source resistance.
    pub fn set_resistance(&mut self, ohms: f64) {
        self.tree.a.set_resistance(ohms);
        self.tree.refresh();
    }
    /// Sets the capacitance.
    pub fn set_capacitance(&mut self, farads: f64) {
        self.tree.b.set_capacitance(farads);
        self.tree.refresh();
    }
    /// Clears the state.
    pub fn reset(&mut self) {
        self.tree.reset();
    }
    /// Input in volts (a sample of 1.0 is 1 V); output is the voltage across the diodes.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let vin = if input.is_finite() { f64::from(input) } else { 0.0 };
        self.tree.a.set_voltage(vin);
        let v = self.root.process(&mut self.tree);
        if v.is_finite() && v.abs() < 1e6 {
            v as f32
        } else {
            self.reset();
            0.0
        }
    }
    /// Processes a buffer in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf {
            *v = self.process_sample(*v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omega_solves_its_equation() {
        let mut x = -700.0;
        while x < 1e6 {
            let w = wright_omega(x);
            if x > -30.0 {
                let r = (w + w.ln() - x).abs() / (1.0 + x.abs());
                assert!(r < 1e-13, "x={x} w={w} r={r}");
            } else {
                assert!((w / x.exp() - 1.0).abs() < 1e-10, "x={x}");
            }
            x = if x < 0.0 { x * 0.7 + 0.01 } else { x * 1.3 + 0.1 };
        }
        assert!((wright_omega(1.0) - 1.0).abs() < 1e-14);
    }

    #[test]
    fn rc_without_diode_matches_bilinear_lowpass() {
        // Resistive source into a capacitor only: a one-pole low-pass.
        let fs = 48_000.0f32;
        let mut tree = Parallel::new(ResistiveVoltageSource::new(1000.0), Capacitor::new(100e-9, fs));
        let mut v = 0.0;
        for _ in 0..4000 {
            tree.a.set_voltage(1.0);
            let a = tree.reflect();
            // An open root reflects everything.
            tree.incident(a);
            v = 0.5 * (a + a);
        }
        assert!((v - 1.0).abs() < 1e-6, "{v}");
    }

    #[test]
    fn diode_pair_satisfies_its_equation() {
        let d = DiodePair::new(DiodeParams::D1N4148);
        for &a in &[-50.0, -3.0, -0.5, -1e-3, 0.0, 1e-6, 0.2, 1.0, 5.0, 100.0] {
            let r = 2200.0;
            let b = d.reflect(a, r);
            let v = 0.5 * (a + b);
            let i = (a - b) / (2.0 * r);
            let id = 2.0 * 2.52e-9 * (v / (1.752 * 0.02585)).sinh();
            assert!((i - id).abs() <= 1e-9 * i.abs().max(1e-9) + 1e-15, "a={a} i={i} id={id}");
        }
    }

    #[test]
    fn diode_pair_with_bad_constants_falls_back_to_the_1n4148() {
        let good = DiodePair::new(DiodeParams::D1N4148).reflect(0.5, 2200.0);
        for bad in [0.0, -0.045, f64::NAN, f64::INFINITY] {
            let d = DiodePair::new(DiodeParams { nvt: bad, ..DiodeParams::D1N4148 });
            assert_eq!(d.reflect(0.5, 2200.0), good, "nvt = {bad}");
            let d = DiodePair::new(DiodeParams { is: bad, ..DiodeParams::D1N4148 });
            assert_eq!(d.reflect(0.5, 2200.0), good, "is = {bad}");
        }
    }
}
