//! A counting global allocator for the conformance kit.
//!
//! Rust allows one `#[global_allocator]` per binary, so the kit cannot install
//! it for you. Add one line to the test binary that calls
//! [`conformance`](crate::conformance):
//!
//! ```
//! openplugin_testing::install_allocator!();
//! # assert!(openplugin_testing::alloc::is_installed());
//! ```
//!
//! The allocator forwards every call to the system allocator. It counts only
//! while the *calling thread* has armed it, and the counters live in
//! thread-locals, so other tests running in parallel on other threads never
//! add to the count. Without the macro the kit still runs and reports the
//! allocation check as skipped.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static FREES: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn bump_alloc() {
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        }
    });
}

#[inline]
fn bump_free() {
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = FREES.try_with(|c| c.set(c.get() + 1));
        }
    });
}

/// Forwards to [`System`] and counts while armed on the calling thread.
///
/// Install it with [`install_allocator!`](crate::install_allocator), or by hand:
///
/// ```
/// use openplugin_testing::alloc::{is_installed, CountingAllocator};
///
/// #[global_allocator]
/// static ALLOCATOR: CountingAllocator = CountingAllocator;
///
/// assert!(is_installed());
/// ```
pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump_alloc();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump_alloc();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        bump_free();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A realloc may move: it is an allocation and a free.
        bump_alloc();
        bump_free();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Allocations and frees seen while armed, as returned by [`measure`]. Both are zero when
/// [`CountingAllocator`] is not the global allocator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls to `alloc`, `alloc_zeroed` and `realloc` (a `realloc` counts as one of each).
    pub allocs: u64,
    /// Calls to `dealloc` and `realloc`.
    pub frees: u64,
}

struct Disarm;

impl Drop for Disarm {
    fn drop(&mut self) {
        let _ = ARMED.try_with(|a| a.set(false));
    }
}

/// Run `f` with the counters armed on this thread and return what it did.
/// Not re-entrant. If `f` panics the counters are disarmed. Only this thread's
/// allocations are counted; other threads running meanwhile are not.
///
/// ```
/// openplugin_testing::install_allocator!();
///
/// use openplugin_testing::alloc::measure;
///
/// let (sum, counts) = measure(|| [1.0f32, 2.0, 3.0].iter().sum::<f32>());
/// assert_eq!(sum, 6.0);
/// assert_eq!(counts.allocs, 0);
///
/// let (_, counts) = measure(|| drop(std::hint::black_box(vec![0u8; 16])));
/// assert_eq!(counts.allocs, 1);
/// ```
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Counts) {
    ALLOCS.with(|c| c.set(0));
    FREES.with(|c| c.set(0));
    let guard = Disarm;
    ARMED.with(|a| a.set(true));
    let result = f();
    drop(guard);
    let counts = Counts { allocs: ALLOCS.with(Cell::get), frees: FREES.with(Cell::get) };
    (result, counts)
}

/// Whether [`CountingAllocator`] is the global allocator of this binary.
/// The kit proves it by making a real allocation under [`measure`].
pub fn is_installed() -> bool {
    let (_, counts) = measure(|| {
        let b = std::hint::black_box(Box::new([0u8; 64]));
        drop(std::hint::black_box(b));
    });
    counts.allocs > 0 && counts.frees > 0
}

/// Install the counting allocator as this binary's global allocator.
/// Use once, at the top level of the test file (or `main.rs` under `cfg(test)`).
/// A second `#[global_allocator]` in the same binary is a compile error.
///
/// ```
/// openplugin_testing::install_allocator!();
///
/// assert!(openplugin_testing::alloc::is_installed());
/// ```
#[macro_export]
macro_rules! install_allocator {
    () => {
        #[global_allocator]
        static OPENPLUGIN_TESTING_ALLOCATOR: $crate::alloc::CountingAllocator =
            $crate::alloc::CountingAllocator;
    };
}
