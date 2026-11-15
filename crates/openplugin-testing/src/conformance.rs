//! The conformance suite. See the crate docs and `docs/TESTING_PLUGINS.md`.

use std::panic::{catch_unwind, AssertUnwindSafe};

use openplugin_core::{
    decode_params, encode_params, restore_params, IoConfig, ParamFlags, ParamInfo, Plugin, PluginInfo, Tail,
    Transport, STEREO_LAYOUTS,
};

use crate::alloc;
use crate::report::{Check, Recorder, Report};
use crate::rng::{derive_seed, Rng};

/// Sample rates hosts run at, in hertz.
pub const HOST_SAMPLE_RATES: [f32; 8] =
    [22_050.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0, 384_000.0];

/// Block sizes hosts use, including the awkward ones.
pub const HOST_BLOCK_SIZES: [usize; 11] = [1, 2, 3, 7, 16, 64, 333, 512, 1024, 4096, 8192];

/// What the suite does. [`Options::default`] is the full run.
#[derive(Clone, Debug)]
pub struct Options {
    /// Master seed. Everything the suite randomises derives from it.
    pub seed: u64,
    pub sample_rates: Vec<f32>,
    pub block_sizes: Vec<usize>,
    /// Largest absolute output sample allowed. Default 64.0 (about +36 dBFS).
    pub max_output: f32,
    /// Require finite output *during* a NaN or infinity burst, not only
    /// after it has passed. Default false: a plugin that passes NaN through
    /// is allowed as long as it recovers.
    pub strict_hostile_output: bool,
    /// Treat denormal output for denormal input as a failure. Default false.
    pub strict_denormals: bool,
    /// Output level, in dBFS, that counts as silent for the tail and reset checks.
    pub silence_db: f32,
    /// Grace after the reported tail, in milliseconds, before the output must be
    /// silent. A filter whose tail is reported as zero still rings for a few
    /// samples; hosts do not care about those. Default 10 ms.
    pub tail_slack_ms: f32,
    /// Tails longer than this many seconds are not run (reported as a note).
    pub max_tail_seconds: f32,
    /// Rates the tail and reset checks run at (those the plugin accepts).
    pub tail_sample_rates: Vec<f32>,
    /// Allow `latency()` to change when a parameter is written. Default false.
    pub latency_depends_on_params: bool,
    /// Also measure latency with an impulse and compare it with `latency()`.
    /// Only meaningful for plugins whose impulse response peaks at the
    /// reported latency (delays, linear-phase filters, look-ahead limiters).
    pub verify_latency_by_impulse: bool,
    /// Parameter-fuzz sessions per rate.
    pub fuzz_sessions: usize,
    /// Checks to leave out. They are reported as skipped.
    pub skip: Vec<Check>,
    /// Run only sessions whose label contains this text. Use the `case`
    /// text of a failure to rerun just that case.
    pub filter: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        let seed = std::env::var("OPENPLUGIN_TEST_SEED")
            .ok()
            .and_then(|s| {
                let s = s.trim();
                match s.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16).ok(),
                    None => s.parse().ok(),
                }
            })
            .unwrap_or(0x0BE0_5EED);
        Self {
            seed,
            sample_rates: HOST_SAMPLE_RATES.to_vec(),
            block_sizes: HOST_BLOCK_SIZES.to_vec(),
            max_output: 64.0,
            strict_hostile_output: false,
            strict_denormals: false,
            silence_db: -90.0,
            tail_slack_ms: 10.0,
            max_tail_seconds: 20.0,
            tail_sample_rates: vec![44_100.0, 48_000.0, 96_000.0],
            latency_depends_on_params: false,
            verify_latency_by_impulse: false,
            fuzz_sessions: 3,
            skip: Vec::new(),
            filter: None,
        }
    }
}

impl Options {
    /// A shorter run for debug builds and slow plugins: three rates, five block sizes.
    pub fn quick() -> Self {
        Self {
            sample_rates: vec![44_100.0, 48_000.0, 96_000.0],
            block_sizes: vec![1, 7, 64, 333, 4096],
            tail_sample_rates: vec![48_000.0],
            fuzz_sessions: 1,
            ..Self::default()
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    fn silence(&self) -> f32 {
        10f32.powf(self.silence_db / 20.0)
    }

    fn wants(&self, label: &str) -> bool {
        match &self.filter {
            Some(f) => label.contains(f.as_str()),
            None => true,
        }
    }
}

/// Run a plugin through what hosts do and report. `make` builds a fresh plugin
/// each time the suite needs one.
pub fn run<P: Plugin>(make: impl Fn() -> P, options: Options) -> Report {
    let mut suite = Suite::new(&make, &options);
    suite.run_all();
    suite.finish()
}

// ---------------------------------------------------------------------------
// Stimulus

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stimulus {
    Silence,
    Dc,
    Noise,
    Sweep,
    NonFinite,
    Denormal,
}

const ALL_STIMULI: [Stimulus; 6] =
    [Stimulus::Silence, Stimulus::Dc, Stimulus::Noise, Stimulus::Sweep, Stimulus::NonFinite, Stimulus::Denormal];

impl Stimulus {
    fn name(self) -> &'static str {
        match self {
            Stimulus::Silence => "silence",
            Stimulus::Dc => "dc",
            Stimulus::Noise => "noise",
            Stimulus::Sweep => "sweep",
            Stimulus::NonFinite => "nan_inf_bursts",
            Stimulus::Denormal => "denormals",
        }
    }
}

struct Gen {
    stim: Stimulus,
    burst_left: u32,
    burst_val: f32,
    phase: f64,
    t: u64,
}

impl Gen {
    fn new(stim: Stimulus) -> Self {
        Self { stim, burst_left: 0, burst_val: 0.0, phase: 0.0, t: 0 }
    }

    /// Fill the first `nin` channels with the stimulus and zero the rest.
    fn fill(&mut self, rng: &mut Rng, rate: f32, io: &mut [Vec<f32>], nin: usize, frames: usize) {
        for (c, ch) in io.iter_mut().enumerate() {
            if c >= nin || self.stim == Stimulus::Silence {
                ch[..frames].fill(0.0);
            }
        }
        if nin == 0 {
            return;
        }
        let nin = nin.min(io.len());
        match self.stim {
            Stimulus::Silence => {}
            Stimulus::Dc => {
                for ch in io.iter_mut().take(nin) {
                    ch[..frames].fill(0.5);
                }
            }
            Stimulus::Noise => {
                for ch in io.iter_mut().take(nin) {
                    for s in &mut ch[..frames] {
                        *s = rng.bipolar();
                    }
                }
            }
            Stimulus::Sweep => {
                let f0 = 20.0f64;
                let f1 = (f64::from(rate) * 0.45).min(20_000.0);
                let period = (f64::from(rate) * 2.0) as u64;
                for i in 0..frames {
                    let pos = (self.t % period.max(1)) as f64 / period.max(1) as f64;
                    let freq = f0 * (f1 / f0).powf(pos);
                    self.phase += std::f64::consts::TAU * freq / f64::from(rate);
                    if self.phase > std::f64::consts::TAU {
                        self.phase -= std::f64::consts::TAU;
                    }
                    let v = (0.9 * self.phase.sin()) as f32;
                    for ch in io.iter_mut().take(nin) {
                        ch[i] = v;
                    }
                    self.t += 1;
                }
            }
            Stimulus::NonFinite => {
                for i in 0..frames {
                    if self.burst_left == 0 && rng.chance(1.0 / 256.0) {
                        self.burst_left = 1 + rng.below(16) as u32;
                        self.burst_val = match rng.below(4) {
                            0 => f32::NAN,
                            1 => f32::INFINITY,
                            2 => f32::NEG_INFINITY,
                            _ => -f32::NAN,
                        };
                    }
                    for ch in io.iter_mut().take(nin) {
                        ch[i] = if self.burst_left > 0 { self.burst_val } else { 0.25 * rng.bipolar() };
                    }
                    self.burst_left = self.burst_left.saturating_sub(1);
                }
            }
            Stimulus::Denormal => {
                for ch in io.iter_mut().take(nin) {
                    for s in &mut ch[..frames] {
                        let magnitude = f32::from_bits(1 + rng.below(0x007f_ffff) as u32);
                        *s = if rng.chance(0.5) { magnitude } else { -magnitude };
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Plan {
    Fixed(usize),
    Random,
}

impl Plan {
    fn next(self, rng: &mut Rng, max_block: usize) -> usize {
        match self {
            Plan::Fixed(n) => n.min(max_block).max(1),
            Plan::Random => {
                let n = match rng.below(4) {
                    0 => 1 + rng.below(8),
                    1 => 9 + rng.below(56),
                    _ => 1 + rng.below(max_block.max(1)),
                };
                n.min(max_block).max(1)
            }
        }
    }

    fn name(self) -> String {
        match self {
            Plan::Fixed(n) => format!("fixed{n}"),
            Plan::Random => "random".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Cfg {
    stim: Stimulus,
    total: usize,
    plan: Plan,
    fuzz: bool,
    notes: bool,
    hostile_transport: bool,
}

impl Cfg {
    fn new(stim: Stimulus, total: usize, plan: Plan) -> Self {
        Self { stim, total, plan, fuzz: false, notes: false, hostile_transport: false }
    }
}

#[derive(Clone, Copy)]
struct Inspect {
    finite: Option<Check>,
    denormals: bool,
    bound: bool,
}

impl Inspect {
    const ALL: Inspect = Inspect { finite: Some(Check::FiniteOutput), denormals: true, bound: true };
}

enum Ev {
    Param { offset: usize, index: usize, value: f32 },
    NoteOn { offset: usize, channel: u8, note: u8, velocity: u8 },
    NoteOff { offset: usize, note: u8 },
    Midi { offset: usize, msg: [u8; 3] },
    AllOff,
}

fn fuzz_value(rng: &mut Rng, p: &ParamInfo) -> f32 {
    let span = (p.max - p.min).abs().max(1e-6);
    match rng.below(20) {
        0 => p.min,
        1 => p.max,
        2 => p.default,
        3 => p.min - span * (0.01 + 10.0 * rng.unit()),
        4 => p.max + span * (0.01 + 10.0 * rng.unit()),
        5 => f32::NAN,
        6 => {
            if rng.chance(0.5) {
                f32::INFINITY
            } else {
                f32::NEG_INFINITY
            }
        }
        7 => 1e30 * rng.bipolar(),
        _ => rng.range(p.min, p.max),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic payload)".to_string()
    }
}

// ---------------------------------------------------------------------------
// Session: one plugin instance in one configuration

struct Session<'s, P: Plugin> {
    plugin: P,
    info: &'static PluginInfo,
    rec: &'s mut Recorder,
    opts: &'s Options,
    alloc: bool,
    label: String,
    master_seed: u64,
    seed: u64,
    rng: Rng,
    rate: f32,
    max_block: usize,
    layout: IoConfig,
    nin: usize,
    nout: usize,
    io: Vec<Vec<f32>>,
    side: Vec<Vec<f32>>,
    scratch: Vec<Vec<f32>>,
    scratch_side: Vec<Vec<f32>>,
    dead: bool,
    frames_done: u64,
    blocks: u64,
    latency: usize,
    parity: bool,
    pos: i64,
    note_ports: openplugin_core::NotePorts,
    held: Vec<u8>,
}

impl<'s, P: Plugin> Session<'s, P> {
    #[allow(clippy::too_many_arguments)]
    fn open(
        rec: &'s mut Recorder,
        opts: &'s Options,
        alloc: bool,
        make: &dyn Fn() -> P,
        label: String,
        rate: f32,
        max_block: usize,
        layout: Option<IoConfig>,
        offline: bool,
    ) -> Option<Self> {
        if !opts.wants(&label) {
            return None;
        }
        let seed = derive_seed(opts.seed, &label);
        let plugin = match catch_unwind(AssertUnwindSafe(make)) {
            Ok(p) => p,
            Err(e) => {
                rec.fail(Check::NoPanic, &label, opts.seed, seed, format!("constructing the plugin panicked: {}", panic_message(&*e)));
                return None;
            }
        };
        let mut s = Session {
            info: plugin.info(),
            plugin,
            rec,
            opts,
            alloc,
            master_seed: opts.seed,
            seed,
            rng: Rng::new(seed),
            rate,
            max_block,
            layout: IoConfig::STEREO,
            nin: 0,
            nout: 0,
            io: Vec::new(),
            side: Vec::new(),
            scratch: Vec::new(),
            scratch_side: Vec::new(),
            dead: false,
            frames_done: 0,
            blocks: 0,
            latency: 0,
            parity: false,
            pos: 0,
            note_ports: openplugin_core::NotePorts { input: false, output: false },
            held: Vec::new(),
            label,
        };
        let layouts = s.catching("supported_layouts", |p| p.supported_layouts())?;
        if layouts.is_empty() {
            s.fail(Check::Layouts, "supported_layouts() is empty".to_string());
            return None;
        }
        let layout = layout.unwrap_or(layouts[0]);
        s.layout = layout;
        match s.catching("set_layout", |p| p.set_layout(layout))? {
            true => {}
            false => {
                s.fail(Check::Layouts, format!("set_layout refused {} which supported_layouts() lists", layout.name()));
                return None;
            }
        }
        let main = layout.main_channels().max(1);
        s.nin = layout.input_channels().min(main);
        s.nout = layout.output_channels().min(main);
        let alloc_buf = |n: usize, len: usize| vec![vec![0.0f32; len]; n];
        s.io = alloc_buf(main, max_block);
        s.scratch = alloc_buf(main, max_block);
        s.side = alloc_buf(layout.sidechain_channels(), max_block);
        s.scratch_side = alloc_buf(layout.sidechain_channels(), max_block);
        s.note_ports = s.catching("note_ports", |p| p.note_ports())?;
        s.catching("set_offline", |p| p.set_offline(offline))?;
        s.catching("prepare_layout", |p| p.prepare_layout(rate, max_block, layout))?;
        s.catching("reset", |p| p.reset())?;
        s.latency = s.catching("latency", |p| p.latency())?;
        if s.latency as f64 > f64::from(rate) * 10.0 {
            let l = s.latency;
            s.fail(Check::Latency, format!("latency of {l} samples is more than ten seconds"));
        }
        s.rec.pass(Check::Layouts);
        Some(s)
    }

    fn fail(&mut self, check: Check, message: String) {
        let full = format!("{message} [after {} frames, block {}]", self.frames_done, self.blocks);
        self.rec.fail(check, &self.label, self.master_seed, self.seed, full);
    }

    /// Call the plugin with panics caught. No allocation accounting.
    fn catching<R>(&mut self, what: &str, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        let plugin = &mut self.plugin;
        match catch_unwind(AssertUnwindSafe(|| f(plugin))) {
            Ok(v) => Some(v),
            Err(e) => {
                self.dead = true;
                let m = panic_message(&*e);
                self.fail(Check::NoPanic, format!("{what} panicked: {m}"));
                None
            }
        }
    }

    /// Call the plugin on the audio path: panics caught, allocations and
    /// frees counted.
    fn guarded<R>(&mut self, what: &str, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        let plugin = &mut self.plugin;
        let (result, counts) = alloc::measure(|| catch_unwind(AssertUnwindSafe(|| f(plugin))));
        match result {
            Ok(v) => {
                if self.alloc {
                    if counts.allocs > 0 || counts.frees > 0 {
                        self.fail(
                            Check::NoAlloc,
                            format!("{what} made {} allocations and {} frees", counts.allocs, counts.frees),
                        );
                    } else {
                        self.rec.pass(Check::NoAlloc);
                    }
                }
                self.rec.pass(Check::NoPanic);
                Some(v)
            }
            Err(e) => {
                self.dead = true;
                let m = panic_message(&*e);
                self.fail(Check::NoPanic, format!("{what} panicked: {m}"));
                None
            }
        }
    }

    fn reprepare(&mut self, offline: bool) {
        let (rate, max_block, layout) = (self.rate, self.max_block, self.layout);
        self.catching("set_offline", |p| p.set_offline(offline));
        self.catching("prepare_layout", |p| p.prepare_layout(rate, max_block, layout));
        if let Some(l) = self.catching("latency", |p| p.latency()) {
            self.latency = l;
        }
    }

    // -- parameters

    fn writable(&self, index: usize) -> bool {
        !self.info.params[index].flags.contains(ParamFlags::READ_ONLY)
    }

    fn apply_param(&mut self, index: usize, value: f32) {
        if self.dead {
            return;
        }
        let info = self.info;
        let p = &info.params[index];
        let by_index = self.rng.chance(0.5);
        let ok = self.guarded("set_param", |pl| if by_index { pl.set_param_index(index, value) } else { pl.set_param(p.id, value) });
        match ok {
            Some(true) => {}
            Some(false) => {
                self.fail(Check::ParamFuzz, format!("set_param({:?}, {value}) returned false for a known parameter", p.id));
                return;
            }
            None => return,
        }
        let Some(read) = self.catching("param", |pl| pl.param(p.id)) else { return };
        let eps = 1e-4 * (p.max - p.min).abs() + 1e-6;
        match read {
            None => self.fail(Check::ParamFuzz, format!("param({:?}) is None for a known parameter", p.id)),
            Some(v) if !v.is_finite() || v < p.min - eps || v > p.max + eps => self.fail(
                Check::ParamFuzz,
                format!("after set_param({:?}, {value}) param() is {v}, outside {}..{}", p.id, p.min, p.max),
            ),
            Some(_) => self.rec.pass(Check::ParamFuzz),
        }
    }

    // -- one block

    fn transport_step(&mut self, frames: usize, hostile: bool) {
        let t = if hostile {
            let r = &mut self.rng;
            let tempos = [0.0, -120.0, f64::NAN, f64::INFINITY, 1e300, 1e-300, 120.0];
            let nums = [0u32, 1, 4, u32::MAX];
            let dens = [0u32, 4, 8, u32::MAX];
            let poss = [i64::MIN, i64::MAX, -1, 0];
            let wild = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 1e300, 0.0];
            Transport {
                playing: r.chance(0.5),
                recording: r.chance(0.5),
                tempo: tempos[r.below(tempos.len())],
                numerator: nums[r.below(nums.len())],
                denominator: dens[r.below(dens.len())],
                sample_pos: poss[r.below(poss.len())],
                seconds: wild[r.below(wild.len())],
                quarter_notes: wild[r.below(wild.len())],
                bar_start: wild[r.below(wild.len())],
                bar_number: if r.chance(0.5) { i64::MIN } else { i64::MAX },
            }
        } else {
            if self.rng.chance(0.2) {
                self.pos = self.rng.below(1 << 30) as i64;
            }
            let seconds = self.pos as f64 / f64::from(self.rate);
            let tempo = 60.0 + 120.0 * f64::from(self.rng.unit());
            Transport {
                playing: self.rng.chance(0.8),
                recording: self.rng.chance(0.1),
                tempo,
                numerator: 4,
                denominator: 4,
                sample_pos: self.pos,
                seconds,
                quarter_notes: seconds * tempo / 60.0,
                bar_start: (seconds * tempo / 240.0).floor() * 4.0,
                bar_number: (seconds * tempo / 240.0) as i64,
            }
        };
        self.pos = self.pos.saturating_add(frames as i64);
        self.guarded("set_transport", |p| p.set_transport(&t));
    }

    fn gen_events(&mut self, frames: usize, fuzz: bool, notes: bool) -> Vec<Ev> {
        let mut events = Vec::new();
        if fuzz {
            let writable: Vec<usize> = (0..self.info.params.len()).filter(|&i| self.writable(i)).collect();
            if !writable.is_empty() {
                let k = self.rng.below(4);
                for _ in 0..k {
                    let index = writable[self.rng.below(writable.len())];
                    let value = fuzz_value(&mut self.rng, &self.info.params[index]);
                    events.push(Ev::Param { offset: self.rng.below(frames), index, value });
                }
            }
        }
        if notes && self.note_ports.input {
            let k = self.rng.below(4);
            for _ in 0..k {
                let offset = self.rng.below(frames);
                match self.rng.below(8) {
                    0..=2 => {
                        let note = self.rng.below(128) as u8;
                        self.held.push(note);
                        events.push(Ev::NoteOn {
                            offset,
                            channel: self.rng.below(16) as u8,
                            note,
                            velocity: self.rng.below(128) as u8,
                        });
                    }
                    3 | 4 => {
                        let note = if !self.held.is_empty() && self.rng.chance(0.7) {
                            self.held.swap_remove(self.rng.below(self.held.len()))
                        } else {
                            self.rng.below(128) as u8
                        };
                        events.push(Ev::NoteOff { offset, note });
                    }
                    5 | 6 => {
                        let status = [0x80u8, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0, 0xF0, 0xFE, 0x00][self.rng.below(10)]
                            | self.rng.below(16) as u8 & 0x0F;
                        events.push(Ev::Midi {
                            offset,
                            msg: [status, self.rng.below(256) as u8, self.rng.below(256) as u8],
                        });
                    }
                    _ => events.push(Ev::AllOff),
                }
            }
        }
        events
    }

    fn process_range(&mut self, a: usize, b: usize, direct: bool) {
        let n = b - a;
        let has_side = self.layout.sidechain.is_some();
        let via_buses = has_side || self.parity;
        self.parity = !self.parity;
        if direct {
            let mut io = std::mem::take(&mut self.io);
            let side = std::mem::take(&mut self.side);
            if via_buses {
                let sc: Option<&[Vec<f32>]> = if has_side { Some(&side) } else { None };
                self.guarded("process_buses", |p| p.process_buses(&mut io, sc, n));
            } else {
                self.guarded("process", |p| p.process(&mut io, n));
            }
            self.io = io;
            self.side = side;
        } else {
            let mut scratch = std::mem::take(&mut self.scratch);
            let mut scratch_side = std::mem::take(&mut self.scratch_side);
            for (dst, src) in scratch.iter_mut().zip(&self.io) {
                dst[..n].copy_from_slice(&src[a..b]);
            }
            for (dst, src) in scratch_side.iter_mut().zip(&self.side) {
                dst[..n].copy_from_slice(&src[a..b]);
            }
            if via_buses {
                let sc: Option<&[Vec<f32>]> = if has_side { Some(&scratch_side) } else { None };
                self.guarded("process_buses", |p| p.process_buses(&mut scratch, sc, n));
            } else {
                self.guarded("process", |p| p.process(&mut scratch, n));
            }
            for (dst, src) in self.io.iter_mut().zip(&scratch) {
                dst[a..b].copy_from_slice(&src[..n]);
            }
            self.scratch = scratch;
            self.scratch_side = scratch_side;
        }
    }

    /// Process one block and return the peak of the finite output samples.
    fn block(&mut self, sg: &mut Gen, frames: usize, events: &[Ev], inspect: Inspect, hostile_transport: bool) -> f32 {
        if self.dead || frames == 0 {
            return 0.0;
        }
        sg.fill(&mut self.rng, self.rate, &mut self.io, self.nin, frames);
        for ch in &mut self.side {
            for s in &mut ch[..frames] {
                *s = 0.25 * self.rng.bipolar();
            }
        }
        if hostile_transport || self.rng.chance(0.4) {
            self.transport_step(frames, hostile_transport);
        }
        let mut param_events: Vec<(usize, usize, f32)> = Vec::new();
        for ev in events {
            match *ev {
                Ev::Param { offset, index, value } => param_events.push((offset.min(frames - 1), index, value)),
                Ev::NoteOn { offset, channel, note, velocity } => {
                    let offset = offset.min(frames - 1);
                    if self.rng.chance(0.5) {
                        self.guarded("note_on_channel", |p| p.note_on_channel(offset, channel, note, velocity));
                    } else {
                        self.guarded("note_on", |p| p.note_on(offset, note, velocity));
                    }
                }
                Ev::NoteOff { offset, note } => {
                    let offset = offset.min(frames - 1);
                    self.guarded("note_off", |p| p.note_off(offset, note));
                }
                Ev::Midi { offset, msg } => {
                    let offset = offset.min(frames - 1);
                    self.guarded("midi", |p| p.midi(offset, msg));
                }
                Ev::AllOff => {
                    self.guarded("all_notes_off", |p| p.all_notes_off());
                }
            }
        }
        let had_params = !param_events.is_empty();
        if param_events.is_empty() {
            self.process_range(0, frames, true);
        } else {
            param_events.sort_by_key(|e| e.0);
            let mut start = 0;
            let mut i = 0;
            while i < param_events.len() {
                let offset = param_events[i].0;
                if offset > start {
                    self.process_range(start, offset, false);
                    start = offset;
                }
                while i < param_events.len() && param_events[i].0 == offset {
                    let (_, index, value) = param_events[i];
                    self.apply_param(index, value);
                    i += 1;
                }
            }
            if start < frames {
                self.process_range(start, frames, false);
            }
        }
        if self.dead {
            return 0.0;
        }
        if self.note_ports.output {
            self.drain_midi(frames);
        }
        self.frames_done += frames as u64;
        self.blocks += 1;
        self.check_latency(had_params);
        self.inspect(frames, inspect)
    }

    fn drain_midi(&mut self, frames: usize) {
        for _ in 0..4096 {
            let Some(ev) = self.guarded("pull_midi", |p| p.pull_midi()) else { return };
            match ev {
                None => return,
                Some((offset, _)) if offset as usize >= frames.max(1) => {
                    self.fail(Check::NoPanic, format!("pull_midi returned an event at offset {offset} in a block of {frames} frames"));
                    return;
                }
                Some(_) => {}
            }
        }
        self.fail(Check::NoPanic, "pull_midi never returned None in 4096 calls".to_string());
    }

    fn check_latency(&mut self, had_params: bool) {
        let Some(now) = self.catching("latency", |p| p.latency()) else { return };
        if now == self.latency {
            self.rec.pass(Check::Latency);
        } else if had_params && self.opts.latency_depends_on_params {
            self.latency = now;
        } else {
            let before = self.latency;
            self.latency = now;
            self.fail(Check::Latency, format!("latency() changed from {before} to {now} samples while processing, with nothing to say why"));
        }
    }

    fn inspect(&mut self, frames: usize, ins: Inspect) -> f32 {
        let mut peak = 0.0f32;
        let mut nonfinite: Option<(usize, usize, f32)> = None;
        let mut denormal: Option<(usize, usize, f32)> = None;
        for (c, channel) in self.io.iter().enumerate().take(self.nout) {
            for (i, &s) in channel[..frames].iter().enumerate() {
                if !s.is_finite() {
                    nonfinite.get_or_insert((c, i, s));
                } else {
                    if s.is_subnormal() {
                        denormal.get_or_insert((c, i, s));
                    }
                    peak = peak.max(s.abs());
                }
            }
        }
        if let Some(check) = ins.finite {
            match nonfinite {
                Some((c, i, s)) => self.fail(check, format!("output is {s} at channel {c}, sample {i} of a {frames}-frame block")),
                None => self.rec.pass(check),
            }
        }
        if ins.denormals {
            match denormal {
                Some((c, i, s)) => self.fail(Check::NoDenormals, format!("output is denormal ({s:e}) at channel {c}, sample {i} of a {frames}-frame block")),
                None => self.rec.pass(Check::NoDenormals),
            }
        }
        if ins.bound && nonfinite.is_none() {
            if peak > self.opts.max_output {
                let max = self.opts.max_output;
                self.fail(Check::BoundedOutput, format!("output peak {peak} is over the bound {max}"));
            } else {
                self.rec.pass(Check::BoundedOutput);
            }
        }
        peak
    }

    // -- scenarios on a session

    fn run(&mut self, cfg: Cfg) {
        let mut sg = Gen::new(cfg.stim);
        let opts = self.opts;
        let inspect = match cfg.stim {
            Stimulus::NonFinite => Inspect {
                finite: if opts.strict_hostile_output { Some(Check::HostileInput) } else { None },
                denormals: false,
                bound: false,
            },
            Stimulus::Denormal => Inspect { finite: Some(Check::FiniteOutput), denormals: opts.strict_denormals, bound: true },
            _ => Inspect::ALL,
        };
        let mut done = 0;
        while done < cfg.total && !self.dead {
            let n = cfg.plan.next(&mut self.rng, self.max_block).min(cfg.total - done);
            let events = self.gen_events(n, cfg.fuzz, cfg.notes);
            self.block(&mut sg, n, &events, inspect, cfg.hostile_transport);
            done += n;
        }
        if cfg.stim == Stimulus::NonFinite && !self.dead {
            self.recover();
        }
    }

    /// After a NaN and infinity burst the plugin must produce finite output again.
    fn recover(&mut self) {
        let rate = self.rate as usize;
        let tail = match self.catching("tail", |p| p.tail()) {
            Some(Tail::Finite(n)) => (n as usize).min(5 * rate),
            _ => 0,
        };
        let total = rate + self.latency.min(2 * rate) + tail;
        let window = total.saturating_sub(4096);
        let mut sg = Gen::new(Stimulus::Noise);
        let mut done = 0;
        while done < total && !self.dead {
            let n = 512.min(self.max_block).min(total - done);
            let late = done + n > window;
            let inspect = Inspect { finite: if late { Some(Check::HostileInput) } else { None }, denormals: false, bound: late };
            self.block(&mut sg, n, &[], inspect, false);
            done += n;
        }
    }

    /// Feed `frames` of `stim` in blocks of at most 512 and return the peak.
    fn feed(&mut self, sg: &mut Gen, frames: usize, inspect: Inspect) -> f32 {
        let mut peak = 0.0f32;
        let mut done = 0;
        while done < frames && !self.dead {
            let n = 512.min(self.max_block).min(frames - done);
            peak = peak.max(self.block(sg, n, &[], inspect, false));
            done += n;
        }
        peak
    }

    fn reset_scenario(&mut self) {
        let silence = self.opts.silence();
        let mut noise = Gen::new(Stimulus::Noise);
        let mut quiet = Gen::new(Stimulus::Silence);
        self.feed(&mut noise, 8192, Inspect::ALL);
        self.feed(&mut quiet, 1024, Inspect::ALL);
        if self.dead {
            return;
        }
        self.guarded("reset", |p| p.reset());
        let peak = self.feed(&mut quiet, 4096, Inspect::ALL);
        if self.dead {
            return;
        }
        if peak > silence {
            let db = 20.0 * peak.log10();
            self.fail(Check::Reset, format!("after reset in the middle of a tail, silent input still gave output up to {db:.1} dBFS"));
        } else {
            self.rec.pass(Check::Reset);
        }
    }

    fn tail_scenario(&mut self) {
        let silence = self.opts.silence();
        let Some(tail) = self.catching("tail", |p| p.tail()) else { return };
        let n = match tail {
            Tail::None => 0usize,
            Tail::Finite(n) => n as usize,
            Tail::Unknown | Tail::Infinite => {
                self.rec.note(Check::Tail, format!("skipped for a {tail:?} tail"));
                return;
            }
        };
        if n as f64 > f64::from(self.rate) * f64::from(self.opts.max_tail_seconds) {
            self.rec.note(Check::Tail, format!("tail of {n} samples is over max_tail_seconds, not run"));
            return;
        }
        let mut noise = Gen::new(Stimulus::Noise);
        let mut quiet = Gen::new(Stimulus::Silence);
        if self.note_ports.input {
            self.guarded("note_on", |p| p.note_on(0, 60, 100));
        }
        self.feed(&mut noise, (self.rate as usize / 2).max(8192), Inspect::ALL);
        if self.note_ports.input {
            self.guarded("note_off", |p| p.note_off(0, 60));
        }
        // The latency may have been refreshed by param changes; use the current one.
        let slack = (self.opts.tail_slack_ms.max(0.0) * 0.001 * self.rate) as usize;
        let quiet_after = self.latency + n + slack;
        self.feed(&mut quiet, quiet_after, Inspect::ALL);
        let peak = self.feed(&mut quiet, 4096, Inspect::ALL);
        // Keep going so a decay through denormals is seen too.
        self.feed(&mut quiet, (self.rate as usize / 2).max(4096), Inspect::ALL);
        if self.dead {
            return;
        }
        if peak > silence {
            let db = 20.0 * peak.log10();
            self.fail(
                Check::Tail,
                format!(
                    "reported tail {n} + latency {} samples (+ {slack} slack), but output was still {db:.1} dBFS after that (limit {:.1} dBFS)",
                    self.latency, self.opts.silence_db
                ),
            );
        } else {
            self.rec.pass(Check::Tail);
        }
    }

    fn impulse_scenario(&mut self) {
        let total = (self.latency + 4096).max(8192);
        let mut peak = 0.0f32;
        let mut peak_at = 0usize;
        let mut done = 0;
        while done < total && !self.dead {
            let len = 512.min(self.max_block).min(total - done);
            self.impulse_block(len, done == 0);
            for channel in self.io.iter().take(self.nout) {
                for (i, &s) in channel[..len].iter().enumerate() {
                    if s.is_finite() && s.abs() > peak {
                        peak = s.abs();
                        peak_at = done + i;
                    }
                }
            }
            done += len;
        }
        if self.dead || peak < 1e-4 {
            self.rec.note(Check::Latency, "impulse check skipped: no measurable response".to_string());
            return;
        }
        let claimed = self.latency;
        let tolerance = 2usize.max(claimed / 100);
        if peak_at.abs_diff(claimed) > tolerance {
            self.fail(Check::Latency, format!("latency() says {claimed} samples but an impulse peaks at sample {peak_at}"));
        } else {
            self.rec.pass(Check::Latency);
        }
    }

    /// A block of silence with an impulse of 1.0 at sample 0 when `impulse`.
    fn impulse_block(&mut self, frames: usize, impulse: bool) {
        for ch in &mut self.io {
            ch[..frames].fill(0.0);
        }
        if impulse {
            for ch in self.io.iter_mut().take(self.nin) {
                ch[0] = 1.0;
            }
        }
        for ch in &mut self.side {
            ch[..frames].fill(0.0);
        }
        self.process_range(0, frames, true);
        self.frames_done += frames as u64;
        self.blocks += 1;
        self.check_latency(false);
        self.inspect(frames, Inspect::ALL);
    }
}

// ---------------------------------------------------------------------------
// Suite

struct Suite<'a, P: Plugin, F: Fn() -> P> {
    make: &'a F,
    opts: &'a Options,
    rec: Recorder,
    alloc: bool,
    sessions: u64,
    rates_run: Vec<f32>,
    rates_skipped: Vec<f32>,
    plugin_name: String,
    /// (rate bits, layout name, max_block, latency)
    latencies: Vec<(u32, String, usize, usize)>,
}

impl<'a, P: Plugin, F: Fn() -> P> Suite<'a, P, F> {
    fn new(make: &'a F, opts: &'a Options) -> Self {
        Self {
            make,
            opts,
            rec: Recorder::new(),
            alloc: alloc::is_installed(),
            sessions: 0,
            rates_run: Vec::new(),
            rates_skipped: Vec::new(),
            plugin_name: String::new(),
            latencies: Vec::new(),
        }
    }

    fn open<'s>(
        &'s mut self,
        label: String,
        rate: f32,
        max_block: usize,
        layout: Option<IoConfig>,
        offline: bool,
    ) -> Option<Session<'s, P>> {
        let make = self.make;
        let made: &dyn Fn() -> P = &|| make();
        self.sessions += 1;
        Session::open(&mut self.rec, self.opts, self.alloc, made, label, rate, max_block, layout, offline)
    }

    /// Compare the latency seen at prepare across `max_block` values at the same rate and layout.
    fn note_latency(&mut self, rate: f32, layout: IoConfig, max_block: usize, latency: usize, label: &str, seed: u64) {
        let name = layout.name();
        let rate_bits = rate.to_bits();
        let other = self
            .latencies
            .iter()
            .find(|(r, l, mb, lat)| *r == rate_bits && *l == name && *mb != max_block && *lat != latency)
            .map(|(_, _, mb, lat)| (*mb, *lat));
        match other {
            Some((mb, lat)) => self.rec.fail(
                Check::Latency,
                label,
                self.opts.seed,
                derive_seed(self.opts.seed, label),
                format!("latency is {latency} with max_block {max_block} but {lat} with max_block {mb} at {rate} Hz, {name}"),
            ),
            None => self.rec.pass(Check::Latency),
        }
        let _ = seed;
        self.latencies.push((rate_bits, name, max_block, latency));
    }

    fn stim_session(&mut self, scenario: &str, rate: f32, max_block: usize, layout: Option<IoConfig>, offline: bool, cfg: Cfg) {
        let label = format!(
            "{scenario} rate={rate} max_block={max_block} layout={} offline={offline} stimulus={} plan={}",
            layout.map(|l| l.name()).unwrap_or_else(|| "default".to_string()),
            cfg.stim.name(),
            cfg.plan.name()
        );
        let Some(mut s) = self.open(label.clone(), rate, max_block, layout, offline) else { return };
        s.run(cfg);
        let (lay, lat, seed) = (s.layout, s.latency, s.seed);
        drop(s);
        self.note_latency(rate, lay, max_block, lat, &label, seed);
    }

    fn pick_rate(&self) -> Option<f32> {
        [48_000.0, 44_100.0].into_iter().find(|r| self.rates_run.contains(r)).or_else(|| self.rates_run.first().copied())
    }

    fn run_all(&mut self) {
        // Probe: name, rates.
        let probe = match catch_unwind(AssertUnwindSafe(|| {
            let p = (self.make)();
            // A group path every format would show as the top level instead.
            let groups: Vec<String> = p
                .info()
                .params
                .iter()
                .filter_map(|param| {
                    openplugin_core::check_group(param.group)
                        .err()
                        .map(|e| format!("parameter {:?} has group {:?}, which hosts are shown as the top level: {e:?}", param.id, param.group))
                })
                .collect();
            (p.info().name.to_string(), self.opts.sample_rates.iter().map(|&r| (r, p.accepts_sample_rate(r))).collect::<Vec<_>>(), groups)
        })) {
            Ok(v) => v,
            Err(e) => {
                self.rec.fail(Check::NoPanic, "construct", self.opts.seed, self.opts.seed, format!("constructing the plugin panicked: {}", panic_message(&*e)));
                return;
            }
        };
        self.plugin_name = probe.0;
        for message in probe.2 {
            self.rec.fail(Check::ParamFuzz, "param-groups", self.opts.seed, self.opts.seed, message);
        }
        for (rate, accepted) in probe.1 {
            if accepted {
                self.rates_run.push(rate);
            } else {
                self.rates_skipped.push(rate);
            }
        }
        if self.rates_run.is_empty() {
            self.rec.note(Check::NoPanic, "the plugin accepts none of the sample rates; nothing ran".to_string());
            return;
        }
        if !self.alloc {
            self.rec.force_skip(
                Check::NoAlloc,
                "the counting allocator is not installed in this binary; add `openplugin_testing::install_allocator!();` at the top level of the test file",
            );
        }
        self.rec.force_skip(
            Check::Locks,
            "not checked: a lock on the audio path cannot be detected reliably from user space without instrumenting the plugin (use the thread sanitizer or a profiler)",
        );

        let rates = self.rates_run.clone();
        let blocks = self.opts.block_sizes.clone();
        let max_block = blocks.iter().copied().max().unwrap_or(512).max(512);

        // Rates x stimuli, fixed and random block plans.
        for &rate in &rates {
            for stim in ALL_STIMULI {
                self.stim_session("rates", rate, max_block, None, false, Cfg::new(stim, 16_384, Plan::Fixed(512)));
                self.stim_session("rates", rate, max_block, None, false, Cfg::new(stim, 16_384, Plan::Random));
            }
        }
        let Some(main_rate) = self.pick_rate() else { return };

        // Block sizes, with max_block equal to the block size and a mix.
        for &bs in &blocks {
            for stim in ALL_STIMULI {
                let total = 16_384.max(4 * bs);
                self.stim_session("blocks", main_rate, bs, None, false, Cfg::new(stim, total, Plan::Fixed(bs)));
            }
        }

        // Parameter fuzz.
        for &rate in rates.iter().filter(|r| [44_100.0, 48_000.0, 96_000.0].contains(*r)) {
            for i in 0..self.opts.fuzz_sessions {
                for stim in [Stimulus::Noise, Stimulus::Sweep, Stimulus::Dc] {
                    let mut cfg = Cfg::new(stim, 16_384, Plan::Random);
                    cfg.fuzz = true;
                    self.stim_session(&format!("fuzz{i}"), rate, 4096, None, false, cfg);
                }
            }
        }
        self.param_extremes(main_rate);

        // Layouts.
        self.layouts();

        // Offline.
        for stim in [Stimulus::Noise, Stimulus::Sweep] {
            let label = format!("offline rate={main_rate} stimulus={}", stim.name());
            let Some(mut s) = self.open(label, main_rate, 1024, None, true) else { continue };
            s.run(Cfg::new(stim, 8192, Plan::Random));
            s.reprepare(false);
            s.run(Cfg::new(stim, 8192, Plan::Random));
            s.reprepare(true);
            s.run(Cfg::new(stim, 8192, Plan::Fixed(333)));
        }

        // Transport.
        let mut cfg = Cfg::new(Stimulus::Noise, 16_384, Plan::Random);
        cfg.hostile_transport = true;
        self.stim_session("transport", main_rate, 4096, None, false, cfg);

        // Notes.
        let notes_probe = self.open(format!("notes-probe rate={main_rate}"), main_rate, 64, None, false).map(|s| s.note_ports);
        if let Some(ports) = notes_probe {
            if ports.input {
                for &rate in rates.iter().filter(|r| [44_100.0, 48_000.0, 96_000.0].contains(*r)) {
                    for stim in [Stimulus::Silence, Stimulus::Noise] {
                        let mut cfg = Cfg::new(stim, 32_768, Plan::Random);
                        cfg.notes = true;
                        self.stim_session("notes", rate, 2048, None, false, cfg);
                    }
                }
            }
        }

        // Reset and tail.
        let tail_rates: Vec<f32> = self.opts.tail_sample_rates.iter().copied().filter(|r| rates.contains(r)).collect();
        for &rate in &tail_rates {
            let label = format!("reset rate={rate}");
            if let Some(mut s) = self.open(label, rate, 512, None, false) {
                s.reset_scenario();
            }
            let label = format!("tail rate={rate}");
            if let Some(mut s) = self.open(label, rate, 512, None, false) {
                s.tail_scenario();
            }
        }

        // State.
        self.state_scenarios(main_rate);

        // Latency by impulse.
        if self.opts.verify_latency_by_impulse {
            let label = format!("impulse rate={main_rate}");
            if let Some(mut s) = self.open(label, main_rate, 512, None, false) {
                s.impulse_scenario();
            }
        }
    }

    fn param_extremes(&mut self, rate: f32) {
        let label = format!("param-extremes rate={rate}");
        let Some(mut s) = self.open(label, rate, 256, None, false) else { return };
        // An unknown id is refused, not invented.
        if let Some(accepted) = s.guarded("set_param", |p| p.set_param("__no_such_parameter__", 1.0)) {
            if accepted {
                s.fail(Check::ParamFuzz, "set_param accepted an id that is not a parameter".to_string());
            } else {
                s.rec.pass(Check::ParamFuzz);
            }
        }
        let mut sg = Gen::new(Stimulus::Noise);
        let info = s.info;
        for (index, p) in info.params.iter().enumerate() {
            if !s.writable(index) {
                continue;
            }
            let span = (p.max - p.min).abs().max(1.0);
            let values = [
                p.min,
                p.max,
                p.default,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::MAX,
                f32::MIN,
                p.min - span,
                p.max + span,
                0.0,
            ];
            for value in values {
                s.apply_param(index, value);
                // Both unsplit and following blocks must stay sane.
                s.block(&mut sg, 64.min(s.max_block), &[], Inspect::ALL, false);
                if s.dead {
                    return;
                }
            }
            s.apply_param(index, p.default);
        }
    }

    fn layouts(&mut self) {
        let layouts = match catch_unwind(AssertUnwindSafe(|| (self.make)().supported_layouts())) {
            Ok(l) => l,
            Err(_) => STEREO_LAYOUTS,
        };
        if layouts.is_empty() {
            self.rec.fail(Check::Layouts, "layouts", self.opts.seed, self.opts.seed, "supported_layouts() is empty".to_string());
            return;
        }
        let rates: Vec<f32> = self.rates_run.iter().copied().filter(|r| [48_000.0, 96_000.0].contains(r)).collect();
        let rates = if rates.is_empty() { self.rates_run.iter().copied().take(1).collect() } else { rates };
        for &layout in layouts {
            for &rate in &rates {
                for stim in [Stimulus::Noise, Stimulus::Sweep, Stimulus::Silence] {
                    self.stim_session("layout", rate, 1024, Some(layout), false, Cfg::new(stim, 8192, Plan::Fixed(512)));
                    self.stim_session("layout", rate, 1024, Some(layout), false, Cfg::new(stim, 8192, Plan::Random));
                }
                let mut cfg = Cfg::new(Stimulus::Noise, 8192, Plan::Random);
                cfg.fuzz = true;
                self.stim_session("layout-fuzz", rate, 1024, Some(layout), false, cfg);
            }
        }
    }

    fn state_scenarios(&mut self, rate: f32) {
        for round in 0..4 {
            self.state_round(rate, round);
        }
    }

    fn state_round(&mut self, rate: f32, round: usize) {
        let label = format!("state rate={rate} round={round}");
        let make = self.make;
        let opts = self.opts;
        let alloc_on = self.alloc;
        self.sessions += 2;
        let rec = &mut self.rec;
        // Each session borrows the recorder, so a second one is opened only after the first is done
        // with it; the two instances are built by hand here.
        let Some(mut a) = Session::open(rec, opts, alloc_on, &|| make(), label.clone(), rate, 512, None, false) else { return };
        let info = a.info;

        // Parameters: write, read back, write the read value again.
        for (index, p) in info.params.iter().enumerate() {
            if !a.writable(index) {
                continue;
            }
            let v = if round == 0 { p.default } else { p.min + (p.max - p.min) * a.rng.unit() };
            a.apply_param(index, v);
            let Some(first) = a.catching("param", |pl| pl.param(p.id)).flatten() else { continue };
            a.apply_param(index, first);
            let Some(second) = a.catching("param", |pl| pl.param(p.id)).flatten() else { continue };
            if first.to_bits() != second.to_bits() {
                a.fail(Check::State, format!("param {:?}: writing back {first} (what param() returned) changed it to {second}", p.id));
            } else {
                a.rec.pass(Check::State);
            }
        }
        a.run(Cfg::new(Stimulus::Noise, 4096, Plan::Fixed(256)));
        if a.dead {
            return;
        }
        let mut saved = Vec::new();
        for p in info.params {
            let Some(v) = a.catching("param", |pl| pl.param(p.id)).flatten() else { return };
            saved.push(v);
        }
        let blob = encode_params(info, &saved);
        match decode_params(info, &blob) {
            Some(back) if back.iter().zip(&saved).all(|(x, y)| x.to_bits() == y.to_bits()) && back.len() == saved.len() => {
                a.rec.pass(Check::State)
            }
            other => a.fail(Check::State, format!("encode_params/decode_params changed the values: {saved:?} became {other:?}")),
        }

        let chunk1 = a.catching("save_state", |p| p.save_state());
        let chunk2 = a.catching("save_state", |p| p.save_state());
        let params_only = a.catching("state_is_parameters_only", |p| p.state_is_parameters_only()).unwrap_or(false);
        let (Some(chunk1), Some(chunk2)) = (chunk1, chunk2) else { return };
        if chunk1 != chunk2 {
            a.fail(Check::State, "two save_state calls in a row returned different bytes".to_string());
        } else {
            a.rec.pass(Check::State);
        }
        if params_only && chunk1.is_some() {
            a.fail(Check::State, "state_is_parameters_only() is true but save_state() returned a chunk".to_string());
        }
        if params_only {
            a.rec.note(Check::State, "state_is_parameters_only: only parameters are round-tripped".to_string());
        } else if chunk1.is_none() {
            a.rec.note(Check::State, "the plugin saves no chunk: only parameters are round-tripped".to_string());
        }
        // Drop `a`'s borrow of the recorder before opening the second instance by finishing with it.
        let a_label = a.label.clone();
        drop(a);

        let Some(mut b) = Session::open(&mut self.rec, opts, alloc_on, &|| make(), format!("{a_label} restore"), rate, 512, None, false) else { return };
        let restored = restore_params(&mut b.plugin, &saved);
        let has_read_only = info.params.iter().any(|p| p.flags.contains(ParamFlags::READ_ONLY));
        if !restored && !has_read_only {
            b.fail(Check::State, "restore_params refused values the plugin itself reported".to_string());
        }
        for (index, p) in info.params.iter().enumerate() {
            if !b.writable(index) {
                continue;
            }
            let Some(got) = b.catching("param", |pl| pl.param(p.id)).flatten() else { continue };
            if got.to_bits() != saved[index].to_bits() {
                b.fail(Check::State, format!("param {:?}: saved {}, restored {got}", p.id, saved[index]));
            } else {
                b.rec.pass(Check::State);
            }
        }
        if let Some(chunk) = &chunk1 {
            if !params_only {
                match b.catching("load_state", |p| p.load_state(chunk)) {
                    Some(true) => {
                        b.rec.pass(Check::State);
                        let again = b.catching("save_state", |p| p.save_state()).flatten();
                        if again.as_deref() != Some(chunk.as_slice()) {
                            b.fail(Check::State, "a chunk loaded and saved again came back different".to_string());
                        } else {
                            b.rec.pass(Check::State);
                        }
                        for (index, p) in info.params.iter().enumerate() {
                            if !b.writable(index) {
                                continue;
                            }
                            let Some(got) = b.catching("param", |pl| pl.param(p.id)).flatten() else { continue };
                            if got.to_bits() != saved[index].to_bits() {
                                b.fail(Check::State, format!("param {:?}: {} before load_state, {got} after", p.id, saved[index]));
                            }
                        }
                    }
                    Some(false) => b.fail(Check::State, "load_state refused the chunk the same plugin saved".to_string()),
                    None => {}
                }
                b.run(Cfg::new(Stimulus::Noise, 2048, Plan::Fixed(256)));
            }
        }
        // Hostile blobs: no panic, and the plugin still works afterwards.
        let half = chunk1.as_ref().map(|c| c[..c.len() / 2].to_vec()).unwrap_or_default();
        let random: Vec<u8> = (0..64).map(|_| b.rng.below(256) as u8).collect();
        for blob in [Vec::new(), half, random, vec![0xFF; 4096], vec![0u8; 3]] {
            if b.dead {
                break;
            }
            b.catching("load_state (hostile bytes)", |p| p.load_state(&blob));
            b.run(Cfg::new(Stimulus::Noise, 1024, Plan::Fixed(256)));
        }
    }

    fn finish(mut self) -> Report {
        for check in self.opts.skip.clone() {
            self.rec.hard_skip(check);
        }
        self.rec.finish(self.plugin_name, self.opts.seed, self.rates_run, self.rates_skipped, self.sessions)
    }
}
