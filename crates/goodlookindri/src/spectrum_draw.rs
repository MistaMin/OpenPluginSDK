//! Spectrum looks from `SpectrumRenderer.h`: standard, multicolour, bars, spectrogram.
//! Painting is command-list only. The FFT stays in [`crate::spectrum`].
//!
//! 3D 2.0 look: the plot is a recessed screen (bevel, inner shadow, vignette, faint glass), with a
//! log-frequency / dB graticule behind the trace. The area fill and the phosphor bloom of the
//! line looks are baked into one [`Image`] per paint: the fill is a multi-stop vertical gradient
//! that fades out at the bottom, the bloom is stored as premultiplied pixels whose colour exceeds
//! their alpha, so a premultiplied source-over blit adds light. The trace itself and the peak-hold
//! trace stay vector strokes over a monotone-cubic resampling of the columns. The spectrogram is
//! one image through a perceptual colour map ([`colour_map`]). Hosts replaying
//! [`Scene::lower_basic`] lose the images and keep screen, graticule, traces and bars.

use crate::color::Color;
use crate::scene::{Blend, Image, Paint, Point, Rect, Scene, Shape, Stops};
use crate::spectrum::{self, colour_at, shade_colour, Analyser, SampleRing, SpectrogramHistory, FLOOR_DB};
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectrumStyle { Standard, Multicolour, Bars, Spectrogram }

impl SpectrumStyle {
    pub fn code(self) -> &'static str {
        match self { Self::Multicolour => "multi", Self::Bars => "bars", Self::Spectrogram => "gram", Self::Standard => "standard" }
    }
    pub fn from_code(c: &str) -> Self {
        match c { "multi" => Self::Multicolour, "bars" => Self::Bars, "gram" => Self::Spectrogram, _ => Self::Standard }
    }
}

/// Where trace colours come from. `Standard` style uses one colour; `Multicolour`, `Bars` and the
/// shaded spectrogram run a ramp from low to high frequency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpectrumPalette {
    /// [`SpectrumSettings::standard`], and an analogous ramp around its hue.
    #[default]
    Accent,
    /// Warm phosphor amber, ember red in the lows to pale gold in the highs.
    Amber,
    /// Cool cyan, indigo in the lows to pale aqua in the highs.
    Cyan,
    /// The seven per-range colours in [`SpectrumSettings::range`] (the original rainbow).
    Range,
}
impl SpectrumPalette {
    pub const ALL: [SpectrumPalette; 4] = [Self::Accent, Self::Amber, Self::Cyan, Self::Range];
    pub fn code(self) -> &'static str {
        match self { Self::Accent => "accent", Self::Amber => "amber", Self::Cyan => "cyan", Self::Range => "range" }
    }
    pub fn from_code(c: &str) -> Self {
        match c { "amber" => Self::Amber, "cyan" => Self::Cyan, "range" => Self::Range, _ => Self::Accent }
    }
}

/// Spectrogram colour maps for [`SpectrumSettings::spectrogram_map`].
pub const MAP_INFERNO: i32 = 0;
/// Level shades the palette colour of each frequency.
pub const MAP_SHADE: i32 = 1;
pub const MAP_MAGMA: i32 = 2;
pub const MAP_ICE: i32 = 3;

#[derive(Clone, Debug)]
pub struct SpectrumSettings {
    pub on: bool,
    pub size_index: i32,
    pub style: SpectrumStyle,
    pub use_output: bool,
    pub tilt: f32,
    pub fall: f32,
    pub opacity: f32,
    pub peak_hold: bool,
    pub smoothing: i32,
    /// [`MAP_INFERNO`], [`MAP_SHADE`], [`MAP_MAGMA`] or [`MAP_ICE`].
    pub spectrogram_map: i32,
    pub standard: u32,
    pub range: [u32; 7],
    pub palette: SpectrumPalette,
    /// Recessed screen behind the plot: bevel, dark glass, vignette, inner shadow, reflection.
    pub frame: bool,
    /// Frequency and dB graticule.
    pub grid: bool,
    /// Graticule labels (needs `grid`).
    pub labels: bool,
    /// Phosphor bloom strength, 0 (off) to 2.
    pub bloom: f32,
}
impl Default for SpectrumSettings {
    fn default() -> Self {
        Self {
            on: true, size_index: 3, style: SpectrumStyle::Standard, use_output: true, tilt: 3.0, fall: 24.0, opacity: 0.85, peak_hold: true,
            smoothing: 0, spectrogram_map: 0, standard: 0xff8fe0c0, range: spectrum::DEFAULT_COLOURS,
            palette: SpectrumPalette::Accent, frame: true, grid: true, labels: true, bloom: 1.0,
        }
    }
}
impl SpectrumSettings {
    pub const SMOOTHING_CHOICES: [i32; 6] = [0, 1, 3, 6, 12, 24];
    /// The 18 GoodLookinUI fields, then `palette,flags,bloom` only when one of those differs from
    /// its default (flags: 1 frame, 2 grid, 4 labels).
    pub fn to_string(&self) -> String {
        let mut s = format!("{},{},{},{},{},{},{},{},{:x}",
            if self.on { 1 } else { 0 }, self.size_index, self.style.code(), if self.use_output { 1 } else { 0 },
            self.tilt, self.fall, self.opacity, if self.peak_hold { 1 } else { 0 }, self.standard);
        for c in self.range { s.push(','); s.push_str(&format!("{c:x}")); }
        s.push(','); s.push_str(&self.smoothing.to_string());
        s.push(','); s.push_str(&self.spectrogram_map.to_string());
        let d = Self::default();
        if self.palette != d.palette || self.frame != d.frame || self.grid != d.grid || self.labels != d.labels || self.bloom != d.bloom {
            let flags = self.frame as i32 | (self.grid as i32) << 1 | (self.labels as i32) << 2;
            s.push_str(&format!(",{},{},{}", self.palette.code(), flags, self.bloom));
        }
        s
    }
    /// Strings without the palette fields keep the per-range colours when those were customised.
    pub fn from_string(text: &str) -> Self {
        let mut d = Self::default();
        let t: Vec<&str> = text.split(',').collect();
        if t.len() != 16 && t.len() != 18 && t.len() != 21 { return d; }
        d.on = t[0] != "0";
        d.size_index = t[1].parse::<i32>().unwrap_or(0).clamp(0, spectrum::FFT_SIZES.len() as i32 - 1);
        d.style = SpectrumStyle::from_code(t[2]);
        d.use_output = t[3] != "0";
        d.tilt = t[4].parse::<f32>().unwrap_or(d.tilt).clamp(-6.0, 9.0);
        d.fall = t[5].parse::<f32>().unwrap_or(d.fall).clamp(2.0, 200.0);
        d.opacity = t[6].parse::<f32>().unwrap_or(d.opacity).clamp(0.1, 1.0);
        d.peak_hold = t[7] != "0";
        d.standard = 0xff00_0000 | u32::from_str_radix(t[8], 16).unwrap_or(0);
        for i in 0..7 { d.range[i] = 0xff00_0000 | u32::from_str_radix(t[9 + i], 16).unwrap_or(0); }
        if t.len() >= 18 {
            d.smoothing = t[16].parse::<i32>().unwrap_or(0).clamp(0, 48);
            d.spectrogram_map = t[17].parse::<i32>().unwrap_or(0).clamp(0, MAP_ICE);
        }
        if t.len() == 21 {
            d.palette = SpectrumPalette::from_code(t[18]);
            let flags = t[19].parse::<i32>().unwrap_or(7);
            d.frame = flags & 1 != 0;
            d.grid = flags & 2 != 0;
            d.labels = flags & 4 != 0;
            d.bloom = t[20].parse::<f32>().unwrap_or(1.0).clamp(0.0, 2.0);
        } else if d.range != spectrum::DEFAULT_COLOURS {
            d.palette = SpectrumPalette::Range;
        }
        d
    }

    /// The single trace colour of the `Standard` look.
    pub fn trace_colour(&self) -> Color {
        match self.palette {
            SpectrumPalette::Amber => Color(0xffffb347),
            SpectrumPalette::Cyan => Color(0xff4fd8ff),
            _ => Color(0xff00_0000 | self.standard),
        }
    }
    /// Palette colour at `t` = 0 (lowest frequency) to 1 (highest) for the multicolour looks.
    /// `freq` is only used by [`SpectrumPalette::Range`].
    pub fn ramp_colour(&self, t: f32, freq: f32) -> Color {
        const AMBER: [u32; 4] = [0xffe8452c, 0xffff8a1f, 0xffffc247, 0xffffe9a3];
        const CYAN: [u32; 4] = [0xff6a5cf5, 0xff2f9bff, 0xff35d6f0, 0xff9af5dc];
        let four = |c: &[u32; 4]| {
            let u = t.clamp(0.0, 1.0) * 3.0;
            let i = (u as usize).min(2);
            Color(c[i]).mix(Color(c[i + 1]), u - i as f32)
        };
        match self.palette {
            SpectrumPalette::Amber => four(&AMBER),
            SpectrumPalette::Cyan => four(&CYAN),
            SpectrumPalette::Range => Color(colour_at(freq, &self.range)),
            SpectrumPalette::Accent => {
                let (h, s, v) = Color(self.standard).to_hsv();
                let t = t.clamp(0.0, 1.0);
                Color::from_hsv(h + 0.13 - 0.24 * t, (s * 1.2 + 0.22 - 0.12 * t).min(1.0), (v * 1.04).min(1.0))
            }
        }
    }
}

/// Perceptual spectrogram ramp: `map` is [`MAP_INFERNO`], [`MAP_MAGMA`] or [`MAP_ICE`]
/// (anything else is inferno), `v` the level 0..255. Opaque straight `0xAARRGGBB`.
pub fn colour_map(map: i32, v: u8) -> u32 {
    const INFERNO: [[u8; 3]; 9] = [[0, 0, 4], [31, 12, 72], [85, 15, 109], [136, 34, 106], [186, 54, 85], [227, 89, 51], [249, 140, 10], [249, 201, 50], [252, 255, 164]];
    const MAGMA: [[u8; 3]; 9] = [[0, 0, 4], [28, 16, 68], [79, 18, 123], [129, 37, 129], [181, 54, 122], [229, 80, 100], [251, 135, 97], [254, 194, 135], [252, 253, 191]];
    const ICE: [[u8; 3]; 9] = [[1, 3, 8], [6, 18, 44], [10, 38, 84], [14, 66, 120], [20, 100, 152], [34, 140, 178], [62, 184, 200], [140, 228, 222], [236, 255, 246]];
    let tab = match map { MAP_MAGMA => &MAGMA, MAP_ICE => &ICE, _ => &INFERNO };
    let u = v as f32 / 255.0 * 8.0;
    let i = (u as usize).min(7);
    let f = u - i as f32;
    let ch = |k: usize| (tab[i][k] as f32 + (tab[i + 1][k] as f32 - tab[i][k] as f32) * f).round() as u32;
    0xff00_0000 | (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

/// Spectrogram picture for [`SpectrumView::paint_gram_image`]: `rows` is row-major levels,
/// `columns` wide, age 0 at the top. Columns above Nyquist are transparent (`sample_rate` 0 keeps
/// every column). One texel per cell; the blit's bilinear filter does the smoothing.
pub fn spectrogram_image(settings: &SpectrumSettings, rows: &[u8], columns: usize, f_min: f32, f_max: f32, sample_rate: f64) -> Option<Arc<Image>> {
    if columns == 0 || rows.len() < columns || f_min <= 0.0 || f_max <= f_min { return None; }
    let n = columns;
    let nrows = rows.len() / n;
    let nv = visible_columns(n, f_min, f_max, sample_rate);
    let mut lut = [0u32; 256];
    let shade = settings.spectrogram_map == MAP_SHADE;
    // Levels go through a contrast curve first, so quiet detail lifts out of the black and loud
    // partials reach the bright end of the ramp.
    let curve: Vec<u8> = (0..256).map(|v| level_curve(v as u8)).collect();
    if !shade { for (v, l) in lut.iter_mut().enumerate() { *l = colour_map(settings.spectrogram_map, curve[v]); } }
    let base: Vec<u32> = if shade {
        (0..n).map(|c| {
            let t = (c as f32 + 0.5) / n as f32;
            settings.ramp_colour(t, f_min * (f_max / f_min).powf(t)).0
        }).collect()
    } else { Vec::new() };
    let mut px = vec![0u32; n * nrows];
    for r in 0..nrows {
        let src = &rows[r * n..(r + 1) * n];
        let dst = &mut px[r * n..(r + 1) * n];
        for c in 0..nv {
            dst[c] = if shade { shade_colour(base[c], curve[src[c] as usize]) } else { lut[src[c] as usize] };
        }
    }
    Some(Arc::new(Image::new(n as u32, nrows as u32, px)))
}

/// Contrast curve for spectrogram levels: a mild gamma lift and an S-curve, 0 and 255 fixed.
pub fn level_curve(v: u8) -> u8 {
    let u = (v as f32 / 255.0).powf(0.82);
    let t = ((u - 0.04) / 0.92).clamp(0.0, 1.0);
    let sc = t * t * (3.0 - 2.0 * t);
    ((u + (sc - u) * 0.7).clamp(0.0, 1.0) * 255.0).round() as u8
}

fn visible_columns(n: usize, f_min: f32, f_max: f32, sample_rate: f64) -> usize {
    if sample_rate > 0.0 {
        let nyq = (f_max as f64).min(sample_rate * 0.5);
        let nv = ((nyq / f_min as f64).ln() / ((f_max / f_min) as f64).ln() * n as f64).ceil() as i64;
        nv.clamp(2.min(n as i64), n as i64) as usize
    } else { n }
}

const DB_TOP: f32 = 0.0;
const DB_BOTTOM: f32 = -90.0;
const SCREEN_RADIUS: f32 = 4.0;

enum Gram<'a> { None, Rows(&'a [u8]), Img(&'a Arc<Image>) }

#[derive(Clone, PartialEq)]
struct GramKey { tick: u64, n: usize, rows: usize, map: i32, palette: SpectrumPalette, standard: u32, range: [u32; 7], f: (f32, f32), sr: f64 }

pub struct SpectrumView {
    pub settings: SpectrumSettings,
    analyser: Analyser,
    cols: Vec<f32>,
    hold_cols: Vec<f32>,
    scratch: Vec<f32>,
    prefix: Vec<f64>,
    smoothed: Vec<f32>,
    hist: SpectrogramHistory,
    gram: Option<(GramKey, Arc<Image>)>,
    pub tick: u64,
}
impl SpectrumView {
    pub fn new(settings: SpectrumSettings) -> Self {
        let mut v = Self { settings, analyser: Analyser::default(), cols: vec![], hold_cols: vec![], scratch: vec![], prefix: vec![], smoothed: vec![], hist: SpectrogramHistory::new(), gram: None, tick: 0 };
        v.apply();
        v
    }
    pub fn analyser(&self) -> &Analyser { &self.analyser }
    fn apply(&mut self) {
        let idx = self.settings.size_index.clamp(0, spectrum::FFT_SIZES.len() as i32 - 1) as usize;
        let size = spectrum::FFT_SIZES[idx] as usize;
        if self.analyser.fft_size() != size { self.analyser.configure(size, 48000.0); }
        let tilt = self.settings.tilt; let fall = self.settings.fall;
        self.analyser.set_tilt(tilt); self.analyser.set_fall(fall);
    }
    pub fn set_settings(&mut self, s: SpectrumSettings) { self.settings = s; self.apply(); }
    pub fn update(&mut self, ring: &SampleRing, seconds: f32, sample_rate: f64) {
        if !self.settings.on { return; }
        if sample_rate > 1000.0 && sample_rate != self.analyser.sample_rate() { self.analyser.set_sample_rate(sample_rate); }
        let _ = self.analyser.analyse(ring, 128);
        self.analyser.decay(seconds);
        self.tick += 1;
    }
    /// Paints the live analyser. The spectrogram advances one row per [`Self::update`]; a repaint
    /// with nothing new reuses the cached image (same `Image::id`, so host textures stay valid).
    pub fn paint(&mut self, scene: &mut Scene, plot: Rect, f_min: f32, f_max: f32) {
        if !self.settings.on || plot.w < 8.0 { return; }
        let n = (plot.w as i32).clamp(8, 2048) as usize;
        self.compute(n, f_min, f_max);
        let sr = self.analyser.sample_rate();
        if self.settings.style == SpectrumStyle::Spectrogram {
            let rows = (plot.h as i32).clamp(8, 256) as usize;
            let s = &self.settings;
            let key = GramKey { tick: self.tick, n, rows, map: s.spectrogram_map, palette: s.palette, standard: s.standard, range: s.range, f: (f_min, f_max), sr };
            let fresh = match &self.gram {
                Some((k, _)) => *k != key,
                None => true,
            };
            if fresh {
                if self.hist.columns() != n || self.hist.rows() != rows { self.hist.configure(n, rows); }
                let pushed = self.gram.as_ref().map_or(false, |(k, _)| k.tick == key.tick && k.n == n && k.rows == rows);
                if !pushed && !self.cols.is_empty() { self.hist.push(&self.cols, -90.0, -10.0); }
                let filled = self.hist.filled().min(rows);
                let mut flat = vec![0u8; n * rows];
                for r in 0..filled { flat[r * n..(r + 1) * n].copy_from_slice(self.hist.row(r)); }
                self.gram = spectrogram_image(&self.settings, &flat, n, f_min, f_max, sr).map(|img| (key, img));
            }
            if let Some((_, img)) = &self.gram {
                paint_core(scene, plot, &self.settings, &self.cols, None, Gram::Img(img), f_min, f_max, sr);
            }
        } else {
            paint_core(scene, plot, &self.settings, &self.cols, Some(&self.hold_cols), Gram::None, f_min, f_max, sr);
        }
    }
    fn compute(&mut self, n: usize, f_min: f32, f_max: f32) {
        self.cols.resize(n, FLOOR_DB);
        self.hold_cols.resize(n, FLOOR_DB);
        let db = self.analyser.display_db().to_vec();
        let hold = self.analyser.hold_db().to_vec();
        let sr = self.analyser.sample_rate();
        spectrum::to_columns(&db, sr, n, f_min, f_max, &mut self.cols);
        spectrum::to_columns(&hold, sr, n, f_min, f_max, &mut self.hold_cols);
        if self.settings.smoothing > 0 {
            self.scratch.resize(n, 0.0);
            self.smoothed.resize(n, 0.0);
            self.prefix.resize(n + 1, 0.0);
            let octaves = (f_max as f64 / f_min as f64).log2() as f32;
            let sm = self.settings.smoothing;
            let cols = self.cols.clone();
            spectrum::smooth_columns(&cols, &mut self.smoothed, octaves, sm, &mut self.scratch, &mut self.prefix);
            self.cols.copy_from_slice(&self.smoothed);
            let hold = self.hold_cols.clone();
            spectrum::smooth_columns(&hold, &mut self.smoothed, octaves, sm, &mut self.scratch, &mut self.prefix);
            self.hold_cols.copy_from_slice(&self.smoothed);
        }
    }
    /// Column / bar / line / spectrogram paint from already-reduced bins.
    /// `gram` is row-major levels, `columns.len()` wide, age 0 at the top. `sample_rate` 0 draws every column.
    /// The spectrogram image is rebuilt on every call; hosts that repaint an unchanged history
    /// should keep [`spectrogram_image`] and call [`Self::paint_gram_image`].
    pub fn paint_columns(
        scene: &mut Scene,
        plot: Rect,
        settings: &SpectrumSettings,
        columns: &[f32],
        hold: Option<&[f32]>,
        gram: Option<&[u8]>,
        f_min: f32,
        f_max: f32,
        sample_rate: f64,
    ) {
        let gram = match gram { Some(r) => Gram::Rows(r), None => Gram::None };
        paint_core(scene, plot, settings, columns, hold, gram, f_min, f_max, sample_rate);
    }
    /// Spectrogram look from an image the host keeps (see [`spectrogram_image`]), whatever
    /// `settings.style` says.
    pub fn paint_gram_image(scene: &mut Scene, plot: Rect, settings: &SpectrumSettings, image: &Arc<Image>, f_min: f32, f_max: f32) {
        if !settings.on || f_min <= 0.0 || f_max <= f_min { return; }
        let mut s = settings.clone();
        s.style = SpectrumStyle::Spectrogram;
        paint_core(scene, plot, &s, &[FLOOR_DB], None, Gram::Img(image), f_min, f_max, 0.0);
    }
    /// Only the recessed screen and graticule, for hosts that draw their own trace on top.
    /// Call [`Self::paint_screen_front`] afterwards for the glass and inner shadow.
    pub fn paint_screen_back(scene: &mut Scene, plot: Rect, settings: &SpectrumSettings, f_min: f32, f_max: f32) {
        if f_min <= 0.0 || f_max <= f_min { return; }
        if settings.frame { screen_back(scene, plot, settings); }
        if settings.grid {
            scene.clip_round(plot, if settings.frame { SCREEN_RADIUS } else { 0.0 });
            grid_lines(scene, plot, f_min, f_max, true, 1.0);
            if settings.labels { grid_labels(scene, plot, f_min, f_max, true, 0.5); }
            scene.pop_clip();
        }
    }
    pub fn paint_screen_front(scene: &mut Scene, plot: Rect, settings: &SpectrumSettings) {
        if settings.frame { screen_front(scene, plot); }
    }
}

fn y_norm(db: f32) -> f32 { ((db - DB_BOTTOM) / (DB_TOP - DB_BOTTOM)).clamp(0.0, 1.0) }

fn paint_core(scene: &mut Scene, plot: Rect, st: &SpectrumSettings, columns: &[f32], hold: Option<&[f32]>, gram: Gram, f_min: f32, f_max: f32, sample_rate: f64) {
    if !st.on || columns.is_empty() || f_min <= 0.0 || f_max <= f_min || !(plot.w > 0.0 && plot.h > 0.0) { return; }
    let n = columns.len();
    let alpha = st.opacity.clamp(0.0, 1.0);
    let nv = visible_columns(n, f_min, f_max, sample_rate);
    let is_gram = st.style == SpectrumStyle::Spectrogram;
    if st.frame { screen_back(scene, plot, st); }
    scene.clip_round(plot, if st.frame { SCREEN_RADIUS } else { 0.0 });
    if st.grid && !is_gram { grid_lines(scene, plot, f_min, f_max, true, 1.0); }
    match st.style {
        SpectrumStyle::Spectrogram => {
            let built;
            let img = match gram {
                Gram::Img(i) => Some(i),
                Gram::Rows(rows) => { built = spectrogram_image(st, rows, n, f_min, f_max, sample_rate); built.as_ref() }
                Gram::None => None,
            };
            if let Some(img) = img {
                scene.image(img, plot, alpha);
                if st.grid {
                    grid_lines(scene, plot, f_min, f_max, false, 0.9);
                    if st.labels {
                        let band = Rect::new(plot.x, plot.bottom() - 16.0, plot.w, 16.0);
                        scene.fill_rect(band, Color(0x00000000), Color(0x8c000000), true);
                    }
                }
            }
        }
        SpectrumStyle::Bars => paint_bars(scene, plot, st, columns, hold, nv, f_min, f_max),
        SpectrumStyle::Standard | SpectrumStyle::Multicolour => paint_trace(scene, plot, st, columns, hold, nv, f_min, f_max),
    }
    if st.grid && st.labels {
        let bars = st.style == SpectrumStyle::Bars;
        if bars {
            // The bars run to the floor: lay a dark band over their feet so the labels read.
            let band = Rect::new(plot.x, plot.bottom() - 17.0, plot.w, 17.0);
            scene.fill_shape(Shape::Round(band, 0.0), Paint::Linear { stops: Stops::new(&[(0.0, Color(0x00000000)), (0.45, Color(0xa0030506)), (1.0, Color(0xd2030506))]), angle: FRAC_PI_2 }, 0.0, Blend::Over);
        }
        grid_labels(scene, plot, f_min, f_max, !is_gram, if bars || is_gram { 0.82 } else { 0.5 });
    }
    scene.pop_clip();
    if st.frame { screen_front(scene, plot); }
}

// ---- screen ----------------------------------------------------------------------------------

fn screen_back(scene: &mut Scene, plot: Rect, st: &SpectrumSettings) {
    let r = SCREEN_RADIUS;
    let tint = st.trace_colour();
    // Bevel: the lip around the recess is dark on top (in shade) and catches light at the bottom.
    let lip = plot.expanded(1.25);
    scene.fill_round_linear(lip, r + 1.25, Stops::two(Color(0xd8000000), Color(0x8c000000)), FRAC_PI_2);
    let catch = Stops::new(&[(0.0, Color(0x00ffffff)), (0.9, Color(0x00ffffff)), (1.0, Color(0x38ffffff))]);
    scene.fill_round_linear(lip, r + 1.25, catch, FRAC_PI_2);
    let top = Color(0xff0c1215).mix(tint, 0.035);
    let bottom = Color(0xff05080a).mix(tint, 0.015);
    scene.fill_round_linear(plot, r, Stops::two(top, bottom), FRAC_PI_2);
    // Faint phosphor haze where the trace lives, then the vignette toward the corners.
    let c = plot.center();
    let haze = Stops::new(&[(0.0, tint.with_alpha(0.055)), (0.6, tint.with_alpha(0.02)), (1.0, tint.with_alpha(0.0))]);
    scene.fill_shape(Shape::Round(plot, r), Paint::Radial { stops: haze, center: Point::new(c.x, plot.y + plot.h * 0.62), rx: plot.w * 0.62, ry: plot.h * 0.75, focus: Point::new(c.x, plot.y + plot.h * 0.62) }, 0.0, Blend::Screen);
    vignette(scene, plot, 0.42);
}

fn vignette(scene: &mut Scene, plot: Rect, strength: f32) {
    let c = plot.center();
    let stops = Stops::new(&[(0.0, Color(0x00000000)), (0.55, Color(0x00000000)), (0.85, Color(0xff000000).with_alpha(strength * 0.45)), (1.0, Color(0xff000000).with_alpha(strength))]);
    scene.fill_shape(Shape::Round(plot, SCREEN_RADIUS), Paint::Radial { stops, center: c, rx: plot.w * 0.74, ry: plot.h * 0.86, focus: c }, 0.0, Blend::Over);
}

fn screen_front(scene: &mut Scene, plot: Rect) {
    let r = SCREEN_RADIUS;
    let shape = Shape::Round(plot, r);
    // Glass: a soft diagonal sheen from the top-left, and a thin bright line under the top lip.
    let sheen = Stops::new(&[(0.0, Color(0x0effffff)), (0.2, Color(0x06ffffff)), (0.4, Color(0x01ffffff)), (0.5, Color(0x00ffffff))]);
    scene.fill_shape(shape, Paint::Linear { stops: sheen, angle: 1.15 }, 0.0, Blend::Screen);
    let band = Rect::new(plot.x + plot.w * 0.04, plot.y - plot.h * 0.55, plot.w * 0.92, plot.h * 0.8);
    scene.clip_round(plot, r);
    scene.glow_ellipse(band, Color(0x06ffffff), plot.h * 0.18, Blend::Screen);
    scene.pop_clip();
    // Recess: wide soft shade from the top lip, a tight contact shadow all round.
    scene.inner_shadow(shape, Point::new(0.0, 3.0), 5.0, Color(0xc8000000));
    scene.inner_shadow(shape, Point::new(0.0, 0.6), 1.4, Color(0x99000000));
    scene.stroke_round(plot.reduced(0.25, 0.25), r, Color(0x70000000), 0.5);
}

// ---- graticule -------------------------------------------------------------------------------

const GRID_FREQS: [f32; 10] = [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0];
const GRID_INK: Color = Color(0xffb8cdd6);

fn freq_x(plot: Rect, f: f32, f_min: f32, f_max: f32) -> f32 { plot.x + (f / f_min).ln() / (f_max / f_min).ln() * plot.w }

fn db_step(plot: Rect) -> f32 {
    let per_db = plot.h / (DB_TOP - DB_BOTTOM);
    for s in [10.0, 20.0, 30.0] { if s * per_db >= 24.0 { return s; } }
    45.0
}

fn grid_lines(scene: &mut Scene, plot: Rect, f_min: f32, f_max: f32, db: bool, gain: f32) {
    let (top, bottom) = (plot.y, plot.bottom());
    let decade = plot.w / (f_max / f_min).log10();
    if decade >= 90.0 && db {
        let mut d = 10.0f32;
        while d < f_max {
            for m in [3.0, 4.0, 6.0, 7.0, 8.0, 9.0] {
                let x = freq_x(plot, d * m, f_min, f_max);
                if x > plot.x + 1.0 && x < plot.right() - 1.0 {
                    scene.line(Point::new(x, top), Point::new(x, bottom), GRID_INK.with_alpha(0.028 * gain), 0.5);
                }
            }
            d *= 10.0;
        }
    }
    for f in GRID_FREQS {
        let x = freq_x(plot, f, f_min, f_max);
        if x <= plot.x + 1.0 || x >= plot.right() - 1.0 { continue; }
        let decade_line = f == 100.0 || f == 1000.0 || f == 10000.0;
        let a = if decade_line { 0.105 } else { 0.065 } * gain;
        scene.line(Point::new(x, top), Point::new(x, bottom), GRID_INK.with_alpha(a), 0.5);
    }
    if db {
        let step = db_step(plot);
        let mut v = DB_TOP - step;
        while v > DB_BOTTOM + 0.5 {
            let y = (plot.bottom() - y_norm(v) * plot.h).round() + 0.25;
            scene.line(Point::new(plot.x, y), Point::new(plot.right(), y), GRID_INK.with_alpha(0.065 * gain), 0.5);
            v -= step;
        }
    }
}

fn grid_labels(scene: &mut Scene, plot: Rect, f_min: f32, f_max: f32, db: bool, ink_alpha: f32) {
    if plot.w < 120.0 || plot.h < 48.0 { return; }
    let size = if plot.h >= 260.0 && plot.w >= 520.0 { 8.5 } else { 7.0 };
    let ink = GRID_INK.with_alpha(ink_alpha);
    let y = plot.bottom() - size * 0.5 - 2.5;
    let mut last_end = f32::MIN;
    for f in GRID_FREQS {
        let x = freq_x(plot, f, f_min, f_max);
        if x < plot.x - 0.5 || x > plot.right() { continue; }
        let text = if f >= 1000.0 { format!("{}k", (f / 1000.0) as i32) } else { format!("{}", f as i32) };
        let width = text.len() as f32 * size * 0.56;
        let at = x.max(plot.x + 1.0) + 2.5;
        if at < last_end + 3.0 || at + width > plot.right() - 3.0 { continue; }
        last_end = at + width;
        scene.text(Point::new(at, y), text, ink, size, false);
    }
    if db {
        let step = db_step(plot);
        let mut v = DB_TOP - step;
        // The line nearest the floor is left unlabelled: a quiet trace rests there and would run
        // through the numeral. The others sit on a small dark chip so a trace passing behind
        // them never cuts the figures.
        while v > DB_BOTTOM + step * 1.25 {
            let ly = (plot.bottom() - y_norm(v) * plot.h).round() - size * 0.5 - 1.0;
            if ly > plot.y + size && ly < y - size {
                let text = format!("{}", v as i32);
                let at = Point::new(plot.right() - size * 1.35 - 2.0, ly);
                let chip = Rect::new(at.x - text.len() as f32 * size * 0.29 - 2.0, at.y - size * 0.55 - 0.5, text.len() as f32 * size * 0.58 + 4.0, size * 1.1 + 1.0);
                scene.fill_round(chip, 2.0, Color(0x73050809), Color(0x73050809), true);
                scene.text(at, text, GRID_INK.with_alpha((ink_alpha * 0.9).max(0.42)), size, true);
            }
            v -= step;
        }
    }
}

// ---- trace looks -----------------------------------------------------------------------------

/// Monotone cubic (Fritsch–Carlson, harmonic tangents) through equally spaced values: rounds the
/// polyline joints without overshooting a peak.
struct Curve<'a> { v: &'a [f32], m: Vec<f32> }
impl<'a> Curve<'a> {
    fn new(v: &'a [f32]) -> Self {
        let n = v.len();
        let mut m = vec![0.0f32; n];
        for k in 0..n {
            let d0 = if k > 0 { v[k] - v[k - 1] } else { 0.0 };
            let d1 = if k + 1 < n { v[k + 1] - v[k] } else { 0.0 };
            m[k] = if k == 0 { d1 } else if k + 1 == n { d0 } else if d0 * d1 <= 0.0 { 0.0 } else { 2.0 * d0 * d1 / (d0 + d1) };
        }
        Self { v, m }
    }
    /// `u` in column units, 0 on the first value.
    fn at(&self, u: f32) -> f32 {
        let n = self.v.len();
        if n == 1 || !(u > 0.0) { return self.v[0]; }
        if u >= (n - 1) as f32 { return self.v[n - 1]; }
        let k = u as usize;
        let t = u - k as f32;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.v[k] + (t3 - 2.0 * t2 + t) * self.m[k] + (-2.0 * t3 + 3.0 * t2) * self.v[k + 1] + (t3 - t2) * self.m[k + 1]
    }
}

/// Long polylines go out in short runs sharing an end point: same picture, and a distance-field
/// rasteriser only tests each pixel against the segments near it.
fn stroke_runs(scene: &mut Scene, pts: &[Point], color: Color, width: f32) {
    const RUN: usize = 20;
    let mut i = 0;
    while i + 1 < pts.len() {
        let end = (i + RUN).min(pts.len() - 1);
        scene.stroke_poly(pts[i..=end].to_vec(), color, width, false);
        i = end;
    }
}

fn paint_trace(scene: &mut Scene, plot: Rect, st: &SpectrumSettings, columns: &[f32], hold: Option<&[f32]>, nv: usize, f_min: f32, f_max: f32) {
    let n = columns.len();
    let alpha = st.opacity.clamp(0.0, 1.0);
    let multi = st.style == SpectrumStyle::Multicolour;
    let step = plot.w / n as f32;
    // The trace runs edge to edge, or stops on the last column below Nyquist.
    let x_end = if nv >= n { plot.w } else { (nv as f32 - 0.5) * step };
    if !(x_end > 0.0) { return; }
    let curve = Curve::new(&columns[..nv]);
    let colour_at_x = |x: f32| -> Color {
        if multi { let t = (x / plot.w).clamp(0.0, 1.0); st.ramp_colour(t, f_min * (f_max / f_min).powf(t)) } else { st.trace_colour() }
    };

    // Fill and bloom image. Two texels per scene unit while that stays cheap.
    let area = plot.w * plot.h;
    let res = if area * 4.0 <= 800_000.0 { 2.0 } else { (800_000.0 / area).sqrt().clamp(0.5, 2.0) };
    let (w, h) = (((plot.w * res).ceil() as usize).max(2), ((plot.h * res).ceil() as usize).max(2));
    let (sx, sy) = (w as f32 / plot.w, h as f32 / plot.h);
    let last = ((x_end * sx).ceil() as usize).min(w);
    let yc: Vec<f32> = (0..last).map(|i| (1.0 - y_norm(curve.at((i as f32 + 0.5) / sx / step - 0.5))) * h as f32).collect();
    let fill_stops = if multi {
        Stops::new(&[(0.0, Color(0xa6ffffff)), (0.3, Color(0x70ffffff)), (0.65, Color(0x30ffffff)), (1.0, Color(0x00ffffff))])
    } else {
        Stops::new(&[(0.0, Color(0x8cffffff)), (0.3, Color(0x5cffffff)), (0.65, Color(0x26ffffff)), (1.0, Color(0x00ffffff))])
    };
    // The gradient spans from the highest point of the trace to the floor, so a quiet signal
    // still shows its body; its strength follows that height.
    let crest = yc.iter().cloned().fold(h as f32, f32::min).max(0.0);
    let span = (h as f32 - crest).max(1.0);
    let body = 0.5 + 0.5 * (span / h as f32).sqrt();
    let row_fill: Vec<f32> = (0..h).map(|j| body * fill_stops.at((j as f32 + 0.5 - crest) / span).alpha_f()).collect();
    let bloom = st.bloom.clamp(0.0, 2.0);
    let sigma = 2.1 * sy;
    let skirt = 10.0 * sy;
    // Bloom falls off with the distance to the trace, taken as the nearest of the vertical
    // spans the trace covers in the neighbouring texel columns (so steep flanks glow evenly).
    let reach = (sigma * 3.4).ceil().max(1.0);
    let win = reach as usize;
    let mid = |a: usize, b: usize| (yc[a] + yc[b]) * 0.5;
    let lo: Vec<f32> = (0..last).map(|i| yc[i].min(mid(i, i.saturating_sub(1))).min(mid(i, (i + 1).min(last - 1)))).collect();
    let hi: Vec<f32> = (0..last).map(|i| yc[i].max(mid(i, i.saturating_sub(1))).max(mid(i, (i + 1).min(last - 1)))).collect();
    let mut px = vec![0u32; w * h];
    for i in 0..(last + win).min(w) {
        let inside = i < last;
        let col = colour_at_x((i as f32 + 0.5) / sx);
        let hot = col.brighter(0.65);
        let (cr, cg, cb) = (col.r() as f32, col.g() as f32, col.b() as f32);
        let (hr, hg, hb) = (hot.r() as f32, hot.g() as f32, hot.b() as f32);
        let y = yc[i.min(last - 1)];
        // A trace lying on the floor does not glow.
        let lift = ((h as f32 - y) / (5.0 * sy)).clamp(0.0, 1.0);
        // Below Nyquist the body fades out over a few units instead of ending on a wall.
        let edge = if !inside { 0.0 } else if nv >= n { 1.0 } else { ((x_end * sx - i as f32) / (8.0 * sx)).clamp(0.0, 1.0) };
        let (k0, k1) = (i.saturating_sub(win), (i + win).min(last - 1));
        let (mut top, mut bot) = (f32::MAX, f32::MIN);
        if bloom > 0.0 && lift > 0.0 && k0 <= k1 {
            for k in k0..=k1 { top = top.min(lo[k]); bot = bot.max(hi[k]); }
        }
        let g0 = (top - reach).floor().max(0.0);
        let g1 = bot + reach;
        let f0 = if inside { (y - 0.5).floor().max(0.0) } else { h as f32 };
        let j0 = g0.min(f0).min(h as f32) as usize;
        for j in j0..h {
            let yy = j as f32 + 0.5;
            let below = yy - y;
            let cov = (below + 0.5).clamp(0.0, 1.0);
            let af = if inside { cov * alpha * edge * (row_fill[j] + 0.20 * (-below.max(0.0) / skirt).exp() * lift) } else { 0.0 };
            let mut core = 0.0;
            let mut g = 0.0;
            if yy >= g0 && yy <= g1 {
                let mut best = reach * reach;
                for k in k0..=k1 {
                    let dx = k as f32 - i as f32;
                    let dx2 = dx * dx;
                    if dx2 >= best { continue; }
                    let dy = (lo[k] - yy).max(yy - hi[k]).max(0.0);
                    let d2 = dx2 + dy * dy;
                    if d2 < best { best = d2; }
                }
                if best < reach * reach {
                    let d = best.sqrt();
                    let fade = 1.0 - d / reach;
                    core = (-(d / sigma) * (d / sigma)).exp();
                    g = lift * bloom * alpha * (0.44 * core + 0.20 * (-d / (1.6 * sigma)).exp() * fade * fade);
                }
            } else if !inside && yy > g1 {
                break;
            }
            if af <= 0.002 && g <= 0.002 { continue; }
            let k = 0.55 * core;
            let ch = |c: f32, hc: f32| ((c * af + (c + (hc - c) * k) * g).round().min(255.0)) as u32;
            let a = (af * 255.0).round().min(255.0) as u32;
            px[j * w + i] = (a << 24) | (ch(cr, hr) << 16) | (ch(cg, hg) << 8) | ch(cb, hb);
        }
    }
    scene.image(&Arc::new(Image::new(w as u32, h as u32, px)), plot, 1.0);

    // Vector traces: peak hold first (dimmer, thinner), then the line with a hot core.
    let count = (x_end.ceil() as usize).max(2);
    let xs = |k: usize| x_end * k as f32 / count as f32;
    let poly = |c: &Curve| -> Vec<Point> {
        (0..=count).map(|k| Point::new(plot.x + xs(k), plot.bottom() - y_norm(c.at(xs(k) / step - 0.5)) * plot.h)).collect()
    };
    if st.peak_hold {
        if let Some(hd) = hold {
            let m = nv.min(hd.len());
            if m >= 2 {
                let hc = Curve::new(&hd[..m]);
                let ink = if multi { Color(0xffffffff) } else { st.trace_colour().brighter(0.55) };
                stroke_runs(scene, &poly(&hc), ink.with_alpha(0.28 * alpha), 0.7);
            }
        }
    }
    let pts = poly(&curve);
    if multi {
        const RUN: usize = 6;
        let mut i = 0;
        while i + 1 < pts.len() {
            let end = (i + RUN).min(pts.len() - 1);
            let col = colour_at_x(pts[(i + end) / 2].x - plot.x);
            scene.stroke_poly(pts[i..=end].to_vec(), col.with_alpha(alpha), 1.5, false);
            scene.stroke_poly(pts[i..=end].to_vec(), col.brighter(0.7).with_alpha(alpha * 0.8), 0.6, false);
            i = end;
        }
    } else {
        let col = st.trace_colour();
        stroke_runs(scene, &pts, col.with_alpha(alpha), 1.5);
        stroke_runs(scene, &pts, col.brighter(0.7).with_alpha(alpha * 0.8), 0.6);
    }
}

fn paint_bars(scene: &mut Scene, plot: Rect, st: &SpectrumSettings, columns: &[f32], hold: Option<&[f32]>, nv: usize, f_min: f32, f_max: f32) {
    let n = columns.len();
    let alpha = st.opacity.clamp(0.0, 1.0);
    let step = plot.w / n as f32;
    let group = ((6.5 / step).ceil() as usize).max(1);
    let pitch = step * group as f32;
    let gap = (pitch * 0.2).clamp(1.0, 2.5);
    let bw = (pitch - gap).max(0.5);
    let radius = (bw * 0.22).min(1.5);
    let bloom = st.bloom.clamp(0.0, 2.0);
    let mut c = 0;
    while c + group <= nv {
        let mut m = FLOOR_DB;
        let mut pk = FLOOR_DB;
        for k in 0..group {
            m = m.max(columns[c + k]);
            if let Some(h) = hold { if c + k < h.len() { pk = pk.max(h[c + k]); } }
        }
        let t = (c as f32 + group as f32 * 0.5) / n as f32;
        let col = st.ramp_colour(t, f_min * (f_max / f_min).powf(t));
        let x = plot.x + c as f32 * step + gap * 0.5;
        // Unlit track, so the bar grid reads even where the signal is at the floor.
        scene.fill_rect(Rect::new(x, plot.y, bw, plot.h), col.with_alpha(0.035), col.with_alpha(0.06), true);
        let level = y_norm(m);
        if m > FLOOR_DB + 1.0 && level > 0.0 {
            let y = plot.bottom() - level * plot.h;
            let bar = Rect::new(x, y, bw, plot.bottom() - y + radius);
            let stops = Stops::new(&[
                (0.0, col.brighter(0.45).with_alpha(alpha)),
                (0.08, col.with_alpha(alpha)),
                (0.55, col.darker(0.25).with_alpha(alpha * 0.8)),
                (1.0, col.darker(0.6).with_alpha(alpha * 0.4)),
            ]);
            scene.fill_round_linear(bar, radius, stops, FRAC_PI_2);
            // Lit edge down the left side of each bar.
            scene.fill_rect(Rect::new(x, y + radius, (bw * 0.18).max(0.5), (plot.bottom() - y - radius).max(0.0)), Color(0xffffffff).with_alpha(0.16 * alpha), Color(0xffffffff).with_alpha(0.0), true);
            if bloom > 0.0 {
                let reach = 5.0 + 6.0 * level;
                let halo = Rect::new(x - reach, y - reach, bw + reach * 2.0, reach * 2.6);
                scene.glow_round(halo, reach, col.with_alpha((0.10 + 0.34 * level) * bloom.min(1.5) * alpha), reach, Blend::Add);
            }
        }
        if st.peak_hold && hold.is_some() && pk > FLOOR_DB + 1.0 && y_norm(pk) > 0.0 {
            let y = plot.bottom() - y_norm(pk) * plot.h;
            let cap = col.brighter(0.5);
            if bloom > 0.0 { scene.glow_round(Rect::new(x - 2.5, y - 3.5, bw + 5.0, 7.0), 3.5, cap.with_alpha(0.22 * bloom.min(1.5) * alpha), 3.0, Blend::Add); }
            scene.fill_rect(Rect::new(x, y - 0.75, bw, 1.25), cap.with_alpha(alpha), cap.with_alpha(alpha), true);
        }
        c += group;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Cmd;

    fn demo() -> (Vec<f32>, Vec<f32>, Vec<u8>) {
        let n = 64;
        let cols: Vec<f32> = (0..n).map(|i| -80.0 + 50.0 * (-((i as f32 - 20.0) / 6.0).powi(2)).exp()).collect();
        let hold: Vec<f32> = cols.iter().map(|v| v + 6.0).collect();
        let gram: Vec<u8> = (0..n * 8).map(|i| (i % 251) as u8).collect();
        (cols, hold, gram)
    }

    #[test]
    fn extended_settings_round_trip_and_old_strings_load() {
        let mut s = SpectrumSettings::default();
        assert_eq!(s.to_string().split(',').count(), 18);
        s.palette = SpectrumPalette::Amber;
        s.grid = false;
        s.bloom = 0.5;
        s.spectrogram_map = MAP_ICE;
        let back = SpectrumSettings::from_string(&s.to_string());
        assert_eq!(back.palette, SpectrumPalette::Amber);
        assert!(back.frame && !back.grid && back.labels);
        assert_eq!(back.bloom, 0.5);
        assert_eq!(back.spectrogram_map, MAP_ICE);
        let old = "1,3,multi,1,3,24,0.85,1,ff8fe0c0,ff112233,ffff9a1f,ffffe23a,ff5be85a,ff35e0d0,ff3f8ff0,ffc05cff,0,0";
        assert_eq!(SpectrumSettings::from_string(old).palette, SpectrumPalette::Range);
    }

    #[test]
    fn every_style_and_palette_paints_with_balanced_clips() {
        let (cols, hold, gram) = demo();
        for style in [SpectrumStyle::Standard, SpectrumStyle::Multicolour, SpectrumStyle::Bars, SpectrumStyle::Spectrogram] {
            for palette in SpectrumPalette::ALL {
                for frame in [true, false] {
                    let mut st = SpectrumSettings::default();
                    st.style = style;
                    st.palette = palette;
                    st.frame = frame;
                    let mut scene = Scene::new();
                    SpectrumView::paint_columns(&mut scene, Rect::new(10.0, 10.0, 300.0, 120.0), &st, &cols, Some(&hold), Some(&gram), 20.0, 20000.0, 48000.0);
                    assert!(!scene.cmds.is_empty());
                    let push = scene.cmds.iter().filter(|c| matches!(c, Cmd::ClipRound { .. })).count();
                    let pop = scene.cmds.iter().filter(|c| matches!(c, Cmd::PopClip)).count();
                    assert_eq!(push, pop);
                    assert!(!scene.lower_basic().cmds.is_empty());
                }
            }
        }
    }

    #[test]
    fn spectrogram_is_one_image_and_cached_until_the_next_update() {
        let (cols, _, gram) = demo();
        let mut st = SpectrumSettings::default();
        st.style = SpectrumStyle::Spectrogram;
        let mut scene = Scene::new();
        SpectrumView::paint_columns(&mut scene, Rect::new(0.0, 0.0, 256.0, 96.0), &st, &cols, None, Some(&gram), 20.0, 20000.0, 0.0);
        let images: Vec<&Arc<Image>> = scene.cmds.iter().filter_map(|c| if let Cmd::Image { img, .. } = c { Some(img) } else { None }).collect();
        assert_eq!(images.len(), 1);
        assert_eq!((images[0].w, images[0].h), (64, 8));
        assert!(scene.cmds.len() < 60, "{} commands", scene.cmds.len());

        let mut view = SpectrumView::new(st);
        let id = |view: &mut SpectrumView| {
            let mut scene = Scene::new();
            view.paint(&mut scene, Rect::new(0.0, 0.0, 200.0, 80.0), 20.0, 20000.0);
            scene.cmds.iter().find_map(|c| if let Cmd::Image { img, .. } = c { Some(img.id) } else { None }).unwrap()
        };
        let a = id(&mut view);
        assert_eq!(a, id(&mut view));
        view.tick += 1;
        assert_ne!(a, id(&mut view));
    }

    #[test]
    fn colour_maps_rise_in_brightness() {
        for map in [MAP_INFERNO, MAP_MAGMA, MAP_ICE] {
            let mut last = -1.0;
            for v in (0..=255).step_by(15) {
                let b = Color(colour_map(map, v as u8)).perceived_brightness();
                assert!(b > last, "map {map} at {v}");
                last = b;
            }
        }
    }

    #[test]
    fn curve_passes_through_values_without_overshoot() {
        let v = [0.0, 0.0, 10.0, 0.0, 0.0, 5.0, 5.0];
        let c = Curve::new(&v);
        for (i, x) in v.iter().enumerate() { assert!((c.at(i as f32) - x).abs() < 1e-5); }
        for k in 0..600 { let y = c.at(k as f32 * 0.01); assert!((-1e-4..=10.0001).contains(&y)); }
    }
}
