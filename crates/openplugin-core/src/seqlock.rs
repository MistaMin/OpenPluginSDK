//! Single-writer seqlock for a `Copy` snapshot.
//!
//! Water Channel publishes meter and telemetry words under a monotonic sequence
//! so a GUI read is either the previous frame or the next one, never a mix, and
//! so `reset` cannot rewind the sequence into an ABA window. One audio thread
//! publishes. Any number of editor threads may read.
#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicU64, Ordering};

pub struct Seqlock<T> {
    seq: AtomicU64,
    slot: UnsafeCell<T>,
}

unsafe impl<T: Copy + Send> Send for Seqlock<T> {}
unsafe impl<T: Copy + Send> Sync for Seqlock<T> {}

impl<T: Copy> Seqlock<T> {
    pub fn new(value: T) -> Self {
        Self {
            seq: AtomicU64::new(0),
            slot: UnsafeCell::new(value),
        }
    }

    /// Publish `value`. The sequence only moves forward.
    pub fn publish(&self, value: T) {
        self.seq.fetch_add(1, Ordering::Release);
        unsafe {
            self.slot.get().write(value);
        }
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// A consistent copy. Spins while a publish is in progress.
    pub fn read(&self) -> T {
        loop {
            let start = self.seq.load(Ordering::Acquire);
            if start & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let value = unsafe { self.slot.get().read() };
            fence(Ordering::Acquire);
            if start == self.seq.load(Ordering::Acquire) {
                return value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    struct Pair {
        a: u64,
        b: u64,
    }

    #[test]
    fn readers_never_observe_a_torn_pair() {
        let lock = std::sync::Arc::new(Seqlock::new(Pair { a: 0, b: 0 }));
        let writer = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                for n in 1..50_000 {
                    lock.publish(Pair { a: n, b: n });
                }
            })
        };
        let mut last = 0;
        while last < 49_999 {
            let pair = lock.read();
            assert_eq!(pair.a, pair.b);
            last = last.max(pair.a);
        }
        writer.join().unwrap();
        assert_eq!(lock.read().a, 49_999);
    }
}
