//! Stereo plugins on surround buses.
//!
//! [`SurroundAdapter`] wraps a plugin written for stereo and is itself a
//! [`Plugin`]. It publishes the inner plugin's own layouts plus the surround
//! layouts the developer chooses, and on a surround layout it runs several
//! copies ("engines") of the inner plugin, one per channel pair.
//!
//! ```ignore
//! // In the plugin's export, one line:
//! SurroundAdapter::new(MyPlugin::default)
//! // or with options:
//! SurroundAdapter::new(MyCompressor::default)
//!     .surround_layouts(&[SpeakerLayout::Surround51, SpeakerLayout::Surround714])
//!     .lfe(LfeMode::Pass)
//!     .linked()
//! ```
//!
//! # Where it lives
//!
//! Here in `openplugin-dsp`, not `openplugin-core` as the roadmap first said.
//! The fold-to-stereo mode needs [`Downmix`], and `openplugin-dsp` depends on
//! `openplugin-core`, so core cannot depend back on it. The adapter only uses
//! the public `Plugin` trait and the layout helpers of core.
//!
//! # Layouts
//!
//! `supported_layouts()` lists, in this order: the inner plugin's own layouts
//! (the first is still the default), then every surround layout with matching
//! input and output (default 5.0, 5.1, 7.1, 7.1.4, 5.1.4), then every fold
//! layout (surround in, [`SpeakerLayout::Stereo`] out; none by default). If
//! the inner plugin does not list stereo in and out, only its own layouts are
//! published. Surround and fold layouts have no sidechain. A layout the inner
//! plugin lists itself is always handled by the inner plugin directly (one
//! engine, no adaptation, sidechain and all).
//!
//! The returned slice is `&'static`, as the trait needs. Each distinct list is
//! leaked once into a process-wide pool, so building many adapters with the
//! same options leaks once.
//!
//! # Engines
//!
//! On a surround layout the engines are, in this order:
//!
//! 1. one per left/right pair of `SpeakerLayout::pairs()` (front, sides,
//!    backs, front wides, height pairs, top sides), prepared with the stereo
//!    layout, fed the two channels;
//! 2. one per unpaired non-LFE channel (centre, back centre, top centre), in
//!    ascending channel order. If the inner plugin lists mono in and out it is
//!    prepared with the mono layout and fed the one channel. Otherwise it is
//!    prepared as stereo, fed the channel on both inputs, and the **left**
//!    output is taken (the right is discarded). For a plugin that is
//!    stereo-linked and symmetric the two choices sound the same;
//! 3. for [`LfeMode::Process`], one more mono engine (same rule) for the LFE.
//!    [`LfeMode::Pass`] delays the LFE by the inner plugin's latency (read
//!    in `prepare_layout`) so it stays in time with the other channels, and
//!    otherwise leaves it untouched; [`LfeMode::Mute`] writes zeros.
//!
//! Engines are created from the factory in `prepare_layout`, never in
//! `process`. A new engine takes the parameters (and, unless
//! `state_is_parameters_only`, the saved state) of the first engine.
//!
//! # Parameters, state, reads
//!
//! Every parameter is linked: `set_param` goes to every engine; `param`,
//! `meters`, `bus_meters`, `latency`, `tail`, `save_state` come from the first
//! engine (the front pair on a surround layout, so the meters show the front
//! pair only); `load_state` loads into all. `gain_reduction_db` is the
//! largest of any engine, so a meter shows the deepest reduction. All engines
//! have the same latency, so the first one's is the bus latency. MIDI and
//! transport go to every engine, and so do the per-note calls (`note_on_ref`,
//! `note_off_ref`, `note_expression`), `param_modulation` and
//! `track_info_changed`; `note_expressions`, `mpe_input`, `voice_info` and
//! `pull_note_end` come from the first engine. The adapter overrides every
//! `Plugin` method, so none falls back to the trait default.
//!
//! # Shared detector
//!
//! A dynamics plugin implements [`LinkedDetector`] and the developer calls
//! [`SurroundAdapter::linked`]. The adapter then processes in chunks of
//! [`SurroundAdapter::link_interval`] frames (default 32). After each chunk it
//! takes the largest `detector_level` of all linked engines (not the LFE
//! engine) and gives it to all of them with `set_linked_level`. The link
//! therefore lags by one chunk: a transient on one pair reaches the other
//! engines at the next chunk boundary, at most 32 frames later by default.
//! It is exact for steady and slowly moving signals. Chunks restart at the
//! start of every host block, so output depends on block boundaries (slightly,
//! and only when linked). Without `linked()` the engines are independent and
//! each block is one chunk.
//!
//! # Fold to stereo
//!
//! A fold layout is surround in, stereo out. Per block the adapter computes
//! `stereo = Downmix::for_layout(input, Stereo, settings).process(input)`
//! (BS.775 levels by default, [`SurroundAdapter::fold_settings`] changes them),
//! runs one inner engine on that stereo signal, and writes the result to the
//! first two channels; the remaining output channels are zeroed. Precisely
//! `out = inner(Downmix(in))`. The downmix adds no latency.
//!
//! # Not for everything
//!
//! A widener, a reverb or anything that needs the speaker positions should
//! have its own surround code; the adapter treats every pair as an
//! independent stereo signal.

use std::sync::Mutex;

use openplugin_core::{
    AaxIds, BusMeters, EditorState, Expression, ExpressionSet, HostNotifier, IoConfig, MeterFrame,
    MpeZones, NoteRef, NotePorts, Plugin, PluginInfo, SpeakerLayout, Tail, TrackInfo, Transport,
    VoiceInfo, MAX_AUDIO_CHANNELS,
};

use crate::downmix::{roles_of, Downmix, DownmixTarget, MixSettings};

/// A plugin that can share its detector with sibling engines.
///
/// Implement this on the inner stereo plugin of a compressor, limiter or gate
/// and call [`SurroundAdapter::linked`]. `detector_level` is a non-negative
/// number where bigger means the detector asks for more gain change; its unit
/// is the plugin's choice (envelope amplitude, reduction in dB), as long as
/// `set_linked_level` takes the same unit. Zero means "no demand".
pub trait LinkedDetector {
    /// The detector level after the frames processed so far.
    fn detector_level(&self) -> f32;
    /// From now on, act as if the detector level were at least `level`.
    /// The plugin keeps its own level if that is higher. Called after every
    /// chunk, and with 0.0 on reset and prepare to clear the link.
    fn set_linked_level(&mut self, level: f32);
}

/// What happens to the LFE channel on a surround layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LfeMode {
    /// Run a mono engine on it.
    #[default]
    Process,
    /// Leave it untouched, delayed by the inner plugin's latency.
    Pass,
    /// Write silence.
    Mute,
}

/// The surround layouts offered by default.
pub const DEFAULT_SURROUND_LAYOUTS: [SpeakerLayout; 5] = [
    SpeakerLayout::Surround50,
    SpeakerLayout::Surround51,
    SpeakerLayout::Surround71,
    SpeakerLayout::Surround714,
    SpeakerLayout::Surround514,
];

/// Default frames between detector exchanges.
pub const DEFAULT_LINK_INTERVAL: usize = 32;

const MONO: IoConfig = IoConfig {
    input: SpeakerLayout::Mono,
    output: SpeakerLayout::Mono,
    sidechain: None,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The inner plugin handles the bus itself (native layouts, fold).
    Whole,
    Pair(usize, usize),
    Mono(usize),
    /// Stereo engine fed one channel twice, left output taken.
    Dup(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Native,
    Surround,
    Fold,
}

struct Engine<P> {
    plugin: P,
    kind: Kind,
    /// Takes part in the shared detector.
    linked: bool,
    scratch: Vec<Vec<f32>>,
}

struct Link<P> {
    level: fn(&P) -> f32,
    set: fn(&mut P, f32),
}

impl<P> Clone for Link<P> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<P> Copy for Link<P> {}

static POOL: Mutex<Vec<&'static [IoConfig]>> = Mutex::new(Vec::new());

fn intern(list: Vec<IoConfig>) -> &'static [IoConfig] {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = pool.iter().find(|s| ***s == *list) {
        return found;
    }
    let leaked: &'static [IoConfig] = Box::leak(list.into_boxed_slice());
    pool.push(leaked);
    leaked
}

fn usable(layout: SpeakerLayout) -> bool {
    let n = layout.channels().unwrap_or(0) as usize;
    n > 2 && n <= MAX_AUDIO_CHANNELS && roles_of(layout).is_some()
}

/// A stereo plugin on surround buses. See the module docs.
pub struct SurroundAdapter<P: Plugin, F: Fn() -> P + Send + 'static> {
    make: F,
    engines: Vec<Engine<P>>,
    surround: Vec<SpeakerLayout>,
    fold: Vec<SpeakerLayout>,
    lfe: LfeMode,
    fold_settings: MixSettings,
    link: Option<Link<P>>,
    link_interval: usize,
    layouts: &'static [IoConfig],
    layout: IoConfig,
    mode: Mode,
    downmix: Option<Downmix>,
    fold_scratch: Vec<Vec<f32>>,
    max_block: usize,
    /// [`LfeMode::Pass`]: (channel, delay ring of `latency()` samples).
    lfe_delay: Vec<(usize, Vec<f32>)>,
    lfe_pos: usize,
}

impl<P: Plugin, F: Fn() -> P + Send + 'static> SurroundAdapter<P, F> {
    /// Adapt the plugin `make` builds, with the default surround layouts,
    /// the LFE processed, no fold layouts and independent engines.
    pub fn new(make: F) -> Self {
        let first = make();
        let mut me = Self {
            make,
            engines: vec![Engine { plugin: first, kind: Kind::Whole, linked: false, scratch: Vec::new() }],
            surround: DEFAULT_SURROUND_LAYOUTS.to_vec(),
            fold: Vec::new(),
            lfe: LfeMode::Process,
            fold_settings: MixSettings::default(),
            link: None,
            link_interval: DEFAULT_LINK_INTERVAL,
            layouts: &[],
            layout: IoConfig::STEREO,
            mode: Mode::Native,
            downmix: None,
            fold_scratch: Vec::new(),
            max_block: 0,
            lfe_delay: Vec::new(),
            lfe_pos: 0,
        };
        me.layout = me.engines[0].plugin.supported_layouts().first().copied().unwrap_or(IoConfig::STEREO);
        me.refresh_layouts();
        me
    }

    /// The surround layouts to publish, each with matching input and output.
    /// Layouts with two channels or fewer, or with no speaker map, are ignored.
    pub fn surround_layouts(mut self, layouts: &[SpeakerLayout]) -> Self {
        self.surround = layouts.iter().copied().filter(|l| usable(*l)).collect();
        self.refresh_layouts();
        self
    }

    /// Layouts to publish as surround in, stereo out (see "Fold to stereo").
    pub fn fold_layouts(mut self, layouts: &[SpeakerLayout]) -> Self {
        self.fold = layouts.iter().copied().filter(|l| usable(*l)).collect();
        self.refresh_layouts();
        self
    }

    /// What to do with the LFE on a surround layout.
    pub fn lfe(mut self, mode: LfeMode) -> Self {
        self.lfe = mode;
        self
    }

    /// Downmix levels for fold layouts.
    pub fn fold_settings(mut self, settings: MixSettings) -> Self {
        self.fold_settings = settings;
        self
    }

    /// Frames between detector exchanges when linked (at least 1).
    pub fn link_interval(mut self, frames: usize) -> Self {
        self.link_interval = frames.max(1);
        self
    }

    fn refresh_layouts(&mut self) {
        let inner = self.engines[0].plugin.supported_layouts();
        let mut list: Vec<IoConfig> = inner.to_vec();
        if inner.contains(&IoConfig::STEREO) {
            for &l in &self.surround {
                let c = IoConfig { input: l, output: l, sidechain: None };
                if !list.contains(&c) {
                    list.push(c);
                }
            }
            for &l in &self.fold {
                let c = IoConfig { input: l, output: SpeakerLayout::Stereo, sidechain: None };
                if !list.contains(&c) {
                    list.push(c);
                }
            }
        }
        self.layouts = intern(list);
    }

    fn classify(&self, layout: IoConfig) -> Option<Mode> {
        if !self.layouts.contains(&layout) {
            return None;
        }
        if self.engines[0].plugin.supported_layouts().contains(&layout) {
            Some(Mode::Native)
        } else if layout.input == layout.output {
            Some(Mode::Surround)
        } else {
            Some(Mode::Fold)
        }
    }

    fn copy_state(src: &mut P, dst: &mut P) {
        for p in src.info().params {
            if let Some(v) = src.param(p.id) {
                dst.set_param(p.id, v);
            }
        }
        if !src.state_is_parameters_only() {
            if let Some(bytes) = src.save_state() {
                dst.load_state(&bytes);
            }
        }
    }

    fn plan(&self, layout: IoConfig, mono_ok: bool) -> Vec<(Kind, bool)> {
        let mut plan = Vec::new();
        match self.mode {
            Mode::Native | Mode::Fold => plan.push((Kind::Whole, false)),
            Mode::Surround => {
                let l = layout.input;
                for (a, b) in l.pairs() {
                    plan.push((Kind::Pair(a, b), true));
                }
                let single = |c: usize| if mono_ok { Kind::Mono(c) } else { Kind::Dup(c) };
                for c in l.unpaired() {
                    if l.is_lfe(c) {
                        if self.lfe == LfeMode::Process {
                            plan.push((single(c), false));
                        }
                    } else {
                        plan.push((single(c), true));
                    }
                }
            }
        }
        plan
    }

    fn zeroed(channels: usize, frames: usize) -> Vec<Vec<f32>> {
        (0..channels).map(|_| vec![0.0; frames]).collect()
    }
}

impl<P: Plugin + LinkedDetector, F: Fn() -> P + Send + 'static> SurroundAdapter<P, F> {
    /// Share the detector between engines (see "Shared detector").
    pub fn linked(mut self) -> Self {
        self.link = Some(Link { level: P::detector_level, set: P::set_linked_level });
        self
    }
}

impl<P: Plugin, F: Fn() -> P + Send + 'static> Plugin for SurroundAdapter<P, F> {
    fn info(&self) -> &'static PluginInfo {
        self.engines[0].plugin.info()
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.mode = Mode::Native;
        self.engines.truncate(1);
        self.engines[0].kind = Kind::Whole;
        self.engines[0].plugin.prepare(sample_rate, max_block, channels);
        self.max_block = max_block;
        self.lfe_delay.clear();
    }

    fn prepare_layout(&mut self, sample_rate: f32, max_block: usize, layout: IoConfig) {
        let mode = self.classify(layout).unwrap_or(Mode::Native);
        self.mode = mode;
        self.layout = layout;
        self.max_block = max_block;
        let cap = max_block.max(1);
        let mono_ok = self.engines[0].plugin.supported_layouts().contains(&MONO);
        let plan = self.plan(layout, mono_ok);

        self.engines.truncate(plan.len().max(1));
        while self.engines.len() < plan.len() {
            let mut plugin = (self.make)();
            Self::copy_state(&mut self.engines[0].plugin, &mut plugin);
            self.engines.push(Engine { plugin, kind: Kind::Whole, linked: false, scratch: Vec::new() });
        }
        for (engine, &(kind, linked)) in self.engines.iter_mut().zip(&plan) {
            engine.kind = kind;
            engine.linked = linked;
            let (config, channels) = match kind {
                Kind::Whole if mode == Mode::Native => (layout, 0),
                Kind::Whole | Kind::Pair(..) | Kind::Dup(_) => (IoConfig::STEREO, 2),
                Kind::Mono(_) => (MONO, 1),
            };
            engine.scratch = Self::zeroed(if mode == Mode::Surround { channels } else { 0 }, cap);
            engine.plugin.set_layout(config);
            engine.plugin.prepare_layout(sample_rate, max_block, config);
            if let Some(link) = self.link {
                (link.set)(&mut engine.plugin, 0.0);
            }
        }
        if mode == Mode::Fold {
            self.downmix = Downmix::for_layout(layout.input, DownmixTarget::Stereo, self.fold_settings);
            self.fold_scratch = Self::zeroed(2, cap);
        } else {
            self.downmix = None;
        }
        self.lfe_delay.clear();
        self.lfe_pos = 0;
        let delay = self.engines[0].plugin.latency();
        if mode == Mode::Surround && self.lfe == LfeMode::Pass && delay > 0 {
            for c in layout.input.unpaired().filter(|&c| layout.input.is_lfe(c)) {
                self.lfe_delay.push((c, vec![0.0; delay]));
            }
        }
    }

    fn set_offline(&mut self, offline: bool) {
        for e in &mut self.engines {
            e.plugin.set_offline(offline);
        }
    }

    fn reset(&mut self) {
        for (_, ring) in &mut self.lfe_delay {
            ring.fill(0.0);
        }
        self.lfe_pos = 0;
        for e in &mut self.engines {
            e.plugin.reset();
            if let Some(link) = self.link {
                (link.set)(&mut e.plugin, 0.0);
            }
        }
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let mut ok = true;
        for e in &mut self.engines {
            ok &= e.plugin.set_param(id, value);
        }
        ok
    }

    fn set_param_index(&mut self, index: usize, value: f32) -> bool {
        let mut ok = true;
        for e in &mut self.engines {
            ok &= e.plugin.set_param_index(index, value);
        }
        ok
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.engines[0].plugin.param(id)
    }

    fn meters(&self) -> MeterFrame {
        self.engines[0].plugin.meters()
    }

    fn bus_meters(&self) -> BusMeters {
        self.engines[0].plugin.bus_meters()
    }

    fn gain_reduction_db(&self) -> f32 {
        self.engines.iter().map(|e| e.plugin.gain_reduction_db()).fold(0.0, f32::max)
    }

    fn latency(&self) -> usize {
        self.engines[0].plugin.latency()
    }

    fn max_latency(&self) -> Option<usize> {
        self.engines[0].plugin.max_latency()
    }

    fn tail_samples(&self) -> usize {
        self.engines[0].plugin.tail_samples()
    }

    fn tail(&self) -> Tail {
        self.engines[0].plugin.tail()
    }

    fn supported_layouts(&self) -> &'static [IoConfig] {
        self.layouts
    }

    fn set_layout(&mut self, layout: IoConfig) -> bool {
        match self.classify(layout) {
            Some(Mode::Native) => {
                let ok = self.engines[0].plugin.set_layout(layout);
                if ok {
                    self.layout = layout;
                }
                ok
            }
            Some(_) => {
                self.layout = layout;
                true
            }
            None => false,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        match self.mode {
            Mode::Native => self.engines[0].plugin.process(io, frames),
            Mode::Surround => self.process_surround(io, frames),
            Mode::Fold => self.process_fold(io, frames),
        }
    }

    fn process_buses(&mut self, io: &mut [Vec<f32>], sidechain: Option<&[Vec<f32>]>, frames: usize) {
        match self.mode {
            Mode::Native => self.engines[0].plugin.process_buses(io, sidechain, frames),
            _ => self.process(io, frames),
        }
    }

    fn note_ports(&self) -> NotePorts {
        self.engines[0].plugin.note_ports()
    }

    fn note_on_channel(&mut self, offset: usize, channel: u8, note: u8, velocity: u8) {
        for e in &mut self.engines {
            e.plugin.note_on_channel(offset, channel, note, velocity);
        }
    }

    fn pull_midi(&mut self) -> Option<(u32, [u8; 3])> {
        self.engines[0].plugin.pull_midi()
    }

    fn set_host_bypass(&mut self, bypassed: bool) {
        for e in &mut self.engines {
            e.plugin.set_host_bypass(bypassed);
        }
    }

    fn keeps_latency_when_bypassed(&self) -> bool {
        self.engines[0].plugin.keeps_latency_when_bypassed()
    }

    fn accepts_sample_rate(&self, sample_rate: f32) -> bool {
        self.engines[0].plugin.accepts_sample_rate(sample_rate)
    }

    fn set_transport(&mut self, transport: &Transport) {
        for e in &mut self.engines {
            e.plugin.set_transport(transport);
        }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        for e in &mut self.engines {
            e.plugin.note_on(offset, note, velocity);
        }
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        for e in &mut self.engines {
            e.plugin.note_off(offset, note);
        }
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        for e in &mut self.engines {
            e.plugin.midi(offset, msg);
        }
    }

    fn all_notes_off(&mut self) {
        for e in &mut self.engines {
            e.plugin.all_notes_off();
        }
    }

    fn note_on_ref(&mut self, offset: usize, note: NoteRef, velocity: u8) {
        for e in &mut self.engines {
            e.plugin.note_on_ref(offset, note, velocity);
        }
    }

    fn note_off_ref(&mut self, offset: usize, note: NoteRef, velocity: u8) {
        for e in &mut self.engines {
            e.plugin.note_off_ref(offset, note, velocity);
        }
    }

    fn note_expression(&mut self, offset: usize, note: NoteRef, expression: Expression, value: f32) {
        for e in &mut self.engines {
            e.plugin.note_expression(offset, note, expression, value);
        }
    }

    fn param_modulation(&mut self, offset: usize, index: usize, note: NoteRef, amount: f32) {
        for e in &mut self.engines {
            e.plugin.param_modulation(offset, index, note, amount);
        }
    }

    fn track_info_changed(&mut self, info: &TrackInfo) {
        for e in &mut self.engines {
            e.plugin.track_info_changed(info);
        }
    }

    fn note_expressions(&self) -> ExpressionSet {
        self.engines[0].plugin.note_expressions()
    }

    fn mpe_input(&self) -> Option<MpeZones> {
        self.engines[0].plugin.mpe_input()
    }

    fn voice_info(&self) -> Option<VoiceInfo> {
        self.engines[0].plugin.voice_info()
    }

    fn pull_note_end(&mut self) -> Option<NoteRef> {
        self.engines[0].plugin.pull_note_end()
    }

    fn preset_count(&self) -> usize {
        self.engines[0].plugin.preset_count()
    }

    fn preset_name(&self, index: usize) -> &'static str {
        self.engines[0].plugin.preset_name(index)
    }

    fn preset_values(&self, index: usize) -> Option<&'static [f32]> {
        self.engines[0].plugin.preset_values(index)
    }

    fn editor_state(&self) -> Option<EditorState> {
        self.engines[0].plugin.editor_state()
    }

    /// The first engine reports the latency and the parameters, so it keeps the notifier.
    fn set_host_notifier(&mut self, notifier: HostNotifier) {
        self.engines[0].plugin.set_host_notifier(notifier);
    }

    fn aax_ids(&self) -> Option<AaxIds> {
        self.engines[0].plugin.aax_ids()
    }

    fn lv2_uri(&self) -> Option<&'static str> {
        self.engines[0].plugin.lv2_uri()
    }

    fn clap_id(&self) -> Option<&'static str> {
        self.engines[0].plugin.clap_id()
    }

    fn vst3_ids(&self) -> Option<openplugin_core::Vst3Ids> {
        self.engines[0].plugin.vst3_ids()
    }

    fn au_ids(&self) -> Option<openplugin_core::AuIds> {
        self.engines[0].plugin.au_ids()
    }

    fn state_is_parameters_only(&self) -> bool {
        self.engines[0].plugin.state_is_parameters_only()
    }

    fn flush_denormals(&self) -> bool {
        self.engines[0].plugin.flush_denormals()
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.engines[0].plugin.save_state()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let mut ok = true;
        for e in &mut self.engines {
            ok &= e.plugin.load_state(data);
        }
        ok
    }
}

impl<P: Plugin, F: Fn() -> P + Send + 'static> SurroundAdapter<P, F> {
    fn process_surround(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let frames = frames.min(io.iter().map(Vec::len).min().unwrap_or(0));
        let cap = self.max_block.max(1);
        let step = if self.link.is_some() { self.link_interval.min(cap) } else { cap };
        if self.lfe == LfeMode::Mute {
            for (c, ch) in io.iter_mut().enumerate() {
                if self.layout.input.is_lfe(c) {
                    ch[..frames].fill(0.0);
                }
            }
        }
        if let Some(len) = self.lfe_delay.first().map(|(_, ring)| ring.len()) {
            for (c, ring) in &mut self.lfe_delay {
                let Some(ch) = io.get_mut(*c) else { continue };
                let mut pos = self.lfe_pos;
                for x in &mut ch[..frames] {
                    *x = std::mem::replace(&mut ring[pos], *x);
                    pos = if pos + 1 == len { 0 } else { pos + 1 };
                }
            }
            self.lfe_pos = (self.lfe_pos + frames) % len;
        }
        let mut start = 0;
        while start < frames {
            let n = step.min(frames - start);
            let end = start + n;
            for e in &mut self.engines {
                match e.kind {
                    Kind::Pair(l, r) if l < io.len() && r < io.len() => {
                        e.scratch[0][..n].copy_from_slice(&io[l][start..end]);
                        e.scratch[1][..n].copy_from_slice(&io[r][start..end]);
                        e.plugin.process(&mut e.scratch, n);
                        io[l][start..end].copy_from_slice(&e.scratch[0][..n]);
                        io[r][start..end].copy_from_slice(&e.scratch[1][..n]);
                    }
                    Kind::Mono(c) if c < io.len() => {
                        e.scratch[0][..n].copy_from_slice(&io[c][start..end]);
                        e.plugin.process(&mut e.scratch, n);
                        io[c][start..end].copy_from_slice(&e.scratch[0][..n]);
                    }
                    Kind::Dup(c) if c < io.len() => {
                        e.scratch[0][..n].copy_from_slice(&io[c][start..end]);
                        e.scratch[1][..n].copy_from_slice(&io[c][start..end]);
                        e.plugin.process(&mut e.scratch, n);
                        io[c][start..end].copy_from_slice(&e.scratch[0][..n]);
                    }
                    _ => {}
                }
            }
            if let Some(link) = self.link {
                let mut top = 0.0f32;
                for e in self.engines.iter().filter(|e| e.linked) {
                    top = top.max((link.level)(&e.plugin));
                }
                for e in self.engines.iter_mut().filter(|e| e.linked) {
                    (link.set)(&mut e.plugin, top);
                }
            }
            start = end;
        }
    }

    fn process_fold(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let Some(downmix) = &self.downmix else { return };
        let nin = downmix.input_channels();
        let frames = frames.min(io.iter().map(Vec::len).min().unwrap_or(0));
        if io.len() < nin.max(2) {
            for ch in io.iter_mut() {
                ch[..frames].fill(0.0);
            }
            return;
        }
        let cap = self.max_block.max(1);
        let engine = &mut self.engines[0];
        let mut start = 0;
        while start < frames {
            let n = cap.min(frames - start);
            let end = start + n;
            {
                let mut ins: [&[f32]; MAX_AUDIO_CHANNELS] = [&[]; MAX_AUDIO_CHANNELS];
                for (slot, ch) in ins.iter_mut().zip(io.iter()).take(nin) {
                    *slot = &ch[start..end];
                }
                let (a, b) = self.fold_scratch.split_at_mut(1);
                let mut outs: [&mut [f32]; 2] = [&mut a[0][..n], &mut b[0][..n]];
                downmix.process(&ins[..nin], &mut outs);
            }
            engine.plugin.process(&mut self.fold_scratch, n);
            for ch in io.iter_mut().skip(2) {
                ch[start..end].fill(0.0);
            }
            io[0][start..end].copy_from_slice(&self.fold_scratch[0][..n]);
            io[1][start..end].copy_from_slice(&self.fold_scratch[1][..n]);
            start = end;
        }
    }
}

/// The adapter for a plugin that implements [`Default`], with a plain function
/// pointer as the factory so the type can be named. Export macros need a
/// `Default` type:
///
/// ```ignore
/// export_clap!(openplugin_dsp::StereoOnSurround<MyPlugin>);
/// ```
///
/// `Default` gives the default options (see [`SurroundAdapter::new`]). To use
/// the options, wrap the built adapter in a small type of your own that
/// implements `Default`.
pub type StereoOnSurround<P> = SurroundAdapter<P, fn() -> P>;

impl<P: Plugin + Default> Default for SurroundAdapter<P, fn() -> P> {
    fn default() -> Self {
        Self::new(P::default as fn() -> P)
    }
}
