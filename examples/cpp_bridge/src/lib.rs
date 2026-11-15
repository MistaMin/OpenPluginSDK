//! A Rust plugin whose DSP is existing C++, called through `cxx`.
//!
//! This is the pattern of `docs/CPP_BRIDGE.md`. The C++ (`cpp/dsp.h`) is a
//! one-pole low-pass and a smoothed gain with no knowledge of Rust; `cpp/bridge.h`
//! is the shim; this file declares the bridge and wraps it as a [`Plugin`].
//!
//! The rules the wrapper keeps:
//! - `prepare` allocates on the C++ side; a C++ exception becomes an `Err`, the
//!   plugin then passes audio through and reports the error ([`CppBridge::last_error`]).
//! - `process` makes one `noexcept` call per channel with a slice. It does not
//!   allocate, lock or throw. A Rust panic around it is caught and turns the
//!   plugin into a pass-through.
//! - State is bytes in both directions; a bad chunk is an `Err`, never a crash.
//! - Parameters cross as plain `f32` values.

use std::panic::{AssertUnwindSafe, catch_unwind};

use cxx::UniquePtr;
use openplugin::prelude::*;

#[cxx::bridge(namespace = "opbridge")]
#[allow(dead_code)]
mod ffi {
    unsafe extern "C++" {
        include!("bridge.h");

        type OnePoleGain;

        fn new_dsp() -> UniquePtr<OnePoleGain>;
        fn prepare(dsp: Pin<&mut OnePoleGain>, sample_rate: f32, max_block: usize, channels: usize) -> Result<()>;
        fn reset(dsp: Pin<&mut OnePoleGain>);
        fn set_cutoff_hz(dsp: Pin<&mut OnePoleGain>, hz: f32);
        fn set_gain_db(dsp: Pin<&mut OnePoleGain>, db: f32);
        fn cutoff_hz(dsp: &OnePoleGain) -> f32;
        fn gain_db(dsp: &OnePoleGain) -> f32;
        fn process(dsp: Pin<&mut OnePoleGain>, channel: usize, samples: &mut [f32]);
        fn save_state(dsp: &OnePoleGain) -> Result<Vec<u8>>;
        fn load_state(dsp: Pin<&mut OnePoleGain>, bytes: &[u8]) -> Result<()>;
        fn set_fault(dsp: Pin<&mut OnePoleGain>, mode: i32);
    }
}

// SAFETY: the C++ object has no thread affinity and no global state. The
// plugin moves between threads (made on one, prepared on another, run on the
// audio thread) but is only ever used from one thread at a time.
unsafe impl Send for ffi::OnePoleGain {}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("cutoff", "Cutoff", 20.0, 20_000.0, 8_000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.example.cpp-bridge",
    name: "OpenPlugin C++ Bridge",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.3.1-beta",
    category: Category::Eq,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

/// Faults a test can inject (feature `test-faults`).
#[cfg(feature = "test-faults")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    /// `new`/`delete` in the C++ process path.
    CppNew,
    /// `malloc`/`free` in the C++ process path.
    CppMalloc,
    /// `std::runtime_error` thrown in C++ `prepare`.
    PrepareStdException,
    /// `throw 42` in C++ `prepare`.
    PrepareForeignException,
    /// `throw 7` in C++ `load_state`.
    LoadForeignException,
    /// A Rust panic in `process`.
    RustPanic,
}

pub struct CppBridge {
    dsp: UniquePtr<ffi::OnePoleGain>,
    sample_rate: f32,
    channels: usize,
    /// False until `prepare` succeeds, and after a failure or a caught panic:
    /// audio then passes through unchanged.
    ready: bool,
    last_error: Option<String>,
    #[cfg(feature = "test-faults")]
    panic_in_process: bool,
}

impl Default for CppBridge {
    fn default() -> Self {
        Self {
            dsp: ffi::new_dsp(),
            sample_rate: 48_000.0,
            channels: 0,
            ready: false,
            last_error: None,
            #[cfg(feature = "test-faults")]
            panic_in_process: false,
        }
    }
}

impl CppBridge {
    /// The last error from the C++ side or a caught panic, if any.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Whether `process` runs the C++ DSP (false means pass-through).
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    #[cfg(feature = "test-faults")]
    pub fn inject_fault(&mut self, fault: Fault) {
        self.panic_in_process = fault == Fault::RustPanic;
        let mode = match fault {
            Fault::CppNew => 1,
            Fault::CppMalloc => 2,
            Fault::PrepareStdException => 3,
            Fault::PrepareForeignException => 4,
            Fault::LoadForeignException => 5,
            Fault::None | Fault::RustPanic => 0,
        };
        ffi::set_fault(self.dsp.pin_mut(), mode);
    }

    fn run_dsp(&mut self, io: &mut [Vec<f32>], frames: usize) {
        #[cfg(feature = "test-faults")]
        if self.panic_in_process {
            panic!("injected Rust panic in process");
        }
        let channels = self.channels.min(io.len());
        for (index, channel) in io.iter_mut().take(channels).enumerate() {
            let len = frames.min(channel.len());
            ffi::process(self.dsp.pin_mut(), index, &mut channel[..len]);
        }
    }
}

impl Plugin for CppBridge {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.ready = false;
        match ffi::prepare(self.dsp.pin_mut(), sample_rate, max_block, channels) {
            Ok(()) => {
                self.sample_rate = sample_rate;
                self.channels = channels;
                self.ready = true;
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.what().to_string()),
        }
    }

    fn reset(&mut self) {
        ffi::reset(self.dsp.pin_mut());
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "cutoff" => ffi::set_cutoff_hz(self.dsp.pin_mut(), PARAMS[0].clamp(value)),
            "gain" => ffi::set_gain_db(self.dsp.pin_mut(), PARAMS[1].clamp(value)),
            _ => return false,
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "cutoff" => Some(ffi::cutoff_hz(&self.dsp)),
            "gain" => Some(ffi::gain_db(&self.dsp)),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if !self.ready {
            return; // pass-through: the audio is already in `io`
        }
        // catch_unwind costs nothing unless a panic happens. The SDK's CLAP
        // entry does not catch for us, and a panic out of an `extern "C"`
        // function aborts the host.
        if catch_unwind(AssertUnwindSafe(|| self.run_dsp(io, frames))).is_err() {
            self.ready = false;
            self.last_error = Some("panic in process".into());
        }
    }

    /// Worst case: 16 time constants of the lowest cutoff, enough to fall
    /// 24 dB of gain plus 90 dB below.
    fn tail_samples(&self) -> usize {
        (16.0 * self.sample_rate / (std::f32::consts::TAU * PARAMS[0].min)).ceil() as usize
    }

    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        ffi::save_state(&self.dsp).ok()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        match ffi::load_state(self.dsp.pin_mut(), data) {
            Ok(()) => true,
            Err(e) => {
                self.last_error = Some(e.what().to_string());
                false
            }
        }
    }
}

openplugin::targets::export_clap!(CppBridge);
openplugin::targets::export_lv2!(CppBridge);

/// Rust's allocator for the test build's C++ `operator new` (`cpp/count_new.cpp`),
/// so that the conformance kit's counting allocator sees C++ allocations.
#[cfg(feature = "test-faults")]
mod counted_new {
    use std::alloc::{Layout, alloc, dealloc};
    const HEADER: usize = 16;

    /// # Safety
    /// Called from C++ `operator new`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn opb_rust_alloc(size: usize) -> *mut u8 {
        let Ok(layout) = Layout::from_size_align(size + HEADER, HEADER) else { return std::ptr::null_mut() };
        unsafe {
            let base = alloc(layout);
            if base.is_null() {
                return base;
            }
            (base as *mut usize).write(size + HEADER);
            base.add(HEADER)
        }
    }

    /// # Safety
    /// `p` came from `opb_rust_alloc` or is null.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn opb_rust_free(p: *mut u8) {
        if p.is_null() {
            return;
        }
        unsafe {
            let base = p.sub(HEADER);
            let total = (base as *const usize).read();
            dealloc(base, Layout::from_size_align_unchecked(total, HEADER));
        }
    }
}
