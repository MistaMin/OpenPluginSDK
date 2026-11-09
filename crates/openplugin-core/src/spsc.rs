//! Fixed-capacity single-producer, single-consumer queue.
//!
//! This is the ownership-transfer ring from the MIT OpenGridder client (the
//! archived tree and the current `ag-plugin` queue). Endpoints are not `Sync`.
//! A successful push or pop does not allocate, park, or drop an element.
#![allow(unsafe_code)]

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    read: AtomicUsize,
    write: AtomicUsize,
}

// Each slot is owned by one endpoint at a time. Release/Acquire hands that
// ownership to the other endpoint before it reads or reuses the slot.
unsafe impl<T: Send> Sync for Ring<T> {}
unsafe impl<T: Send> Send for Ring<T> {}

pub struct Producer<T>(Arc<Ring<T>>, PhantomData<Cell<()>>);
pub struct Consumer<T>(Arc<Ring<T>>, PhantomData<Cell<()>>);

pub fn channel<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    assert!(capacity > 0, "spsc capacity must be non-zero");
    let ring = Arc::new(Ring {
        slots: (0..capacity + 1)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect(),
        read: AtomicUsize::new(0),
        write: AtomicUsize::new(0),
    });
    (
        Producer(ring.clone(), PhantomData),
        Consumer(ring, PhantomData),
    )
}

impl<T> Producer<T> {
    /// Returns the item when the ring is full.
    pub fn push(&mut self, item: T) -> Result<(), T> {
        let write = self.0.write.load(Ordering::Relaxed);
        let next = (write + 1) % self.0.slots.len();
        if next == self.0.read.load(Ordering::Acquire) {
            return Err(item);
        }
        unsafe {
            (*self.0.slots[write].get()).write(item);
        }
        self.0.write.store(next, Ordering::Release);
        Ok(())
    }
}

impl<T> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let read = self.0.read.load(Ordering::Relaxed);
        if read == self.0.write.load(Ordering::Acquire) {
            return None;
        }
        let item = unsafe { (*self.0.slots[read].get()).assume_init_read() };
        self.0
            .read
            .store((read + 1) % self.0.slots.len(), Ordering::Release);
        Some(item)
    }
}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let mut read = *self.read.get_mut();
        let write = *self.write.get_mut();
        while read != write {
            unsafe {
                self.slots[read].get_mut().assume_init_drop();
            }
            read = (read + 1) % self.slots.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_transfer_and_wraparound() {
        let (mut tx, mut rx) = channel(2);
        for n in 0..100 {
            assert_eq!(tx.push(n), Ok(()));
            assert_eq!(tx.push(n + 1), Ok(()));
            assert_eq!(tx.push(n + 2), Err(n + 2));
            assert_eq!(rx.pop(), Some(n));
            assert_eq!(rx.pop(), Some(n + 1));
            assert_eq!(rx.pop(), None);
        }
    }

    #[test]
    fn concurrent_ordering() {
        let (mut tx, mut rx) = channel(16);
        let producer = std::thread::spawn(move || {
            for n in 0..100_000 {
                let mut pending = n;
                while let Err(item) = tx.push(pending) {
                    pending = item;
                    std::thread::yield_now();
                }
            }
        });
        for expected in 0..100_000 {
            loop {
                if let Some(n) = rx.pop() {
                    assert_eq!(n, expected);
                    break;
                }
                std::thread::yield_now();
            }
        }
        producer.join().unwrap();
    }
}
