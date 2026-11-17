//! `SurroundAdapter` against engines run by hand: bit-exact outputs on 5.1 and
//! 7.1.4, linked gain reduction, the LFE modes, fold to stereo, state and
//! parameter linking, the conformance kit and the allocation check.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use openplugin_core::{
    Category, Expression, ExpressionSet, IoConfig, MpeZones, NoteRef, ParamInfo, Plugin,
    PluginInfo, SpeakerLayout, Tail, TrackInfo, Unit, VoiceInfo,
};
use openplugin_dsp::{
    Downmix, DownmixTarget, LfeMode, LinkedDetector, MixSettings, SurroundAdapter,
};
use openplugin_testing::{alloc, conformance, Check, Options, Status};

openplugin_testing::install_allocator!();

static PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("thresh", "Threshold", -60.0, 0.0, -20.0, Unit::Db),
    ParamInfo::new("ratio", "Ratio", 1.0, 20.0, 4.0, Unit::Ratio),
];

static INFO: PluginInfo = PluginInfo::new(
    "test.gain-comp",
    "Test Gain Comp",
    "test",
    "0",
    Category::Dynamics,
    &PARAMS,
);

const MONO: IoConfig = IoConfig { input: SpeakerLayout::Mono, output: SpeakerLayout::Mono, sidechain: None };
static STEREO_ONLY: &[IoConfig] = &[IoConfig::STEREO];
static STEREO_MONO: &[IoConfig] = &[IoConfig::STEREO, MONO];

/// A stereo-linked gain and compressor. The detector follows the larger of the
/// first two channels; the gain is the user gain minus the reduction.
#[derive(Clone)]
struct Comp {
    values: [f32; 3],
    mono: bool,
    env: f32,
    linked: f32,
    reduction: f32,
    attack: f32,
    release: f32,
}

impl Comp {
    fn new(mono: bool) -> Self {
        Self { values: [0.0, -20.0, 4.0], mono, env: 0.0, linked: 0.0, reduction: 0.0, attack: 0.0, release: 0.0 }
    }
}

impl Plugin for Comp {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.attack = (-1.0 / (0.001 * sample_rate)).exp();
        self.release = (-1.0 / (0.05 * sample_rate)).exp();
        self.reset();
    }
    fn reset(&mut self) {
        self.env = 0.0;
        self.linked = 0.0;
        self.reduction = 0.0;
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match PARAMS.iter().position(|p| p.id == id) {
            Some(i) => {
                self.values[i] = PARAMS[i].clamp(value);
                true
            }
            None => false,
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        PARAMS.iter().position(|p| p.id == id).map(|i| self.values[i])
    }
    fn supported_layouts(&self) -> &'static [IoConfig] {
        if self.mono { STEREO_MONO } else { STEREO_ONLY }
    }
    fn latency(&self) -> usize {
        7
    }
    fn tail(&self) -> Tail {
        Tail::Finite(100)
    }
    fn meters(&self) -> openplugin_core::MeterFrame {
        let mut frame = openplugin_core::MeterFrame::SILENT;
        frame.reduction_db = self.reduction;
        frame
    }
    fn state_is_parameters_only(&self) -> bool {
        false
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(self.values.iter().flat_map(|v| v.to_le_bytes()).collect())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        if data.len() != 12 {
            return false;
        }
        for (i, c) in data.chunks_exact(4).enumerate() {
            self.values[i] = PARAMS[i].clamp(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        true
    }
    fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
        let [gain, thresh, ratio] = self.values;
        for i in 0..frames {
            let mut peak = 0.0f32;
            for ch in io.iter().take(2) {
                peak = peak.max(ch[i].abs());
            }
            let c = if peak > self.env { self.attack } else { self.release };
            self.env = peak + c * (self.env - peak);
            let eff = self.env.max(self.linked);
            let red = if eff > 1e-9 { ((20.0 * eff.log10() - thresh) * (1.0 - 1.0 / ratio)).max(0.0) } else { 0.0 };
            self.reduction = red;
            let g = 10f32.powf((gain - red) / 20.0);
            for ch in io.iter_mut().take(2) {
                ch[i] *= g;
            }
        }
    }
}

impl LinkedDetector for Comp {
    fn detector_level(&self) -> f32 {
        self.env
    }
    fn set_linked_level(&mut self, level: f32) {
        self.linked = level;
    }
}

fn comp() -> Comp {
    Comp::new(false)
}
fn comp_mono() -> Comp {
    Comp::new(true)
}

const RATE: f32 = 48_000.0;
const MAX_BLOCK: usize = 256;
const BLOCKS: [usize; 6] = [100, 256, 37, 100, 1, 200];

fn surround(layout: SpeakerLayout) -> IoConfig {
    IoConfig { input: layout, output: layout, sidechain: None }
}

/// Deterministic, different on every channel, loud on some.
fn signal(channels: usize, frames: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|c| {
            let amp = if c % 3 == 0 { 0.9 } else { 0.3 };
            (0..frames).map(|i| amp * ((i as f32 + 13.0 * c as f32) * (0.011 + 0.003 * c as f32)).sin()).collect()
        })
        .collect()
}

fn adapted<P, F>(adapter: &mut SurroundAdapter<P, F>, layout: IoConfig, params: &[(&str, f32)])
where
    P: Plugin,
    F: Fn() -> P + Send + 'static,
{
    assert!(adapter.supported_layouts().contains(&layout), "{} not offered", layout.name());
    assert!(adapter.set_layout(layout));
    for (id, v) in params {
        assert!(adapter.set_param(id, *v));
    }
    adapter.prepare_layout(RATE, MAX_BLOCK, layout);
}

fn run_adapter<P, F>(adapter: &mut SurroundAdapter<P, F>, input: &[Vec<f32>]) -> Vec<Vec<f32>>
where
    P: Plugin,
    F: Fn() -> P + Send + 'static,
{
    let mut io = input.to_vec();
    let mut start = 0;
    let mut k = 0;
    let total = input[0].len();
    while start < total {
        let n = BLOCKS[k % BLOCKS.len()].min(total - start);
        k += 1;
        let mut block: Vec<Vec<f32>> = io.iter().map(|c| c[start..start + n].to_vec()).collect();
        adapter.process(&mut openplugin_core::ChannelsMut::new(&mut block), n);
        for (c, b) in io.iter_mut().zip(&block) {
            c[start..start + n].copy_from_slice(b);
        }
        start += n;
    }
    io
}

/// Run the engines by hand: one per pair, one per other non-LFE channel (and
/// the LFE when processed), the same chunking and detector exchange.
fn run_by_hand(
    layout: SpeakerLayout,
    lfe: LfeMode,
    mono_engines: bool,
    link: Option<usize>,
    params: &[(&str, f32)],
    input: &[Vec<f32>],
) -> Vec<Vec<f32>> {
    let mut engines: Vec<(Vec<usize>, Comp, bool)> = Vec::new();
    let new = |dup: bool| {
        let mut p = Comp::new(mono_engines);
        for (id, v) in params {
            p.set_param(id, *v);
        }
        p.prepare(RATE, MAX_BLOCK, if dup { 2 } else { 1 });
        p
    };
    for (l, r) in layout.pairs() {
        let mut p = Comp::new(false);
        for (id, v) in params {
            p.set_param(id, *v);
        }
        p.prepare(RATE, MAX_BLOCK, 2);
        engines.push((vec![l, r], p, true));
    }
    for c in layout.unpaired() {
        if layout.is_lfe(c) {
            if lfe == LfeMode::Process {
                engines.push((vec![c], new(!mono_engines), false));
            }
        } else {
            engines.push((vec![c], new(!mono_engines), true));
        }
    }
    let mut io = input.to_vec();
    for (c, ch) in io.iter_mut().enumerate() {
        if layout.is_lfe(c) {
            match lfe {
                LfeMode::Mute => ch.fill(0.0),
                // Delayed by the engines' latency, as the mains are.
                LfeMode::Pass => {
                    let d = Comp::new(false).latency();
                    ch.rotate_right(d);
                    ch[..d].fill(0.0);
                }
                LfeMode::Process => {}
                _ => unimplemented!(),
            }
        }
    }
    let total = input[0].len();
    let (mut start, mut k) = (0, 0);
    while start < total {
        let n = BLOCKS[k % BLOCKS.len()].min(total - start);
        k += 1;
        let step = link.map_or(MAX_BLOCK, |s| s.min(MAX_BLOCK));
        let mut at = start;
        while at < start + n {
            let m = step.min(start + n - at);
            for (chans, p, _) in &mut engines {
                let mut scratch: Vec<Vec<f32>> = match chans.len() {
                    2 => chans.iter().map(|&c| io[c][at..at + m].to_vec()).collect(),
                    _ if mono_engines => vec![io[chans[0]][at..at + m].to_vec()],
                    _ => vec![io[chans[0]][at..at + m].to_vec(); 2],
                };
                p.process(&mut openplugin_core::ChannelsMut::new(&mut scratch), m);
                for (j, &c) in chans.iter().enumerate() {
                    io[c][at..at + m].copy_from_slice(&scratch[j]);
                }
            }
            if link.is_some() {
                let top = engines.iter().filter(|e| e.2).map(|e| e.1.detector_level()).fold(0.0, f32::max);
                for e in engines.iter_mut().filter(|e| e.2) {
                    e.1.set_linked_level(top);
                }
            }
            at += m;
        }
        start += n;
    }
    io
}

fn assert_bits(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
    assert_eq!(a.len(), b.len());
    for (c, (x, y)) in a.iter().zip(b).enumerate() {
        for (i, (p, q)) in x.iter().zip(y).enumerate() {
            assert_eq!(p.to_bits(), q.to_bits(), "{what}: channel {c} sample {i}: {p} vs {q}");
        }
    }
}

const SETUP: [(&str, f32); 3] = [("gain", 3.0), ("thresh", -24.0), ("ratio", 6.0)];

#[test]
fn publishes_inner_layouts_then_the_surround_ones() {
    let a = SurroundAdapter::new(comp);
    let l = a.supported_layouts();
    assert_eq!(l[0], IoConfig::STEREO);
    for s in [
        SpeakerLayout::Surround50,
        SpeakerLayout::Surround51,
        SpeakerLayout::Surround71,
        SpeakerLayout::Surround714,
        SpeakerLayout::Surround514,
    ] {
        assert!(l.contains(&surround(s)), "{s:?}");
    }
    assert_eq!(l.len(), 6);
    let b = SurroundAdapter::new(comp)
        .surround_layouts(&[SpeakerLayout::Surround51, SpeakerLayout::Stereo, SpeakerLayout::Ambisonic(1)])
        .fold_layouts(&[SpeakerLayout::Surround714]);
    let l = b.supported_layouts();
    assert_eq!(l.len(), 3);
    assert!(l.contains(&IoConfig { input: SpeakerLayout::Surround714, output: SpeakerLayout::Stereo, sidechain: None }));
    // Same options, same pooled slice.
    assert!(std::ptr::eq(l, SurroundAdapter::new(comp).surround_layouts(&[SpeakerLayout::Surround51]).fold_layouts(&[SpeakerLayout::Surround714]).supported_layouts()));
    let mut c = SurroundAdapter::new(comp);
    assert!(!c.set_layout(surround(SpeakerLayout::Surround61)));
}

#[test]
fn stereo_is_the_inner_plugin_untouched() {
    let mut a = SurroundAdapter::new(comp);
    adapted(&mut a, IoConfig::STEREO, &SETUP);
    let input = signal(2, 3000);
    let got = run_adapter(&mut a, &input);
    let mut p = comp();
    for (id, v) in SETUP {
        p.set_param(id, v);
    }
    p.prepare(RATE, MAX_BLOCK, 2);
    let want = run_by_hand_stereo(&mut p, &input);
    assert_bits(&got, &want, "stereo");
}

fn run_by_hand_stereo(p: &mut Comp, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
    let mut io = input.to_vec();
    let (mut start, mut k) = (0, 0);
    while start < io[0].len() {
        let n = BLOCKS[k % BLOCKS.len()].min(io[0].len() - start);
        k += 1;
        let mut block: Vec<Vec<f32>> = io.iter().map(|c| c[start..start + n].to_vec()).collect();
        p.process(&mut openplugin_core::ChannelsMut::new(&mut block), n);
        for (c, b) in io.iter_mut().zip(&block) {
            c[start..start + n].copy_from_slice(b);
        }
        start += n;
    }
    io
}

#[test]
fn independent_engines_match_hand_run_pairs_bit_exact() {
    for layout in [SpeakerLayout::Surround50, SpeakerLayout::Surround51, SpeakerLayout::Surround714, SpeakerLayout::Surround514] {
        for lfe in [LfeMode::Process, LfeMode::Pass, LfeMode::Mute] {
            let n = layout.channels().unwrap() as usize;
            let input = signal(n, 4000);
            let mut a = SurroundAdapter::new(comp).lfe(lfe);
            adapted(&mut a, surround(layout), &SETUP);
            let got = run_adapter(&mut a, &input);
            let want = run_by_hand(layout, lfe, false, None, &SETUP, &input);
            assert_bits(&got, &want, &format!("{layout:?} {lfe:?}"));
            if let Some(l) = layout.index_of(openplugin_core::layout::SURROUND_LFE) {
                match lfe {
                    LfeMode::Pass => {
                        assert!(got[l][..7].iter().all(|&v| v == 0.0));
                        assert_bits(&[got[l][7..].to_vec()], &[input[l][..4000 - 7].to_vec()], "pass");
                    }
                    LfeMode::Mute => assert!(got[l].iter().all(|&v| v == 0.0)),
                    LfeMode::Process => assert_ne!(got[l], input[l]),
                    _ => unimplemented!(),
                }
            }
        }
    }
}

#[test]
fn a_plugin_with_mono_gets_mono_engines_and_agrees_with_the_duplicate_path() {
    let layout = SpeakerLayout::Surround714;
    let n = layout.channels().unwrap() as usize;
    let input = signal(n, 3000);
    let mut a = SurroundAdapter::new(comp_mono);
    adapted(&mut a, surround(layout), &SETUP);
    let got = run_adapter(&mut a, &input);
    let want = run_by_hand(layout, LfeMode::Process, true, None, &SETUP, &input);
    assert_bits(&got, &want, "mono engines");
    // This plugin is symmetric, so the stereo-duplicate path gives the same bits.
    let mut b = SurroundAdapter::new(comp);
    adapted(&mut b, surround(layout), &SETUP);
    assert_bits(&got, &run_adapter(&mut b, &input), "mono vs duplicate");
}

#[test]
fn linked_engines_match_hand_run_with_the_same_exchange() {
    for (layout, interval) in [(SpeakerLayout::Surround51, 32), (SpeakerLayout::Surround714, 5), (SpeakerLayout::Surround514, 1000)] {
        let n = layout.channels().unwrap() as usize;
        let input = signal(n, 4000);
        let mut a = SurroundAdapter::new(comp).linked().link_interval(interval);
        adapted(&mut a, surround(layout), &SETUP);
        let got = run_adapter(&mut a, &input);
        let want = run_by_hand(layout, LfeMode::Process, false, Some(interval), &SETUP, &input);
        assert_bits(&got, &want, &format!("{layout:?} linked {interval}"));
        // And it is not the unlinked result.
        let free = run_by_hand(layout, LfeMode::Process, false, None, &SETUP, &input);
        assert_ne!(got, free);
    }
}

#[test]
fn linked_gain_reduction_is_the_same_on_every_channel() {
    for layout in [SpeakerLayout::Surround51, SpeakerLayout::Surround714] {
        let n = layout.channels().unwrap() as usize;
        let lfe = layout.index_of(openplugin_core::layout::SURROUND_LFE);
        // Front pair loud, everything else quiet, all constant.
        let level = |c: usize| if c < 2 { 0.9 } else { 0.05 };
        let input: Vec<Vec<f32>> = (0..n).map(|c| vec![level(c); 48_000]).collect();
        let params = [("thresh", -20.0), ("ratio", 4.0)];

        let mut linked = SurroundAdapter::new(comp).linked();
        adapted(&mut linked, surround(layout), &params);
        let out = run_adapter(&mut linked, &input);
        let gain = |c: usize| out[c][47_999] / input[c][47_999];
        let want = gain(0);
        assert!(want < 0.6, "front pair is reduced: {want}");
        for c in 0..n {
            if Some(c) == lfe {
                continue;
            }
            assert!((gain(c) - want).abs() <= 1e-5 * want, "{layout:?} channel {c}: {} vs {want}", gain(c));
        }
        // The LFE has its own engine, outside the link: quiet, so not reduced.
        if let Some(l) = lfe {
            assert!((gain(l) - 1.0).abs() < 1e-6);
        }
        assert!(linked.meters().reduction_db > 10.0);

        // Independent engines leave the quiet channels alone.
        let mut free = SurroundAdapter::new(comp);
        adapted(&mut free, surround(layout), &params);
        let out = run_adapter(&mut free, &input);
        for c in 2..n {
            assert!((out[c][47_999] / input[c][47_999] - 1.0).abs() < 1e-6);
        }
    }
}

#[test]
fn parameters_state_and_reads_cover_every_engine() {
    let layout = SpeakerLayout::Surround51;
    let mut a = SurroundAdapter::new(comp);
    assert!(a.set_layout(surround(layout)));
    a.prepare_layout(RATE, MAX_BLOCK, surround(layout));
    // Set after the engines exist: all of them follow.
    assert!(a.set_param("gain", -6.0));
    assert!(!a.set_param("nope", 1.0));
    assert!(a.set_param_index(1, -50.0));
    assert_eq!(a.param("gain"), Some(-6.0));
    assert_eq!(a.param("thresh"), Some(-50.0));
    let g = 10f32.powf(-6.0 / 20.0);
    let mut io: Vec<Vec<f32>> = (0..6).map(|_| vec![0.001; 64]).collect();
    a.process(&mut openplugin_core::ChannelsMut::new(&mut io), 64);
    for (c, ch) in io.iter().enumerate() {
        assert!((ch[63] / 0.001 - g).abs() < 1e-5, "channel {c}");
    }
    // State is the first engine's; loading goes to all.
    let state = a.save_state().unwrap();
    let mut b = SurroundAdapter::new(comp);
    assert!(b.load_state(&state));
    b.prepare_layout(RATE, MAX_BLOCK, surround(layout));
    assert_eq!(b.param("gain"), Some(-6.0), "new engines start from the first's parameters");
    assert_eq!(b.latency(), 7);
    assert_eq!(b.tail(), Tail::Finite(100));
    assert!(!b.load_state(&[0, 1]));
    let mut io: Vec<Vec<f32>> = (0..6).map(|_| vec![0.001; 64]).collect();
    b.process(&mut openplugin_core::ChannelsMut::new(&mut io), 64);
    for ch in &io {
        assert!((ch[63] / 0.001 - g).abs() < 1e-5);
    }
    // Back to stereo: one engine again.
    b.set_layout(IoConfig::STEREO);
    b.prepare_layout(RATE, MAX_BLOCK, IoConfig::STEREO);
    let mut io: Vec<Vec<f32>> = (0..2).map(|_| vec![0.001; 64]).collect();
    b.process(&mut openplugin_core::ChannelsMut::new(&mut io), 64);
    assert!((io[1][63] / 0.001 - g).abs() < 1e-5);
}

#[test]
fn fold_to_stereo_is_the_inner_plugin_on_the_downmix() {
    for layout in [SpeakerLayout::Surround51, SpeakerLayout::Surround714] {
        let n = layout.channels().unwrap() as usize;
        let input = signal(n, 4000);
        let config = IoConfig { input: layout, output: SpeakerLayout::Stereo, sidechain: None };
        let mut a = SurroundAdapter::new(comp).fold_layouts(&[layout]);
        adapted(&mut a, config, &SETUP);
        assert_eq!(a.latency(), 7);
        let got = run_adapter(&mut a, &input);

        // By hand: downmix the whole input (a matrix has no memory), then
        // run the inner plugin over it in the same blocks.
        let dm = Downmix::for_layout(layout, DownmixTarget::Stereo, MixSettings::default()).unwrap();
        let ins: Vec<&[f32]> = input.iter().map(|c| c.as_slice()).collect();
        let mut stereo = vec![vec![0.0f32; 4000]; 2];
        {
            let (l, r) = stereo.split_at_mut(1);
            dm.process(&ins, &mut [&mut l[0], &mut r[0]]);
        }
        let mut p = comp();
        for (id, v) in SETUP {
            p.set_param(id, v);
        }
        p.prepare(RATE, MAX_BLOCK, 2);
        let want = run_by_hand_stereo(&mut p, &stereo);
        assert_bits(&got[..2], &want, &format!("{layout:?} fold"));
        for ch in &got[2..] {
            assert!(ch.iter().all(|&v| v == 0.0), "channels past the stereo output are silent");
        }
    }
}

#[test]
fn fold_settings_change_the_downmix() {
    let layout = SpeakerLayout::Surround51;
    let config = IoConfig { input: layout, output: SpeakerLayout::Stereo, sidechain: None };
    let input: Vec<Vec<f32>> = (0..6).map(|c| vec![if c == 2 { 0.01 } else { 0.0 }; 64]).collect();
    let mut a = SurroundAdapter::new(comp)
        .fold_layouts(&[layout])
        .fold_settings(MixSettings { centre_db: 0.0, ..MixSettings::default() });
    adapted(&mut a, config, &[("thresh", 0.0)]);
    let mut io = input.clone();
    a.process(&mut openplugin_core::ChannelsMut::new(&mut io), 64);
    assert!((io[0][63] - 0.01).abs() < 1e-6 && (io[1][63] - 0.01).abs() < 1e-6);
}

fn options() -> Options {
    if cfg!(debug_assertions) { Options::quick() } else { Options::default() }
}

#[test]
fn adapted_plugin_passes_the_conformance_kit() {
    let make = || {
        SurroundAdapter::new(comp)
            .linked()
            .fold_layouts(&[SpeakerLayout::Surround51, SpeakerLayout::Surround714])
    };
    let report = conformance(make, options());
    println!("{report}");
    report.assert_pass();
    assert_eq!(report.status(Check::NoAlloc), Status::Pass);
    assert_eq!(report.status(Check::Layouts), Status::Pass);

    let report = conformance(|| SurroundAdapter::new(comp_mono).lfe(LfeMode::Mute), options());
    report.assert_pass();
}

#[test]
fn process_does_not_allocate() {
    assert!(alloc::is_installed());
    for (layout, out, lfe, link) in [
        (SpeakerLayout::Surround714, SpeakerLayout::Surround714, LfeMode::Process, true),
        (SpeakerLayout::Surround51, SpeakerLayout::Surround51, LfeMode::Mute, false),
        (SpeakerLayout::Surround514, SpeakerLayout::Surround514, LfeMode::Pass, true),
        (SpeakerLayout::Surround714, SpeakerLayout::Stereo, LfeMode::Process, false),
    ] {
        let n = layout.channels().unwrap() as usize;
        let config = IoConfig { input: layout, output: out, sidechain: None };
        let base = SurroundAdapter::new(comp).lfe(lfe).fold_layouts(&[layout]);
        let mut a = if link { base.linked() } else { base };
        adapted(&mut a, config, &SETUP);
        let mut io = signal(n, 1000);
        let (_, counts) = alloc::measure(|| {
            for _ in 0..20 {
                a.process(&mut openplugin_core::ChannelsMut::new(&mut io), 1000);
                a.set_param("gain", 1.0);
                let _ = a.meters();
            }
        });
        assert_eq!(counts.allocs, 0, "{}", config.name());
        assert_eq!(counts.frees, 0, "{}", config.name());
    }
}

impl Default for Comp {
    fn default() -> Self {
        Comp::new(false)
    }
}

#[test]
fn default_adapter_names_a_type_for_the_export_macros() {
    let mut a = openplugin_dsp::StereoOnSurround::<Comp>::default();
    assert_eq!(a.supported_layouts().len(), 6);
    adapted(&mut a, surround(SpeakerLayout::Surround51), &[]);
    let mut io = signal(6, 64);
    a.process(&mut openplugin_core::ChannelsMut::new(&mut io), 64);
}

/// An impulse on every channel through an inner plugin with 264 samples of
/// look-ahead: the passed LFE comes out with the mains, not 264 samples early.
#[test]
fn passed_lfe_is_delayed_with_the_mains() {
    #[derive(Default)]
    struct LookAhead(Vec<Vec<f32>>, usize);
    impl Plugin for LookAhead {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {
            self.0 = vec![vec![0.0; 264]; 2];
            self.1 = 0;
        }
        fn supported_layouts(&self) -> &'static [IoConfig] {
            STEREO_ONLY
        }
        fn latency(&self) -> usize {
            264
        }
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            false
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, io: &mut [&mut [f32]], frames: usize) {
            let start = self.1;
            for (ch, ring) in io.iter_mut().zip(&mut self.0) {
                let mut pos = start;
                for x in &mut ch[..frames] {
                    *x = std::mem::replace(&mut ring[pos], *x);
                    pos = (pos + 1) % 264;
                }
            }
            self.1 = (start + frames) % 264;
        }
    }
    let layout = SpeakerLayout::Surround51;
    let mut a = openplugin_dsp::StereoOnSurround::<LookAhead>::default().lfe(LfeMode::Pass);
    adapted(&mut a, surround(layout), &[]);
    let mut input = vec![vec![0.0f32; 1000]; 6];
    for ch in &mut input {
        ch[10] = 1.0;
    }
    let out = run_adapter(&mut a, &input);
    for (c, ch) in out.iter().enumerate() {
        let peak = ch.iter().position(|&v| v == 1.0);
        assert_eq!(peak, Some(274), "channel {c}");
    }
}

// Every Plugin call the adapter must forward. A probe counts the calls it gets
// and answers each getter with its own engine number, so a test sees both how
// many engines a call reached and which engine answered.

const ON_REF: usize = 0;
const OFF_REF: usize = 1;
const EXPRESSION: usize = 2;
const MODULATION: usize = 3;
const TRACK: usize = 4;
const MADE: usize = 5;

#[derive(Default)]
struct Calls([AtomicUsize; 6]);

struct Probe {
    calls: Arc<Calls>,
    engine: usize,
    ended: bool,
}

impl Plugin for Probe {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn supported_layouts(&self) -> &'static [IoConfig] {
        STEREO_ONLY
    }
    fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn note_on_ref(&mut self, _: usize, _: NoteRef, _: u8) {
        self.calls.0[ON_REF].fetch_add(1, Ordering::Relaxed);
    }
    fn note_off_ref(&mut self, _: usize, _: NoteRef, _: u8) {
        self.calls.0[OFF_REF].fetch_add(1, Ordering::Relaxed);
    }
    fn note_expression(&mut self, _: usize, _: NoteRef, _: Expression, _: f32) {
        self.calls.0[EXPRESSION].fetch_add(1, Ordering::Relaxed);
    }
    fn param_modulation(&mut self, _: usize, _: usize, _: NoteRef, _: f32) {
        self.calls.0[MODULATION].fetch_add(1, Ordering::Relaxed);
    }
    fn track_info_changed(&mut self, _: &TrackInfo) {
        self.calls.0[TRACK].fetch_add(1, Ordering::Relaxed);
    }
    fn note_expressions(&self) -> ExpressionSet {
        ExpressionSet::from_bits(self.engine as u32 + 1)
    }
    fn mpe_input(&self) -> Option<MpeZones> {
        Some(MpeZones::new(self.engine as u8 + 1, 0))
    }
    fn voice_info(&self) -> Option<VoiceInfo> {
        Some(VoiceInfo::new(self.engine as u32 + 1, 16))
    }
    fn pull_note_end(&mut self) -> Option<NoteRef> {
        if self.ended {
            return None;
        }
        self.ended = true;
        Some(NoteRef::key(self.engine as u8 + 1))
    }
}

/// A probe adapter on 5.1 (four engines: two pairs, the centre, the LFE).
fn probe() -> (SurroundAdapter<Probe, impl Fn() -> Probe + Send + 'static>, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let c = calls.clone();
    let make = move || Probe { calls: c.clone(), engine: c.0[MADE].fetch_add(1, Ordering::Relaxed), ended: false };
    let mut a = SurroundAdapter::new(make);
    adapted(&mut a, surround(SpeakerLayout::Surround51), &[]);
    assert_eq!(calls.0[MADE].load(Ordering::Relaxed), 4);
    (a, calls)
}

fn reached(calls: &Calls, which: usize) -> usize {
    calls.0[which].load(Ordering::Relaxed)
}

#[test]
fn note_on_ref_reaches_every_engine() {
    let (mut a, calls) = probe();
    a.note_on_ref(0, NoteRef::new(5, 0, 0, 60), 100);
    assert_eq!(reached(&calls, ON_REF), 4);
}

#[test]
fn note_off_ref_reaches_every_engine() {
    let (mut a, calls) = probe();
    a.note_off_ref(0, NoteRef::new(5, 0, 0, 60), 64);
    assert_eq!(reached(&calls, OFF_REF), 4);
}

#[test]
fn note_expression_reaches_every_engine() {
    let (mut a, calls) = probe();
    a.note_expression(0, NoteRef::ALL, Expression::Tuning, 1.0);
    assert_eq!(reached(&calls, EXPRESSION), 4);
}

#[test]
fn param_modulation_reaches_every_engine() {
    let (mut a, calls) = probe();
    a.param_modulation(0, 0, NoteRef::ALL, 0.5);
    assert_eq!(reached(&calls, MODULATION), 4);
}

#[test]
fn track_info_changed_reaches_every_engine() {
    let (mut a, calls) = probe();
    a.track_info_changed(&TrackInfo::default());
    assert_eq!(reached(&calls, TRACK), 4);
}

#[test]
fn note_expressions_come_from_the_first_engine() {
    let (a, _) = probe();
    assert_eq!(a.note_expressions(), ExpressionSet::from_bits(1));
}

#[test]
fn mpe_input_comes_from_the_first_engine() {
    let (a, _) = probe();
    assert_eq!(a.mpe_input(), Some(MpeZones::new(1, 0)));
}

#[test]
fn voice_info_comes_from_the_first_engine() {
    let (a, _) = probe();
    assert_eq!(a.voice_info(), Some(VoiceInfo::new(1, 16)));
}

#[test]
fn pull_note_end_comes_from_the_first_engine() {
    let (mut a, _) = probe();
    assert_eq!(a.pull_note_end(), Some(NoteRef::key(1)));
    assert_eq!(a.pull_note_end(), None);
}
