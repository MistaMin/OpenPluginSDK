//! Flush-to-zero for the current thread: [`DenormalGuard`].
//!
//! Every format entry (CLAP `process`, `params.flush` and `reset`, LV2 `run`,
//! and the shell ABI's audio calls that the VST3, Audio Unit and AAX shells
//! make) holds one of these for the duration of the call into the plugin, so
//! a plugin runs with denormals flushed whatever mode the host's audio thread
//! is in, and the host gets its own mode back afterwards, also when the plugin
//! panics. A plugin that really needs denormals opts out with
//! [`crate::Plugin::flush_denormals`].
//!
//! The control register is per CPU architecture, not per operating system:
//! one `#[cfg(target_arch)]` item per architecture below, each with the same
//! three functions (`read`, `write`, `flush_bits`), and a no-op for the rest.
#![allow(unsafe_code)]

/// Flush-to-zero for the lifetime of the value, on the current thread.
///
/// Denormal (subnormal) floats make some CPUs take a slow path, tens to
/// hundreds of cycles per operation, which shows up as sudden CPU spikes in
/// recursive filters and reverb tails. The format entries already hold one
/// around every audio-thread call into a plugin (see the [module
/// documentation](self)); a plugin can still make its own at the top of
/// `process`, which is then a harmless no-op change:
///
/// ```
/// use openplugin_core::DenormalGuard;
/// let _guard = DenormalGuard::new();
/// // ... process ...
/// ```
///
/// On drop the previous mode is restored, so the host's own setting is not
/// disturbed. A panic that unwinds through the guard drops it too. The guard
/// is `!Send` because the register belongs to one thread. Making and dropping
/// one is two control-register reads and two writes: no allocation, no lock,
/// no system call.
///
/// # What is used
///
/// - `aarch64` (Apple Silicon, Linux and Windows on Arm): reads and writes
///   `FPCR` with `mrs` / `msr` inline assembly and sets bit 24 (`FZ`). On
///   AArch64 `FZ` flushes denormal inputs and results together (Arm ARM,
///   FPCR; there is no separate DAZ bit).
/// - `x86_64`: `stmxcsr` / `ldmxcsr` inline assembly on `MXCSR`, setting bit
///   15 (`FTZ`, flush results) and bit 6 (`DAZ`, treat inputs as zero)
///   (Intel SDM vol. 1, section 10.2.3). Every x86_64 CPU has DAZ. An x86_64
///   build running under Rosetta 2 on Apple Silicon gets the same bits: the
///   translator keeps `MXCSR` and honours FTZ and DAZ (proven by the tests
///   below run with `--target x86_64-apple-darwin`). Inline assembly is used
///   because the `_mm_getcsr` / `_mm_setcsr` intrinsics are deprecated since
///   Rust 1.75.
/// - `x86` (32-bit) built with SSE: the same `MXCSR` bits. `DAZ` is set only
///   when the CPU reports it in `MXCSR_MASK` (read once with `fxsave`, Intel
///   SDM vol. 1, section 11.6.6), since the first SSE CPUs fault on it. A
///   32-bit build without SSE does its float arithmetic on the x87 unit,
///   which has no flush mode: nothing happens there.
/// - Elsewhere: nothing happens and [`DenormalGuard::active`] is `false`.
///
/// # Why the `unsafe` is sound
///
/// `asm!` is `unsafe` because it can do anything. These blocks only read or
/// write the thread's floating-point control register: they touch no memory
/// other than a local slot (x86 `stmxcsr`/`ldmxcsr`/`fxsave`) and preserve
/// flags. A changed denormal mode cannot cause memory unsafety; it changes
/// only the numeric results of float operations on this thread, and `Drop`
/// restores it. The value written is the old value with defined bits set, so
/// no reserved bit changes (an invalid MXCSR value would fault, and cannot
/// occur).
pub struct DenormalGuard {
    previous: u64,
    active: bool,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Default for DenormalGuard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "aarch64")]
mod arch {
    pub const AVAILABLE: bool = true;
    /// FPCR.FZ.
    const FZ: u64 = 1 << 24;
    /// FPCR.RMode, for the tests' distinct host values.
    pub const ROUNDING: u64 = 0b11 << 22;
    /// FPCR holds no status flags.
    pub const STATUS: u64 = 0;

    pub fn read() -> u64 {
        let value: u64;
        // SAFETY: reads FPCR, no memory access.
        unsafe {
            core::arch::asm!("mrs {}, fpcr", out(reg) value, options(nomem, nostack, preserves_flags));
        }
        value
    }

    pub fn write(value: u64) {
        // SAFETY: writes FPCR; only float behaviour of this thread changes.
        unsafe {
            core::arch::asm!("msr fpcr, {}", in(reg) value, options(nomem, nostack, preserves_flags));
        }
    }

    pub fn flush_bits() -> u64 {
        FZ
    }

    /// Flush bits a host may have on that are only part of the guard's set.
    pub fn partial_flush_bits() -> Option<u64> {
        None
    }
}

/// `MXCSR` access, shared by the two x86 architectures that have SSE.
#[cfg(any(target_arch = "x86_64", all(target_arch = "x86", target_feature = "sse")))]
mod mxcsr {
    pub const FTZ: u64 = 1 << 15;
    pub const DAZ: u64 = 1 << 6;
    /// MXCSR.RC, for the tests' distinct host values.
    pub const ROUNDING: u64 = 0b11 << 13;
    /// The six sticky exception flags: any float operation may set them.
    pub const STATUS: u64 = 0x3f;

    pub fn read() -> u64 {
        let mut slot: u32 = 0;
        // SAFETY: stores MXCSR into a local 4-byte slot.
        unsafe {
            core::arch::asm!("stmxcsr [{}]", in(reg) &mut slot, options(nostack, preserves_flags));
        }
        u64::from(slot)
    }

    pub fn write(value: u64) {
        let slot = value as u32;
        // SAFETY: loads MXCSR from a local slot; the value is one read from
        // MXCSR with defined bits changed, so no reserved bit is set.
        unsafe {
            core::arch::asm!("ldmxcsr [{}]", in(reg) &slot, options(nostack, preserves_flags, readonly));
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    pub use super::mxcsr::{read, write, ROUNDING, STATUS};
    use super::mxcsr::{DAZ, FTZ};
    pub const AVAILABLE: bool = true;

    pub fn flush_bits() -> u64 {
        FTZ | DAZ
    }

    pub fn partial_flush_bits() -> Option<u64> {
        Some(FTZ)
    }
}

#[cfg(all(target_arch = "x86", target_feature = "sse"))]
mod arch {
    pub use super::mxcsr::{read, write, ROUNDING, STATUS};
    use super::mxcsr::{DAZ, FTZ};
    use std::sync::atomic::{AtomicU8, Ordering};
    pub const AVAILABLE: bool = true;

    /// Whether the CPU accepts MXCSR.DAZ (Intel SDM vol. 1, 11.6.6): read
    /// `MXCSR_MASK` from an `fxsave` image once; 0 there means the default
    /// mask 0xFFBF, which has no DAZ.
    fn daz_supported() -> bool {
        static KNOWN: AtomicU8 = AtomicU8::new(0); // 0 unknown, 1 no, 2 yes
        match KNOWN.load(Ordering::Relaxed) {
            1 => false,
            2 => true,
            _ => {
                #[repr(C, align(16))]
                struct Area([u8; 512]);
                let mut area = Area([0; 512]);
                // SAFETY: fxsave writes 512 bytes to a 16-byte aligned local area.
                unsafe {
                    core::arch::asm!("fxsave [{}]", in(reg) &mut area, options(nostack, preserves_flags));
                }
                let mask = u32::from_le_bytes([area.0[28], area.0[29], area.0[30], area.0[31]]);
                let mask = if mask == 0 { 0xffbf } else { mask };
                let yes = u64::from(mask) & DAZ != 0;
                KNOWN.store(if yes { 2 } else { 1 }, Ordering::Relaxed);
                yes
            }
        }
    }

    pub fn flush_bits() -> u64 {
        if daz_supported() {
            FTZ | DAZ
        } else {
            FTZ
        }
    }

    pub fn partial_flush_bits() -> Option<u64> {
        daz_supported().then_some(FTZ)
    }
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64", all(target_arch = "x86", target_feature = "sse"))))]
mod arch {
    pub const AVAILABLE: bool = false;
    pub const ROUNDING: u64 = 0;
    pub const STATUS: u64 = 0;
    pub fn read() -> u64 {
        0
    }
    pub fn write(_: u64) {}
    pub fn flush_bits() -> u64 {
        0
    }
    pub fn partial_flush_bits() -> Option<u64> {
        None
    }
}

impl DenormalGuard {
    /// Whether this target has a flush mode the guard sets.
    pub const SUPPORTED: bool = arch::AVAILABLE;

    /// Turn flushing on, remembering the old mode.
    #[inline]
    pub fn new() -> Self {
        Self::new_if(true)
    }

    /// [`DenormalGuard::new`] when `flush` is true; otherwise a guard that
    /// leaves the mode alone (and restores nothing). The format entries pass
    /// [`crate::Plugin::flush_denormals`] here.
    #[inline]
    pub fn new_if(flush: bool) -> Self {
        let active = flush && arch::AVAILABLE;
        let previous = if active {
            let previous = arch::read();
            arch::write(previous | arch::flush_bits());
            previous
        } else {
            0
        };
        Self { previous, active, _not_send: std::marker::PhantomData }
    }

    /// Whether this guard changed the mode (and will restore it).
    pub fn active(&self) -> bool {
        self.active
    }

    /// The raw control word seen right now (FPCR or MXCSR), for tests. 0 on
    /// targets without a flush mode.
    pub fn raw_control() -> u64 {
        arch::read()
    }

    /// The bits the guard sets in the control word (FZ, or FTZ and DAZ).
    pub fn flush_bits() -> u64 {
        arch::flush_bits()
    }

    /// True when the current thread runs with every bit the guard sets: what
    /// a plugin sees inside a format entry unless it opted out. Always false
    /// where [`DenormalGuard::SUPPORTED`] is false.
    pub fn flushing() -> bool {
        arch::AVAILABLE && arch::read() & arch::flush_bits() == arch::flush_bits()
    }
}

impl Drop for DenormalGuard {
    #[inline]
    fn drop(&mut self) {
        if self.active {
            arch::write(self.previous);
        }
    }
}

/// Tools for tests that play a host's audio thread: set a distinct
/// floating-point mode before calling a format entry, and compare the mode
/// afterwards. Not part of the stable API.
#[doc(hidden)]
pub mod host_mode {
    use super::arch;

    /// The control word without the sticky status flags (which any float
    /// operation may set).
    pub fn control() -> u64 {
        arch::read() & !arch::STATUS
    }

    /// Set the thread's control word, returning the old one.
    ///
    /// # Safety
    /// `word` is a value [`control`] or [`hosts`] returned on this CPU: an
    /// x86 CPU faults on a reserved MXCSR bit.
    pub unsafe fn set(word: u64) -> u64 {
        let old = arch::read();
        arch::write(word);
        old
    }

    /// Distinct host modes for a test, named: flushing off, fully on, only
    /// partly on (x86 FTZ without DAZ), each also with a non-default rounding
    /// mode, so a restore that writes "off" instead of the host's own word is
    /// caught. Empty where the target has no flush mode. Built from the
    /// current word, so reserved bits stay as the CPU has them.
    pub fn hosts() -> Vec<(&'static str, u64)> {
        if !arch::AVAILABLE {
            return Vec::new();
        }
        let base = control() & !arch::flush_bits() & !arch::ROUNDING;
        let mut list = vec![
            ("flush off", base),
            ("flush off, round toward zero", base | arch::ROUNDING),
            ("flush already on", base | arch::flush_bits()),
            ("flush already on, round toward zero", base | arch::flush_bits() | arch::ROUNDING),
        ];
        if let Some(partial) = arch::partial_flush_bits() {
            list.push(("FTZ on, DAZ off", base | partial));
            list.push(("FTZ on, DAZ off, round toward zero", base | partial | arch::ROUNDING));
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    /// The smallest positive subnormal `f32`, halved by a multiply.
    fn halve_subnormal() -> f32 {
        black_box(f32::from_bits(2)) * black_box(0.5f32)
    }

    #[test]
    fn guard_flushes_and_restores() {
        let before = DenormalGuard::raw_control();
        {
            let guard = DenormalGuard::new();
            if guard.active() {
                assert!(DenormalGuard::flushing());
                // With flushing a subnormal result becomes zero.
                assert_eq!(halve_subnormal(), 0.0);
            }
        }
        assert_eq!(DenormalGuard::raw_control() & !arch::STATUS, before & !arch::STATUS);
        if DenormalGuard::SUPPORTED && before & arch::flush_bits() == 0 {
            // Without flushing the same multiply keeps a subnormal.
            assert_eq!(halve_subnormal(), f32::from_bits(1));
        }
    }

    #[test]
    fn subnormal_input_reads_as_zero_inside_the_guard() {
        let _guard = DenormalGuard::new();
        if !DenormalGuard::SUPPORTED {
            return;
        }
        // DAZ (x86) or FZ (Arm) treats a subnormal operand as zero:
        // 1e-39 + 0 is not 1e-39 any more.
        let tiny = black_box(1.0e-39f32);
        assert!(tiny.is_subnormal());
        assert_eq!(black_box(tiny) + black_box(0.0f32), 0.0);
    }

    #[test]
    fn new_if_false_leaves_the_mode_alone() {
        let hosts = host_mode::hosts();
        for (name, word) in hosts {
            let old = unsafe { host_mode::set(word) };
            {
                let guard = DenormalGuard::new_if(false);
                assert!(!guard.active());
                assert_eq!(host_mode::control(), word, "{name}: changed by an inactive guard");
            }
            assert_eq!(host_mode::control(), word, "{name}");
            unsafe { host_mode::set(old) };
        }
    }

    #[test]
    fn every_host_mode_comes_back_after_the_guard() {
        let hosts = host_mode::hosts();
        assert_eq!(hosts.is_empty(), !DenormalGuard::SUPPORTED);
        for (name, word) in hosts {
            let old = unsafe { host_mode::set(word) };
            assert_eq!(host_mode::control(), word, "{name}: the host value did not stick");
            {
                let _guard = DenormalGuard::new();
                assert!(DenormalGuard::flushing(), "{name}");
                // The rest of the host's word is kept inside the guard.
                assert_eq!(host_mode::control() & arch::ROUNDING, word & arch::ROUNDING, "{name}");
            }
            assert_eq!(host_mode::control(), word, "{name}: not restored");
            unsafe { host_mode::set(old) };
        }
    }

    #[test]
    fn the_guard_restores_when_a_panic_unwinds_through_it() {
        for (name, word) in host_mode::hosts() {
            let old = unsafe { host_mode::set(word) };
            let caught = std::panic::catch_unwind(|| {
                let _guard = DenormalGuard::new();
                assert!(DenormalGuard::flushing());
                panic!("inside the guard");
            });
            assert!(caught.is_err());
            assert_eq!(host_mode::control(), word, "{name}: not restored after a panic");
            unsafe { host_mode::set(old) };
        }
    }

    #[test]
    fn nested_guards_restore_in_order() {
        let before = host_mode::control();
        {
            let _outer = DenormalGuard::new();
            let inside = host_mode::control();
            {
                let _inner = DenormalGuard::new();
                assert_eq!(host_mode::control(), inside);
            }
            assert_eq!(host_mode::control(), inside);
        }
        assert_eq!(host_mode::control(), before);
    }

    /// A rough look at what the guard buys on this CPU: a decaying one-pole
    /// loop in the subnormal range, with and without flushing. Report only;
    /// run with `cargo test -p openplugin-core --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn report_subnormal_timing() {
        fn run(start: f32) -> (f32, std::time::Duration) {
            let began = std::time::Instant::now();
            let mut y = start;
            let mut sum = 0.0f32;
            for _ in 0..20 {
                y = black_box(start);
                for _ in 0..200_000 {
                    y *= black_box(0.999_9f32);
                    sum += y;
                }
            }
            (black_box(sum + y), began.elapsed())
        }
        let (_, normal) = run(1.0);
        let (_, slow) = run(1.0e-39);
        let guard = DenormalGuard::new();
        let (_, flushed) = run(1.0e-39);
        drop(guard);
        println!(
            "denormal timing on {}: normal {normal:?}, subnormal {slow:?}, subnormal with the guard {flushed:?}",
            std::env::consts::ARCH
        );
    }
}
