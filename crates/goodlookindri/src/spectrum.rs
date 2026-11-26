//! Spectrum analysis ported from GoodLookinUI `Spectrum.h`.
//! Allocate in `configure`. `analyse` and `decay` do not allocate.
//! The audio thread only calls [`SampleRing::push`]. FFT stays on the UI thread.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const FFT_SIZES: [i32; 6] = [512, 1024, 2048, 4096, 8192, 16384];
pub const FLOOR_DB: f32 = -120.0;

const CAPACITY: usize = 1 << 15;

pub struct SampleRing {
    data: Box<[AtomicU32]>,
    write_pos: AtomicU64,
}
impl Default for SampleRing { fn default() -> Self { Self::new() } }
impl SampleRing {
    pub fn new() -> Self {
        let mut v = Vec::with_capacity(CAPACITY);
        for _ in 0..CAPACITY { v.push(AtomicU32::new(0)); }
        Self { data: v.into_boxed_slice(), write_pos: AtomicU64::new(0) }
    }
    pub fn push(&self, channels: &[&[f32]]) {
        if channels.is_empty() { return; }
        let n = channels[0].len();
        if n == 0 { return; }
        let w = self.write_pos.load(Ordering::Relaxed);
        let inv = 1.0 / channels.len() as f32;
        for i in 0..n {
            let mut s = 0.0f32;
            for ch in channels { s += ch[i]; }
            let idx = ((w + i as u64) as usize) & (CAPACITY - 1);
            self.data[idx].store((s * inv).to_bits(), Ordering::Relaxed);
        }
        self.write_pos.store(w + n as u64, Ordering::Release);
    }
    pub fn position(&self) -> u64 { self.write_pos.load(Ordering::Acquire) }
    pub fn copy_latest(&self, dst: &mut [f32]) -> bool {
        let count = dst.len();
        if count == 0 || count > CAPACITY / 2 { return false; }
        let end = self.write_pos.load(Ordering::Acquire);
        if end < count as u64 { return false; }
        let start = end - count as u64;
        for i in 0..count {
            let idx = ((start + i as u64) as usize) & (CAPACITY - 1);
            dst[i] = f32::from_bits(self.data[idx].load(Ordering::Relaxed));
        }
        self.write_pos.load(Ordering::Acquire) - start <= (CAPACITY as u64) - 64
    }
}

pub struct RealFft {
    size: usize,
    half: usize,
    rev: Vec<usize>,
    cos_t: Vec<f32>,
    sin_t: Vec<f32>,
    wr: Vec<f32>,
    wi: Vec<f32>,
    zr: Vec<f32>,
    zi: Vec<f32>,
}
impl RealFft {
    pub fn new(n: usize) -> Self { let mut f = Self { size: 0, half: 0, rev: vec![], cos_t: vec![], sin_t: vec![], wr: vec![], wi: vec![], zr: vec![], zi: vec![] }; f.configure(n); f }
    pub fn configure(&mut self, n: usize) {
        self.size = n; self.half = n / 2;
        let mut bits = 0;
        while (1usize << bits) < self.half { bits += 1; }
        self.rev = (0..self.half).map(|i| {
            let mut r = 0usize;
            for b in 0..bits { if i & (1 << b) != 0 { r |= 1 << (bits - 1 - b); } }
            r
        }).collect();
        let tw = (self.half / 2).max(1);
        self.cos_t = vec![0.0; tw];
        self.sin_t = vec![0.0; tw];
        for k in 0..self.half / 2 {
            let a = -2.0 * std::f64::consts::PI * k as f64 / self.half as f64;
            self.cos_t[k] = a.cos() as f32;
            self.sin_t[k] = a.sin() as f32;
        }
        self.wr = vec![0.0; self.half + 1];
        self.wi = vec![0.0; self.half + 1];
        for k in 0..=self.half {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            self.wr[k] = a.cos() as f32;
            self.wi[k] = a.sin() as f32;
        }
        self.zr = vec![0.0; self.half];
        self.zi = vec![0.0; self.half];
    }
    pub fn size(&self) -> usize { self.size }
    pub fn forward(&mut self, input: &[f32], re: &mut [f32], im: &mut [f32]) {
        let half = self.half;
        for i in 0..half {
            let r = self.rev[i];
            self.zr[r] = input[2 * i];
            self.zi[r] = input[2 * i + 1];
        }
        let mut len = 2;
        while len <= half {
            let h = len >> 1;
            let step = half / len;
            let mut i = 0;
            while i < half {
                for j in 0..h {
                    let c = self.cos_t[j * step];
                    let s = self.sin_t[j * step];
                    let ar = self.zr[i + j]; let ai = self.zi[i + j];
                    let br = self.zr[i + j + h]; let bi = self.zi[i + j + h];
                    let tr = br * c - bi * s;
                    let ti = br * s + bi * c;
                    self.zr[i + j + h] = ar - tr; self.zi[i + j + h] = ai - ti;
                    self.zr[i + j] = ar + tr; self.zi[i + j] = ai + ti;
                }
                i += len;
            }
            len <<= 1;
        }
        for k in 0..=half {
            let a = k % half;
            let b = (half - k) % half;
            let zkr = self.zr[a]; let zki = self.zi[a];
            let zcr = self.zr[b]; let zci = -self.zi[b];
            let er = 0.5 * (zkr + zcr); let ei = 0.5 * (zki + zci);
            let dr = 0.5 * (zkr - zcr); let di = 0.5 * (zki - zci);
            let orr = di; let oi = -dr;
            let c = self.wr[k]; let s = self.wi[k];
            re[k] = er + orr * c - oi * s;
            im[k] = ei + orr * s + oi * c;
        }
    }
}

pub struct Analyser {
    n: usize,
    sr: f64,
    norm: f32,
    tilt: f32,
    fall: f32,
    fft: RealFft,
    window: Vec<f32>,
    frame: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    display: Vec<f32>,
    hold: Vec<f32>,
    tilt_db: Vec<f32>,
    last_pos: u64,
    frames: u64,
    have_frame: bool,
}
impl Default for Analyser {
    fn default() -> Self {
        Self { n: 0, sr: 48000.0, norm: 1.0, tilt: 0.0, fall: 24.0, fft: RealFft::new(2), window: vec![], frame: vec![], re: vec![], im: vec![], display: vec![], hold: vec![], tilt_db: vec![], last_pos: 0, frames: 0, have_frame: false }
    }
}
impl Analyser {
    pub fn configure(&mut self, fft_size: usize, sample_rate: f64) {
        self.n = fft_size; self.sr = sample_rate;
        self.fft.configure(fft_size);
        self.window.resize(fft_size, 0.0);
        let mut sum = 0.0f64;
        for i in 0..fft_size {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / fft_size as f64).cos();
            self.window[i] = w as f32;
            sum += w;
        }
        self.norm = (2.0 / sum) as f32;
        let bins = fft_size / 2 + 1;
        self.frame = vec![0.0; fft_size];
        self.re = vec![0.0; bins];
        self.im = vec![0.0; bins];
        self.display = vec![FLOOR_DB; bins];
        self.hold = vec![FLOOR_DB; bins];
        self.tilt_db = vec![0.0; bins];
        self.last_pos = 0; self.have_frame = false;
        let tilt = self.tilt;
        self.set_tilt(tilt);
    }
    pub fn set_sample_rate(&mut self, s: f64) { if s != self.sr && self.n > 0 { let n = self.n; self.configure(n, s); } }
    pub fn set_tilt(&mut self, db_per_octave: f32) {
        self.tilt = db_per_octave;
        for k in 0..self.tilt_db.len() {
            self.tilt_db[k] = if k == 0 { 0.0 } else { self.tilt * ((k as f64 * self.sr / self.n as f64) as f32 / 1000.0).log2() };
        }
    }
    pub fn set_fall(&mut self, db_per_second: f32) { self.fall = db_per_second; }
    pub fn fft_size(&self) -> usize { self.n }
    pub fn num_bins(&self) -> usize { self.n / 2 + 1 }
    pub fn sample_rate(&self) -> f64 { self.sr }
    pub fn display_db(&self) -> &[f32] { &self.display }
    pub fn hold_db(&self) -> &[f32] { &self.hold }
    pub fn frames_analysed(&self) -> u64 { self.frames }
    pub fn analyse(&mut self, ring: &SampleRing, min_new: u64) -> bool {
        let pos = ring.position();
        if self.have_frame && pos - self.last_pos < min_new { return false; }
        if !ring.copy_latest(&mut self.frame) { return false; }
        self.last_pos = pos; self.have_frame = true;
        let frame = self.frame.clone();
        self.process(&frame);
        true
    }
    pub fn process(&mut self, samples: &[f32]) {
        for i in 0..self.n { self.frame[i] = samples[i] * self.window[i]; }
        self.fft.forward(&self.frame.clone(), &mut self.re, &mut self.im);
        let n = self.n;
        let norm = self.norm;
        for k in 0..self.re.len() {
            let mag = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * norm * if k == 0 || k == n / 2 { 0.5 } else { 1.0 };
            let db = if mag > 1.0e-7 { 20.0 * mag.log10() } else { FLOOR_DB };
            let v = FLOOR_DB.max(db + self.tilt_db[k]);
            self.display[k] = v.max(self.display[k]);
            self.hold[k] = v.max(self.hold[k]);
        }
        self.frames += 1;
    }
    pub fn decay(&mut self, seconds: f32) {
        let drop = self.fall * seconds;
        let hold_drop = 6.0 * seconds;
        for k in 0..self.display.len() {
            self.display[k] = FLOOR_DB.max(self.display[k] - drop);
            self.hold[k] = self.display[k].max(self.hold[k] - hold_drop);
        }
    }
    pub fn reset(&mut self) {
        self.display.fill(FLOOR_DB);
        self.hold.fill(FLOOR_DB);
        self.have_frame = false;
    }
}

pub fn to_columns(db: &[f32], sample_rate: f64, cols: usize, f_min: f32, f_max: f32, out: &mut [f32]) {
    let num_bins = db.len();
    let bin_hz = sample_rate / (2.0 * (num_bins as f64 - 1.0));
    let nyquist = sample_rate * 0.5;
    let ratio = (f_max as f64 / f_min as f64).powf(1.0 / cols as f64);
    let mut f0 = f_min as f64;
    for c in 0..cols {
        let f1 = f0 * ratio;
        if f0 >= nyquist { out[c] = FLOOR_DB; f0 = f1; continue; }
        let b0 = f0 / bin_hz;
        let b1 = f1.min(nyquist) / bin_hz;
        let i0 = b0.ceil() as i32;
        let i1 = b1.floor() as i32;
        if i1 >= i0 {
            let mut m = FLOOR_DB;
            let lo = i0.max(1);
            let hi = i1.min(num_bins as i32 - 1);
            for i in lo..=hi { m = m.max(db[i as usize]); }
            out[c] = m;
        } else {
            let b = 0.5 * (b0 + b1);
            let lo = (b.floor() as i32).max(0) as usize;
            let hi = (lo + 1).min(num_bins - 1);
            let t = (b - lo as f64) as f32;
            out[c] = db[lo] + (db[hi] - db[lo]) * t;
        }
        f0 = f1;
    }
}

pub fn smooth_columns(input: &[f32], out: &mut [f32], octaves_spanned: f32, denominator: i32, scratch: &mut [f32], prefix: &mut [f64]) {
    let cols = input.len();
    let per = cols as f32 / octaves_spanned;
    let w = (per / denominator as f32 * 0.5).round().max(1.0) as i32;
    if w <= 1 { out[..cols].copy_from_slice(input); return; }
    let box_pass = |src: &[f32], dst: &mut [f32], prefix: &mut [f64]| {
        prefix[0] = 0.0;
        for i in 0..cols { prefix[i + 1] = prefix[i] + 10f64.powf(src[i] as f64 / 10.0); }
        let r = w / 2;
        for i in 0..cols {
            let lo = (i as i32 - r).max(0) as usize;
            let hi = ((i as i32 + r).min(cols as i32 - 1)) as usize;
            let mean = (prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64;
            dst[i] = if mean > 1.0e-12 { (10.0 * mean.log10()) as f32 } else { FLOOR_DB };
        }
    };
    box_pass(input, scratch, prefix);
    box_pass(scratch, out, prefix);
}

pub struct SpectrogramHistory { cols: usize, nrows: usize, head: usize, count: usize, data: Vec<u8> }
impl SpectrogramHistory {
    pub fn new() -> Self { Self { cols: 0, nrows: 0, head: 0, count: 0, data: vec![] } }
    pub fn configure(&mut self, columns: usize, rows: usize) {
        self.cols = columns; self.nrows = rows; self.data = vec![0; columns * rows]; self.head = 0; self.count = 0;
    }
    pub fn columns(&self) -> usize { self.cols }
    pub fn rows(&self) -> usize { self.nrows }
    pub fn filled(&self) -> usize { self.count }
    pub fn push(&mut self, db: &[f32], floor_level_db: f32, ceil_level_db: f32) {
        if self.nrows == 0 { return; }
        self.head = (self.head + self.nrows - 1) % self.nrows;
        let scale = 255.0 / (ceil_level_db - floor_level_db);
        let start = self.head * self.cols;
        for c in 0..self.cols {
            let v = ((db[c] - floor_level_db) * scale).clamp(0.0, 255.0).round() as u8;
            self.data[start + c] = v;
        }
        self.count = (self.count + 1).min(self.nrows);
    }
    pub fn row(&self, age: usize) -> &[u8] {
        let i = (self.head + age) % self.nrows;
        &self.data[i * self.cols..(i + 1) * self.cols]
    }
}
impl Default for SpectrogramHistory { fn default() -> Self { Self::new() } }

pub fn heat_colour(v: u8) -> u32 {
    const STOPS: [(f32, i32, i32, i32); 6] = [(0.0, 0, 0, 0), (0.18, 10, 20, 90), (0.40, 140, 20, 150), (0.62, 245, 90, 30), (0.82, 255, 215, 60), (1.0, 255, 255, 235)];
    let t = v as f32 / 255.0;
    let mut i = 0;
    while i < 4 && t > STOPS[i + 1].0 { i += 1; }
    let u = ((t - STOPS[i].0) / (STOPS[i + 1].0 - STOPS[i].0)).clamp(0.0, 1.0);
    let mix = |a: i32, b: i32| (a as f32 + (b as f32 - a as f32) * u).round() as u32;
    0xff00_0000 | (mix(STOPS[i].1, STOPS[i + 1].1) << 16) | (mix(STOPS[i].2, STOPS[i + 1].2) << 8) | mix(STOPS[i].3, STOPS[i + 1].3)
}
pub fn shade_colour(base: u32, v: u8) -> u32 {
    let t = v as f32 / 255.0;
    let ch = |sh: u32| {
        let c = ((base >> sh) & 255) as f32;
        let lit = if t < 0.7 { c * (t / 0.7) } else { c + (255.0 - c) * ((t - 0.7) / 0.3) * 0.6 };
        lit.clamp(0.0, 255.0).round() as u32
    };
    0xff00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

pub struct Range { pub name: &'static str, pub lo: f32, pub hi: f32 }
pub const RANGES: [Range; 7] = [
    Range { name: "SUB", lo: 20.0, hi: 60.0 }, Range { name: "BASS", lo: 60.0, hi: 250.0 },
    Range { name: "LOW MID", lo: 250.0, hi: 500.0 }, Range { name: "MID", lo: 500.0, hi: 2000.0 },
    Range { name: "HIGH MID", lo: 2000.0, hi: 4000.0 }, Range { name: "PRESENCE", lo: 4000.0, hi: 8000.0 },
    Range { name: "AIR", lo: 8000.0, hi: 30000.0 },
];
pub const DEFAULT_COLOURS: [u32; 7] = [0xffff3b4a, 0xffff9a1f, 0xffffe23a, 0xff5be85a, 0xff35e0d0, 0xff3f8ff0, 0xffc05cff];

pub fn colour_at(freq: f32, cols: &[u32; 7]) -> u32 {
    let centres: [f32; 7] = std::array::from_fn(|i| (RANGES[i].lo * RANGES[i].hi).sqrt());
    if freq <= centres[0] { return cols[0]; }
    if freq >= centres[6] { return cols[6]; }
    let mut i = 0;
    while freq > centres[i + 1] { i += 1; }
    let t = (freq.ln() - centres[i].ln()) / (centres[i + 1].ln() - centres[i].ln());
    let mix = |a: u32, b: u32, sh: u32| (((a >> sh) & 255) as f32 * (1.0 - t) + ((b >> sh) & 255) as f32 * t).round() as u32 & 255;
    0xff00_0000 | (mix(cols[i], cols[i + 1], 16) << 16) | (mix(cols[i], cols[i + 1], 8) << 8) | mix(cols[i], cols[i + 1], 0)
}
pub fn range_index_for(freq: f32) -> usize {
    for i in 0..7 { if freq < RANGES[i].hi { return i; } }
    6
}
