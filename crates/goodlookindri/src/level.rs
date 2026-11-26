//! Lock-free stereo level tap. Audio thread calls [`LevelTracker::push`]; UI calls [`LevelTracker::read`].

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub peak: [f32; 2],
    pub mean_square: [f32; 2],
}

fn f32_to_bits(v: f32) -> u32 { v.to_bits() }
fn bits_to_f32(v: u32) -> f32 { f32::from_bits(v) }
fn f64_to_bits(v: f64) -> u64 { v.to_bits() }
fn bits_to_f64(v: u64) -> f64 { f64::from_bits(v) }

pub struct LevelTracker {
    peak: [AtomicU32; 2],
    sum_sq: [AtomicU64; 2],
    count: [AtomicU64; 2],
}
impl Default for LevelTracker {
    fn default() -> Self { Self::new() }
}
impl LevelTracker {
    pub fn new() -> Self {
        Self {
            peak: [AtomicU32::new(0), AtomicU32::new(0)],
            sum_sq: [AtomicU64::new(0), AtomicU64::new(0)],
            count: [AtomicU64::new(0), AtomicU64::new(0)],
        }
    }
    pub fn push(&self, channels: &[&[f32]]) {
        let nch = channels.len().min(2);
        for (c, ch) in channels.iter().take(nch).enumerate() {
            let mut pk = 0.0f32;
            let mut ss = 0.0f64;
            for &v in *ch {
                pk = pk.max(v.abs());
                ss += (v as f64) * (v as f64);
            }
            let mut cur = bits_to_f32(self.peak[c].load(Ordering::Relaxed));
            while pk > cur && self.peak[c].compare_exchange_weak(f32_to_bits(cur), f32_to_bits(pk), Ordering::Relaxed, Ordering::Relaxed).is_err() {
                cur = bits_to_f32(self.peak[c].load(Ordering::Relaxed));
            }
            loop {
                let s = bits_to_f64(self.sum_sq[c].load(Ordering::Relaxed));
                if self.sum_sq[c].compare_exchange_weak(f64_to_bits(s), f64_to_bits(s + ss), Ordering::Relaxed, Ordering::Relaxed).is_ok() { break; }
            }
            self.count[c].fetch_add(ch.len() as u64, Ordering::Relaxed);
        }
    }
    pub fn read(&self) -> Reading {
        let mut r = Reading::default();
        for c in 0..2 {
            r.peak[c] = bits_to_f32(self.peak[c].swap(0, Ordering::Relaxed));
            let s = bits_to_f64(self.sum_sq[c].swap(0, Ordering::Relaxed));
            let n = self.count[c].swap(0, Ordering::Relaxed);
            r.mean_square[c] = if n == 0 { 0.0 } else { (s / n as f64) as f32 };
        }
        r
    }
}
