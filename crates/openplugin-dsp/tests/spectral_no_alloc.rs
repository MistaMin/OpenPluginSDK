//! `Stft::process*` and `SpectrumAnalyzer::process` / `SpectrumWriter::publish`
//! must not allocate, deallocate or reallocate.
//!
//! A counting `#[global_allocator]` forwards to `System` and counts only on the
//! thread that armed it, as in `dither_no_alloc.rs`. A probe first proves the counter
//! sees an allocation, so the test cannot pass vacuously.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use openplugin_dsp::{
    log_frequency_bins, Complex, Identity, SpectrumAnalyzer, SpectrumConfig, Stft, StftConfig,
    WindowKind,
};

thread_local! {
    // Armed per thread: the test harness's own threads (output capture, the
    // "running for over 60 seconds" notice) never add to the count. `const`
    // initialisation, so reading it inside the allocator never allocates.
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn armed() -> bool {
    ARMED.try_with(|a| a.get()).unwrap_or(false)
}
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if armed() {
            DEALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if armed() {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

fn arm() {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    DEALLOCATIONS.store(0, Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
}

fn disarm() -> (usize, usize) {
    ARMED.with(|a| a.set(false));
    (
        ALLOCATIONS.load(Ordering::SeqCst),
        DEALLOCATIONS.load(Ordering::SeqCst),
    )
}

#[test]
fn spectral_audio_path_does_not_allocate() {
    arm();
    drop(std::hint::black_box(Box::new([0u8; 64])));
    let (a, d) = disarm();
    assert!(a >= 1 && d >= 1, "allocator hook is not live");

    let input: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
    let mut buf = input.clone();
    let mut out = vec![0.0f32; 4096];

    let mut stft = Stft::new(
        2,
        StftConfig {
            frame: 1024,
            hop: 256,
            analysis: WindowKind::Hann,
            synthesis: WindowKind::Rectangular,
        },
    )
    .unwrap();
    let (mut analyzer, reader) = SpectrumAnalyzer::new(SpectrumConfig::new(2048, 48_000.0)).unwrap();
    let mut mags = vec![0.0f32; reader.bins()];
    let mut peaks = vec![0.0f32; reader.bins()];
    let mut cols = vec![0.0f32; 256];
    let mut gate = |_: usize, bins: &mut [Complex<f32>]| {
        for b in bins.iter_mut() {
            *b *= 0.5;
        }
    };

    arm();
    for chunk in buf.chunks_mut(37) {
        stft.process_in_place(0, chunk, &mut Identity);
    }
    for chunk in input.chunks(64) {
        let n = chunk.len();
        let mut tmp = [0.0f32; 64];
        stft.process(1, chunk, &mut tmp[..n], &mut gate);
    }
    stft.process(0, &input, &mut out, &mut gate);
    for chunk in input.chunks(1) {
        analyzer.process(chunk);
    }
    analyzer.process(&input);
    let got = reader.read(&mut mags, &mut peaks);
    log_frequency_bins(&mags, 48_000.0, 2048, 20.0, 20_000.0, &mut cols);
    let (a, d) = disarm();
    assert!(got.is_some());
    assert_eq!((a, d), (0, 0), "audio path allocated {a} / freed {d}");
}
