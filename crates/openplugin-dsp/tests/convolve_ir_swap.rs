//! Usage example: load a new impulse response during "playback".
//!
//! The main thread submits a job that builds a whole `BlockConvolver` (the
//! transform plan, the buffers and the transformed kernel). The audio thread
//! only calls `Slot::poll` and then processes with the live convolver. The
//! old convolver is dropped on the worker thread.

use openplugin_core::worker::loader;
use openplugin_dsp::BlockConvolver;
use std::time::{Duration, Instant};

fn convolver_with(fir: Vec<f32>) -> BlockConvolver {
    let mut c = BlockConvolver::new(1);
    c.set_kernel(&fir);
    c
}

/// One simulated `process` call.
fn process(slot: &mut openplugin_core::worker::Slot<BlockConvolver>, block: &mut [f32]) -> bool {
    let swapped = slot.poll();
    slot.get_mut().process(0, block);
    swapped
}

#[test]
fn a_new_impulse_loads_during_playback() {
    // Start as a pass-through: a single unit tap.
    let mut unit = vec![0.0f32; 1];
    unit[0] = 1.0;
    let (mut loader, mut slot) = loader(convolver_with(unit), 2);

    let input = vec![0.5f32; 256];
    let mut block = input.clone();
    assert!(!process(&mut slot, &mut block));
    for (got, want) in block.iter().zip(&input) {
        assert!((got - want).abs() < 1e-4);
    }

    // The UI picks a new IR: a gain of 0.25 on the current sample. The job
    // builds the convolver off the audio thread.
    loader
        .submit(|| {
            let mut fir = vec![0.0f32; 512];
            fir[0] = 0.25;
            convolver_with(fir)
        })
        .unwrap();

    // Playback continues while the worker builds it. Blocks before the swap
    // are still the old pass-through; blocks after are scaled.
    let start = Instant::now();
    let mut swapped_at = None;
    for n in 0..1_000_000 {
        let mut block = input.clone();
        let swapped = process(&mut slot, &mut block);
        if swapped {
            swapped_at = Some(n);
        }
        let expect = if swapped_at.is_some() { 0.125 } else { 0.5 };
        let last = block[block.len() - 1];
        assert!((last - expect).abs() < 1e-3, "block {n}: {last} vs {expect}");
        if swapped_at.is_some() && n > swapped_at.unwrap() + 4 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_micros(50));
    }
    assert!(swapped_at.is_some(), "the new impulse never arrived");
}
