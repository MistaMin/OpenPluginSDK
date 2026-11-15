//! Downmix and upmix `process` and `set_settings` do not allocate.
//! One `#[test]` in this binary so no other test can run while armed.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use openplugin_core::layout::SpeakerLayout;
use openplugin_dsp::{Downmix, DownmixTarget, MixSettings, Normalize, Upmix, UpmixSettings};

static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn downmix_and_upmix_do_not_allocate() {
    let mut dm = Downmix::for_layout(
        SpeakerLayout::Surround714,
        DownmixTarget::Stereo,
        MixSettings::default(),
    )
    .unwrap();
    let mut up = Upmix::for_layout(SpeakerLayout::Surround714, UpmixSettings::default()).unwrap();
    let surround = vec![vec![0.25f32; 512]; 12];
    let srefs: Vec<&[f32]> = surround.iter().map(|v| v.as_slice()).collect();
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    let mut wide = vec![vec![0.0f32; 512]; 12];
    let mut wrefs: Vec<&mut [f32]> = wide.iter_mut().map(|v| v.as_mut_slice()).collect();
    let mut stereo: Vec<&mut [f32]> = vec![l.as_mut_slice(), r.as_mut_slice()];
    let sin = [vec![0.5f32; 512], vec![-0.5f32; 512]];
    let sin_refs: Vec<&[f32]> = sin.iter().map(|v| v.as_slice()).collect();

    ARMED.store(true, Ordering::SeqCst);
    for i in 0..100 {
        dm.process(&srefs, &mut stereo);
        up.process(&sin_refs, &mut wrefs);
        if i % 10 == 0 {
            dm.set_settings(MixSettings {
                centre_db: -4.5,
                lfe_db: 0.0,
                normalize: Normalize::Peak,
                ..MixSettings::default()
            });
            up.set_settings(UpmixSettings {
                centre_db: -3.0,
                ..UpmixSettings::default()
            });
        }
    }
    ARMED.store(false, Ordering::SeqCst);
    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);
    assert!(stereo[0][0] != 0.0);
}
