//! A test plugin whose latency and parameter names change at run time, for the
//! "telling the host about changes" tests (CLAP, LV2 and the shell ABI). Not a
//! product. The ffi tests include this file with `#[path]`.
//!
//! - `lookahead` (0..=256, whole samples) is the latency. The plugin delays its
//!   input by exactly that much, so the delay the host compensates is real.
//!   When a new value reaches `set_param` (inside `process`, on the audio
//!   thread) the plugin calls [`HostNotifier::latency_changed`].
//! - `names` (a toggle) swaps the parameter table [`Plugin::info`] returns for
//!   one with other names and calls [`HostNotifier::param_info_changed`].
//! - [`Shift::after_blocks`] makes the latency jump to [`AFTER_LATENCY`] by
//!   itself once that many blocks have run, without a parameter and without a
//!   notifier call, so the adapters' own comparison after every block is seen.
//!
//! Nothing allocates after `prepare`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin_core::{Category, HostNotifier, NotePorts, ParamInfo, Plugin, PluginInfo, Unit};

/// Longest latency, in samples.
pub const MAX_LOOKAHEAD: usize = 256;
/// The latency [`Shift::after_blocks`] switches to.
pub const AFTER_LATENCY: usize = 100;

pub static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("lookahead", "Lookahead", 0.0, MAX_LOOKAHEAD as f32, 0.0, Unit::None).with_steps(MAX_LOOKAHEAD as u32 + 1),
    ParamInfo::new("names", "Names", 0.0, 1.0, 0.0, Unit::Toggle),
];

/// The same parameters (ids, ranges, order) under other names.
pub static RENAMED: [ParamInfo; 2] = [
    ParamInfo::new("lookahead", "Look Ahead (renamed)", 0.0, MAX_LOOKAHEAD as f32, 0.0, Unit::None).with_steps(MAX_LOOKAHEAD as u32 + 1),
    ParamInfo::new("names", "Names (renamed)", 0.0, 1.0, 0.0, Unit::Toggle),
];

pub static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.shift",
    name: "Latency Shift",
    vendor: "Test",
    url: "https://example.com",
    email: "a@b.c",
    version: "0.1.0",
    category: Category::Delay,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

pub static INFO_RENAMED: PluginInfo = PluginInfo { params: &RENAMED, ..INFO };

/// What the test reads back: how often the plugin called its notifier.
#[derive(Default)]
pub struct Calls {
    pub latency: AtomicU32,
    pub param_info: AtomicU32,
}

pub struct Shift {
    lookahead: usize,
    renamed: bool,
    notifier: Option<HostNotifier>,
    calls: Arc<Calls>,
    after: Option<u32>,
    blocks: u32,
    jumped: bool,
    lines: Vec<Vec<f32>>,
    pos: usize,
}

impl Default for Shift {
    fn default() -> Self {
        Self::new(Arc::new(Calls::default()))
    }
}

impl Shift {
    pub fn new(calls: Arc<Calls>) -> Self {
        Self { lookahead: 0, renamed: false, notifier: None, calls, after: None, blocks: 0, jumped: false, lines: Vec::new(), pos: 0 }
    }

    /// Jump to [`AFTER_LATENCY`] once `blocks` blocks have run, silently.
    pub fn after_blocks(mut self, blocks: u32) -> Self {
        self.after = Some(blocks);
        self
    }

    pub fn has_notifier(&self) -> bool {
        self.notifier.is_some()
    }

    fn current(&self) -> usize {
        if self.jumped { AFTER_LATENCY } else { self.lookahead }
    }
}

impl Plugin for Shift {
    fn info(&self) -> &'static PluginInfo {
        if self.renamed { &INFO_RENAMED } else { &INFO }
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, channels: usize) {
        self.lines = (0..channels.max(2)).map(|_| vec![0.0; MAX_LOOKAHEAD + 1]).collect();
        self.pos = 0;
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.pos = 0;
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "lookahead" => {
                let next = PARAMS[0].clamp(value).round().clamp(0.0, MAX_LOOKAHEAD as f32) as usize;
                if next != self.lookahead {
                    self.lookahead = next;
                    self.calls.latency.fetch_add(1, Ordering::SeqCst);
                    if let Some(notifier) = &self.notifier {
                        notifier.latency_changed();
                    }
                }
                true
            }
            "names" => {
                let next = value >= 0.5;
                if next != self.renamed {
                    self.renamed = next;
                    self.calls.param_info.fetch_add(1, Ordering::SeqCst);
                    if let Some(notifier) = &self.notifier {
                        notifier.param_info_changed();
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "lookahead" => Some(self.lookahead as f32),
            "names" => Some(if self.renamed { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let delay = self.current();
        let size = MAX_LOOKAHEAD + 1;
        let start = self.pos;
        for (channel, line) in io.iter_mut().zip(self.lines.iter_mut()) {
            let mut pos = start;
            for sample in channel.iter_mut().take(frames) {
                line[pos] = *sample;
                *sample = line[(pos + size - delay) % size];
                pos = (pos + 1) % size;
            }
        }
        if !self.lines.is_empty() {
            self.pos = (start + frames) % size;
        }
        self.blocks = self.blocks.saturating_add(1);
        if let Some(after) = self.after {
            if self.blocks >= after {
                self.jumped = true;
            }
        }
    }

    fn latency(&self) -> usize {
        self.current()
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    fn set_host_notifier(&mut self, notifier: HostNotifier) {
        self.notifier = Some(notifier);
    }
}
