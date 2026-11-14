//! Fixed AAX IDs, the tail report, and the editor state of a shell instance.
//! Its own test binary: the shell caches one plugin's descriptor per binary.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use openplugin_core::{AaxIds, Category, EditorState, ParamInfo, Plugin, PluginInfo, Tail, Unit};
use openplugin_ffi::shell::{self, OpShellInfo};

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("decay", "Decay", 0.1, 10.0, 2.0, Unit::Seconds),
    ParamInfo::new("freeze", "Freeze", 0.0, 1.0, 0.0, Unit::Toggle),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.verb",
    name: "Test Verb",
    vendor: "OpenGrid",
    url: "",
    email: "",
    version: "0.0.1",
    category: Category::Reverb,
    params: &PARAMS,
    is_instrument: false,
    has_editor: true,
};

/// What the engine shows its editor.
struct Shared {
    blocks: AtomicU32,
}

struct Verb {
    decay: f32,
    freeze: bool,
    rate: f32,
    shared: Arc<Shared>,
}

impl Default for Verb {
    fn default() -> Self {
        Self { decay: 2.0, freeze: false, rate: 48_000.0, shared: Arc::new(Shared { blocks: AtomicU32::new(0) }) }
    }
}

impl Plugin for Verb {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _: usize, _: usize) {
        self.rate = sample_rate;
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "decay" => self.decay = PARAMS[0].clamp(value),
            "freeze" => self.freeze = PARAMS[1].clamp(value) > 0.5,
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "decay" => Some(self.decay),
            "freeze" => Some(f32::from(u8::from(self.freeze))),
            _ => None,
        }
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
        self.shared.blocks.fetch_add(1, Ordering::Relaxed);
    }
    fn tail(&self) -> Tail {
        if self.freeze {
            Tail::Infinite
        } else {
            Tail::Finite((self.decay * self.rate) as u32)
        }
    }
    fn editor_state(&self) -> Option<EditorState> {
        Some(self.shared.clone())
    }
    fn aax_ids(&self) -> Option<AaxIds> {
        Some(AaxIds { manufacturer: *b"OpGd", product: *b"TVrb", mono: *b"TVrM", stereo: *b"TVrS" })
    }
}

#[test]
fn fixed_aax_ids_replace_the_hashed_ones() {
    let mut info = unsafe { std::mem::zeroed::<OpShellInfo>() };
    assert_eq!(shell::write_info::<Verb>(&mut info), 1);
    assert_eq!(info.manufacturer_id, u32::from_be_bytes(*b"OpGd"));
    assert_eq!(info.product_id, u32::from_be_bytes(*b"TVrb"));
    assert_eq!(info.plugin_id_mono, u32::from_be_bytes(*b"TVrM"));
    assert_eq!(info.plugin_id_stereo, u32::from_be_bytes(*b"TVrS"));
}

#[test]
fn tail_follows_the_engine_after_each_block() {
    let instance = shell::create::<Verb>(48_000.0, 64, 2);
    assert!(!instance.is_null());
    unsafe {
        assert!((shell::tail_seconds(instance) - 2.0).abs() < 1e-3);
        let mut audio = [0.0f32; 64];
        assert_eq!(shell::set_plain(instance, 0, 4.5), 1);
        assert_eq!(shell::process(instance, audio.as_ptr(), audio.as_ptr(), audio.as_mut_ptr(), audio.as_mut_ptr(), 64), 1);
        assert!((shell::tail_seconds(instance) - 4.5).abs() < 1e-3);
        assert_eq!(shell::set_plain(instance, 1, 1.0), 1);
        assert_eq!(shell::process(instance, audio.as_ptr(), audio.as_ptr(), audio.as_mut_ptr(), audio.as_mut_ptr(), 64), 1);
        assert!(shell::tail_seconds(instance).is_infinite());
        shell::destroy(instance);
    }
}

#[test]
fn editor_state_belongs_to_live_instances_only() {
    let a = shell::create::<Verb>(48_000.0, 64, 2);
    let b = shell::create::<Verb>(48_000.0, 64, 2);
    let state_a = shell::editor_state(a).expect("a shares state");
    let state_b = shell::editor_state(b).expect("b shares state");
    assert!(!Arc::ptr_eq(&state_a, &state_b), "each engine has its own");
    let shared = state_a.clone().downcast::<Shared>().expect("the plugin's type");
    let mut audio = [0.0f32; 16];
    unsafe {
        shell::process(a, audio.as_ptr(), audio.as_ptr(), audio.as_mut_ptr(), audio.as_mut_ptr(), 16);
    }
    assert_eq!(shared.blocks.load(Ordering::Relaxed), 1);
    unsafe { shell::destroy(a) };
    assert!(shell::editor_state(a).is_none(), "a destroyed instance binds nothing");
    assert!(shell::editor_state(std::ptr::null()).is_none());
    let bogus = 0x1000 as *const shell::ShellInstance;
    assert!(shell::editor_state(bogus).is_none(), "not an instance");
    assert!(shell::editor_state(b).is_some());
    unsafe { shell::destroy(b) };
}

#[test]
fn trace_file_stem_is_safe() {
    assert_eq!(shell::trace::stem("Test Verb"), "test-verb");
    assert_eq!(shell::trace::stem("Sp9ceVerb"), "sp9ceverb");
}
