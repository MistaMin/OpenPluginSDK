//! Small neural networks for amp and pedal models.
//!
//! A [`Model`] chains [`Dense`], [`Gru`], [`Lstm`] and [`Conv1d`] layers and
//! runs them one audio sample at a time. Weights come from a documented JSON
//! file ([`Model::from_json`], format below), parsed and allocated off the
//! audio thread; [`Model::process_sample`] and [`Model::process_block`] never
//! allocate, lock or panic.
//!
//! # Equations
//!
//! Notation: `σ` is the logistic sigmoid, `⊙` the elementwise product, `x` the
//! layer input, `h` the hidden state, `W` input weights, `U` recurrent
//! weights.
//!
//! * **Dense**: `y = f(W x + b)` with `f` identity, tanh, ReLU or sigmoid.
//! * **GRU** (Cho et al. 2014, "Learning Phrase Representations using RNN
//!   Encoder-Decoder", eqs. 5-8; the reset gate multiplies the state before
//!   the recurrent matrix, as in that paper):
//!   `z = σ(Wz x + Uz h + bz)`, `r = σ(Wr x + Ur h + br)`,
//!   `n = tanh(Wn x + Un (r ⊙ h) + bn)`, `h' = z ⊙ h + (1 - z) ⊙ n`.
//!   (Frameworks that apply `r` after the recurrent product, such as the
//!   cuDNN variant, compute a different function: convert before export.)
//! * **LSTM** (Hochreiter & Schmidhuber 1997 with the forget gate of Gers,
//!   Schmidhuber & Cummins 2000, no peepholes):
//!   `i = σ(Wi x + Ui h + bi)`, `f = σ(Wf x + Uf h + bf)`,
//!   `g = tanh(Wg x + Ug h + bg)`, `o = σ(Wo x + Uo h + bo)`,
//!   `c' = f ⊙ c + i ⊙ g`, `h' = o ⊙ tanh(c')`.
//! * **Conv1d** (causal, dilated; van den Oord et al. 2016, "WaveNet"):
//!   `y[t] = f(b + Σ_j W_j · x[t - j·dilation])`, `j = 0..kernel`. Inputs
//!   before the start of the signal (and after a reset) are zero. This is the
//!   convolution of a WaveNet stack; the gated activation and the
//!   residual/skip wiring *inside* a WaveNet block are not modelled (a gated
//!   block is not a chain of layers). The model-level residual below is.
//!
//! # JSON format (version 1)
//!
//! ```json
//! {
//!   "version": 1,
//!   "sample_rate": 48000,
//!   "input_size": 1,
//!   "residual": false,
//!   "layers": [
//!     { "type": "lstm", "in": 1, "hidden": 2,
//!       "w_x": [...], "w_h": [...], "b": [...] },
//!     { "type": "dense", "in": 2, "out": 1, "activation": "identity",
//!       "weights": [...], "bias": [...] }
//!   ]
//! }
//! ```
//!
//! All matrices are flat arrays of numbers in **row-major** order (one row per
//! output unit; the row is contiguous). Unknown fields, missing fields,
//! non-finite numbers and wrong lengths are errors.
//!
//! | `type` | fields | array lengths (row order) |
//! | --- | --- | --- |
//! | `dense` | `in`, `out`, `activation` (`identity`, `tanh`, `relu`, `sigmoid`), `weights`, `bias` | `weights`: `out x in`; `bias`: `out` |
//! | `gru` | `in`, `hidden`, `w_x`, `w_h`, `b` | `w_x`: `3h x in`; `w_h`: `3h x h`; `b`: `3h`. Row blocks in the order z, r, n |
//! | `lstm` | `in`, `hidden`, `w_x`, `w_h`, `b` | `w_x`: `4h x in`; `w_h`: `4h x h`; `b`: `4h`. Row blocks in the order i, f, g, o |
//! | `conv1d` | `in`, `out`, `kernel`, `dilation`, `activation`, `weights`, `bias` | `weights`: `out x kernel x in` (per output, tap 0 first; tap 0 is the current sample); `bias`: `out` |
//!
//! Model fields: `version` (must be 1), `sample_rate` (Hz the model was
//! trained at, 1000 to 768000), `input_size` (1 = audio, 2 = audio then a
//! conditioning value such as a gain knob), `residual` (optional, default
//! false: add the audio input to the output, `y = net(x) + x`), `layers`
//! (at least one; sizes must chain; the last layer outputs exactly 1 value).
//! Limits: every size at most 512, a conv1d kernel at most 64 and dilation at
//! most 4096, at most 4 million weights in total, the text at most 64 MiB.
//! Exporters should write `f32` values with the shortest round-trip text.
//!
//! # Sample rate
//!
//! A model is trained on audio at one rate ([`Model::sample_rate`]) and its
//! memory (recurrent time constants, conv dilation) is measured in samples, so
//! it sounds different at another rate. [`Model::rate_ratio`] gives
//! `host / trained`. If it is not 1, run the model at its own rate: resample
//! the input to [`Model::sample_rate`] (for integer ratios use
//! [`crate::LinearPhaseOversampler`]; otherwise a windowed-sinc resampler such
//! as [`crate::SincKernel`]), run the model, and resample back. The model
//! itself never resamples, so the latency of that resampling is the caller's
//! to report.
//!
//! # Handing a model to the audio thread
//!
//! ```
//! use openplugin_core::worker::loader;
//! use openplugin_dsp::neural::Model;
//!
//! let json = r#"{"version":1,"sample_rate":48000,"input_size":1,"layers":[
//!   {"type":"dense","in":1,"out":1,"activation":"identity","weights":[0.5],"bias":[0.0]}]}"#;
//! // The audio side starts with no model; the worker thread builds the real one.
//! let (mut loader, mut slot) = loader::<Option<Model>>(None, 2);
//! let text = json.to_string();
//! loader.submit(move || Model::from_json(&text).ok()).unwrap();
//! loader.wait_idle(std::time::Duration::from_secs(5));
//! // Top of `process`:
//! slot.poll();
//! if let Some(model) = slot.get_mut() {
//!     assert_eq!(model.process_sample(1.0, 0.0), 0.5);
//! }
//! ```
//!
//! The replaced model is dropped on the worker thread. A new model starts
//! with cleared state.
//!
//! # Cost and denormals
//!
//! Matrix-vector products use contiguous rows and eight independent
//! accumulators so the compiler can vectorise them (`docs/PERFORMANCE.md`).
//! Recurrent states and the output are flushed to zero below 1e-30, so
//! decaying states never reach denormals.
//!
//! A model with non-zero biases outputs a (usually tiny) DC level for silent
//! input, as the trained amp does; zero biases give silence for silence.

use std::fmt;

/// Largest layer width accepted.
pub const MAX_SIZE: usize = 512;
/// Largest total number of weights accepted.
pub const MAX_WEIGHTS: usize = 4_000_000;
/// Largest conv1d kernel.
pub const MAX_KERNEL: usize = 64;
/// Largest conv1d dilation.
pub const MAX_DILATION: usize = 4096;
/// Largest JSON text [`Model::from_json`] reads, in bytes.
pub const MAX_JSON_BYTES: usize = 64 << 20;

/// Why a model could not be built or loaded.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// The text is not valid JSON, or too large.
    Json(String),
    /// A field is missing, of the wrong type, unknown, or out of range.
    Field(String),
    /// An array has the wrong number of elements.
    Length {
        /// Which array.
        what: String,
        /// The count its sizes require.
        expected: usize,
        /// The count in the file.
        found: usize,
    },
    /// A number is NaN or infinite (or too large for `f32`).
    NotFinite(String),
    /// A size is zero or over a limit.
    Size(String),
    /// Adjacent layers (or the model ends) do not fit together.
    Chain(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(s) => write!(f, "invalid JSON: {s}"),
            Self::Field(s) => write!(f, "bad field: {s}"),
            Self::Length { what, expected, found } => {
                write!(f, "{what} has {found} values, expected {expected}")
            }
            Self::NotFinite(s) => write!(f, "{s} contains a non-finite number"),
            Self::Size(s) => write!(f, "bad size: {s}"),
            Self::Chain(s) => write!(f, "layers do not chain: {s}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// Activation function of a [`Dense`] or [`Conv1d`] layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// `x`
    Identity,
    /// `tanh(x)`
    Tanh,
    /// `max(x, 0)`
    Relu,
    /// `1 / (1 + e^-x)`
    Sigmoid,
}

impl Activation {
    fn parse(s: &str) -> Result<Self, ModelError> {
        match s {
            "identity" | "linear" => Ok(Self::Identity),
            "tanh" => Ok(Self::Tanh),
            "relu" => Ok(Self::Relu),
            "sigmoid" => Ok(Self::Sigmoid),
            other => Err(ModelError::Field(format!("unknown activation \"{other}\""))),
        }
    }

    /// Apply to a slice in place.
    #[inline]
    pub fn apply(self, v: &mut [f32]) {
        self.apply_with(v, false);
    }

    #[inline]
    fn apply_with(self, v: &mut [f32], fast: bool) {
        match self {
            Self::Identity => {}
            Self::Tanh => v.iter_mut().for_each(|x| *x = tanh_of(fast, *x)),
            Self::Relu => v.iter_mut().for_each(|x| *x = x.max(0.0)),
            Self::Sigmoid => v.iter_mut().for_each(|x| *x = sigmoid_of(fast, *x)),
        }
    }
}

#[inline(always)]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Padé [7/6] approximation of tanh (Lambert's continued fraction truncated),
/// input clamped to ±4.97 where it reaches ±1. Maximum absolute error about
/// 1e-4 (at the clamp), 4 to 10 times faster than `f32::tanh`.
#[inline(always)]
fn tanh_fast(x: f32) -> f32 {
    let x = x.clamp(-4.97, 4.97);
    let x2 = x * x;
    let num = x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2)));
    let den = 135135.0 + x2 * (62370.0 + x2 * (3150.0 + x2 * 28.0));
    num / den
}

/// Zero values too small to matter (below 1e-30), so state never goes denormal.
#[inline(always)]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-30 {
        0.0
    } else {
        x
    }
}

#[inline(always)]
fn tanh_of(fast: bool, x: f32) -> f32 {
    if fast {
        tanh_fast(x)
    } else {
        x.tanh()
    }
}

#[inline(always)]
fn sigmoid_of(fast: bool, x: f32) -> f32 {
    if fast {
        0.5 + 0.5 * tanh_fast(0.5 * x)
    } else {
        sigmoid(x)
    }
}

/// Dot product with eight independent accumulators (vectorises to SIMD).
#[inline(always)]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut acc = [0.0f32; 8];
    let mut ca = a.chunks_exact(8);
    let mut cb = b.chunks_exact(8);
    for (x, y) in (&mut ca).zip(&mut cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let mut tail = 0.0;
    for (x, y) in ca.remainder().iter().zip(cb.remainder()) {
        tail += x * y;
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7])) + tail
}

/// A weight matrix stored transposed (column after column) so that a
/// matrix-vector product is a sequence of contiguous `out += w_col * x[c]`
/// passes: pure vertical multiply-adds, no horizontal reductions.
#[derive(Debug, Clone)]
struct Mat {
    rows: usize,
    cols: usize,
    /// `cols` runs of `rows` values.
    t: Vec<f32>,
}

impl Mat {
    /// From row-major `rows x cols` data (length already checked).
    fn new(data: Vec<f32>, rows: usize, cols: usize) -> Self {
        let mut t = vec![0.0; data.len()];
        for r in 0..rows {
            for c in 0..cols {
                t[c * rows + r] = data[r * cols + c];
            }
        }
        Self { rows, cols, t }
    }

    fn len(&self) -> usize {
        self.t.len()
    }

    /// `out[k] += sum_c W[r0 + k][c] * x[c]` for rows `r0..r1`.
    #[inline(always)]
    fn add_rows(&self, r0: usize, r1: usize, x: &[f32], out: &mut [f32]) {
        let out = &mut out[..r1 - r0];
        for (col, &xc) in self.t.chunks_exact(self.rows).zip(&x[..self.cols]) {
            for (o, w) in out.iter_mut().zip(&col[r0..r1]) {
                *o += w * xc;
            }
        }
    }

    #[inline(always)]
    fn add(&self, x: &[f32], out: &mut [f32]) {
        self.add_rows(0, self.rows, x, out);
    }
}

fn check_size(what: &str, n: usize) -> Result<(), ModelError> {
    if n == 0 || n > MAX_SIZE {
        return Err(ModelError::Size(format!("{what} = {n} (must be 1..={MAX_SIZE})")));
    }
    Ok(())
}

fn check_data(what: &str, v: &[f32], expected: usize) -> Result<(), ModelError> {
    if v.len() != expected {
        return Err(ModelError::Length { what: what.into(), expected, found: v.len() });
    }
    if v.iter().any(|x| !x.is_finite()) {
        return Err(ModelError::NotFinite(what.into()));
    }
    Ok(())
}

fn mul(a: usize, b: usize) -> Result<usize, ModelError> {
    a.checked_mul(b)
        .filter(|&n| n <= MAX_WEIGHTS)
        .ok_or_else(|| ModelError::Size("too many weights".into()))
}

/// Fully connected layer, `y = f(W x + b)`.
#[derive(Debug, Clone)]
pub struct Dense {
    inputs: usize,
    outputs: usize,
    act: Activation,
    w: Mat,
    b: Vec<f32>,
}

impl Dense {
    /// `weights` is `outputs x inputs`, row-major; `bias` has `outputs` values.
    pub fn new(
        inputs: usize,
        outputs: usize,
        act: Activation,
        weights: Vec<f32>,
        bias: Vec<f32>,
    ) -> Result<Self, ModelError> {
        check_size("dense in", inputs)?;
        check_size("dense out", outputs)?;
        check_data("dense weights", &weights, mul(inputs, outputs)?)?;
        check_data("dense bias", &bias, outputs)?;
        Ok(Self { inputs, outputs, act, w: Mat::new(weights, outputs, inputs), b: bias })
    }

    #[inline]
    fn process(&mut self, x: &[f32], y: &mut [f32], fast: bool) {
        let y = &mut y[..self.outputs];
        y.copy_from_slice(&self.b);
        self.w.add(&x[..self.inputs], y);
        self.act.apply_with(y, fast);
    }
}

/// Gated recurrent unit (Cho et al. 2014). See the module docs.
#[derive(Debug, Clone)]
pub struct Gru {
    inputs: usize,
    hidden: usize,
    wx: Mat,
    wh: Mat,
    b: Vec<f32>,
    h: Vec<f32>,
    pre: Vec<f32>,
    rh: Vec<f32>,
}

impl Gru {
    /// `w_x`: `3h x inputs`, `w_h`: `3h x h`, `b`: `3h`; blocks z, r, n.
    pub fn new(
        inputs: usize,
        hidden: usize,
        w_x: Vec<f32>,
        w_h: Vec<f32>,
        b: Vec<f32>,
    ) -> Result<Self, ModelError> {
        check_size("gru in", inputs)?;
        check_size("gru hidden", hidden)?;
        check_data("gru w_x", &w_x, mul(3 * hidden, inputs)?)?;
        check_data("gru w_h", &w_h, mul(3 * hidden, hidden)?)?;
        check_data("gru b", &b, 3 * hidden)?;
        Ok(Self {
            inputs,
            hidden,
            wx: Mat::new(w_x, 3 * hidden, inputs),
            wh: Mat::new(w_h, 3 * hidden, hidden),
            b,
            h: vec![0.0; hidden],
            pre: vec![0.0; 3 * hidden],
            rh: vec![0.0; hidden],
        })
    }

    #[inline]
    fn process(&mut self, x: &[f32], y: &mut [f32], fast: bool) {
        let h = self.hidden;
        let x = &x[..self.inputs];
        self.pre.copy_from_slice(&self.b);
        // z and r: rows 0..2h see the input and the previous state.
        self.wx.add_rows(0, 2 * h, x, &mut self.pre[..2 * h]);
        self.wh.add_rows(0, 2 * h, &self.h, &mut self.pre[..2 * h]);
        for k in 0..2 * h {
            self.pre[k] = sigmoid_of(fast, self.pre[k]);
        }
        for k in 0..h {
            self.rh[k] = self.pre[h + k] * self.h[k];
        }
        // Candidate: input part plus U (r . h).
        let (zr, n) = self.pre.split_at_mut(2 * h);
        self.wx.add_rows(2 * h, 3 * h, x, n);
        self.wh.add_rows(2 * h, 3 * h, &self.rh, n);
        for k in 0..h {
            let z = zr[k];
            self.h[k] = flush(z * self.h[k] + (1.0 - z) * tanh_of(fast, n[k]));
        }
        y[..h].copy_from_slice(&self.h);
    }

    fn reset(&mut self) {
        self.h.fill(0.0);
    }
}

/// Long short-term memory with forget gate, no peepholes. See the module docs.
#[derive(Debug, Clone)]
pub struct Lstm {
    inputs: usize,
    hidden: usize,
    wx: Mat,
    wh: Mat,
    b: Vec<f32>,
    h: Vec<f32>,
    c: Vec<f32>,
    pre: Vec<f32>,
}

impl Lstm {
    /// `w_x`: `4h x inputs`, `w_h`: `4h x h`, `b`: `4h`; blocks i, f, g, o.
    pub fn new(
        inputs: usize,
        hidden: usize,
        w_x: Vec<f32>,
        w_h: Vec<f32>,
        b: Vec<f32>,
    ) -> Result<Self, ModelError> {
        check_size("lstm in", inputs)?;
        check_size("lstm hidden", hidden)?;
        check_data("lstm w_x", &w_x, mul(4 * hidden, inputs)?)?;
        check_data("lstm w_h", &w_h, mul(4 * hidden, hidden)?)?;
        check_data("lstm b", &b, 4 * hidden)?;
        Ok(Self {
            inputs,
            hidden,
            wx: Mat::new(w_x, 4 * hidden, inputs),
            wh: Mat::new(w_h, 4 * hidden, hidden),
            b,
            h: vec![0.0; hidden],
            c: vec![0.0; hidden],
            pre: vec![0.0; 4 * hidden],
        })
    }

    #[inline]
    fn process(&mut self, x: &[f32], y: &mut [f32], fast: bool) {
        let h = self.hidden;
        self.pre.copy_from_slice(&self.b);
        self.wx.add(&x[..self.inputs], &mut self.pre);
        self.wh.add(&self.h, &mut self.pre);
        let (i, rest) = self.pre.split_at_mut(h);
        let (f, rest) = rest.split_at_mut(h);
        let (g, o) = rest.split_at_mut(h);
        for k in 0..h {
            let c = sigmoid_of(fast, f[k]) * self.c[k]
                + sigmoid_of(fast, i[k]) * tanh_of(fast, g[k]);
            self.c[k] = flush(c);
            self.h[k] = flush(sigmoid_of(fast, o[k]) * tanh_of(fast, c));
        }
        y[..h].copy_from_slice(&self.h);
    }

    fn reset(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
    }
}

/// Causal dilated 1-D convolution over time, `y[t] = f(b + Σ W_j x[t-j·d])`.
#[derive(Debug, Clone)]
pub struct Conv1d {
    inputs: usize,
    outputs: usize,
    kernel: usize,
    dilation: usize,
    act: Activation,
    w: Vec<f32>,
    b: Vec<f32>,
    /// Ring of past input frames, `cap` frames of `inputs` values.
    hist: Vec<f32>,
    cap: usize,
    pos: usize,
}

impl Conv1d {
    /// `weights`: `outputs x kernel x inputs`, tap 0 (the current sample) first.
    pub fn new(
        inputs: usize,
        outputs: usize,
        kernel: usize,
        dilation: usize,
        act: Activation,
        weights: Vec<f32>,
        bias: Vec<f32>,
    ) -> Result<Self, ModelError> {
        check_size("conv1d in", inputs)?;
        check_size("conv1d out", outputs)?;
        if kernel == 0 || kernel > MAX_KERNEL {
            return Err(ModelError::Size(format!("conv1d kernel = {kernel} (1..={MAX_KERNEL})")));
        }
        if dilation == 0 || dilation > MAX_DILATION {
            return Err(ModelError::Size(format!(
                "conv1d dilation = {dilation} (1..={MAX_DILATION})"
            )));
        }
        let taps = mul(kernel, inputs)?;
        check_data("conv1d weights", &weights, mul(taps, outputs)?)?;
        check_data("conv1d bias", &bias, outputs)?;
        let cap = (kernel - 1) * dilation + 1;
        let hist = vec![0.0; mul(cap, inputs)?];
        Ok(Self { inputs, outputs, kernel, dilation, act, w: weights, b: bias, hist, cap, pos: 0 })
    }

    /// Samples of past input the layer looks at, `(kernel - 1) * dilation`.
    pub fn receptive_past(&self) -> usize {
        self.cap - 1
    }

    #[inline]
    fn process(&mut self, x: &[f32], y: &mut [f32], fast: bool) {
        let n = self.inputs;
        self.pos = if self.pos + 1 == self.cap { 0 } else { self.pos + 1 };
        self.hist[self.pos * n..(self.pos + 1) * n].copy_from_slice(&x[..n]);
        let y = &mut y[..self.outputs];
        y.copy_from_slice(&self.b);
        let row = self.kernel * n;
        for j in 0..self.kernel {
            let back = j * self.dilation;
            let idx = if back <= self.pos { self.pos - back } else { self.pos + self.cap - back };
            let frame = &self.hist[idx * n..(idx + 1) * n];
            for (o, yo) in y.iter_mut().enumerate() {
                *yo += dot(&self.w[o * row + j * n..o * row + (j + 1) * n], frame);
            }
        }
        self.act.apply_with(y, fast);
    }

    fn reset(&mut self) {
        self.hist.fill(0.0);
        self.pos = 0;
    }
}

/// One layer of a [`Model`].
#[derive(Debug, Clone)]
pub enum Layer {
    /// Fully connected.
    Dense(Dense),
    /// Gated recurrent unit.
    Gru(Gru),
    /// Long short-term memory.
    Lstm(Lstm),
    /// Causal dilated convolution.
    Conv1d(Conv1d),
}

impl Layer {
    fn sizes(&self) -> (usize, usize) {
        match self {
            Self::Dense(l) => (l.inputs, l.outputs),
            Self::Gru(l) => (l.inputs, l.hidden),
            Self::Lstm(l) => (l.inputs, l.hidden),
            Self::Conv1d(l) => (l.inputs, l.outputs),
        }
    }

    fn weight_count(&self) -> usize {
        match self {
            Self::Dense(l) => l.w.len() + l.b.len(),
            Self::Gru(l) => l.wx.len() + l.wh.len() + l.b.len(),
            Self::Lstm(l) => l.wx.len() + l.wh.len() + l.b.len(),
            Self::Conv1d(l) => l.w.len() + l.b.len(),
        }
    }

    #[inline]
    fn process(&mut self, x: &[f32], y: &mut [f32], fast: bool) {
        match self {
            Self::Dense(l) => l.process(x, y, fast),
            Self::Gru(l) => l.process(x, y, fast),
            Self::Lstm(l) => l.process(x, y, fast),
            Self::Conv1d(l) => l.process(x, y, fast),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Dense(_) => {}
            Self::Gru(l) => l.reset(),
            Self::Lstm(l) => l.reset(),
            Self::Conv1d(l) => l.reset(),
        }
    }
}

/// A chain of layers run one sample at a time: input size 1 (audio) or 2
/// (audio and a conditioning value), output size 1.
#[derive(Debug, Clone)]
pub struct Model {
    layers: Vec<Layer>,
    input_size: usize,
    sample_rate: f32,
    residual: bool,
    fast: bool,
    a: Vec<f32>,
    b: Vec<f32>,
}

impl Model {
    /// Build from layers. The first layer must take `input_size` values, each
    /// layer's output must match the next layer's input, and the last layer
    /// must output exactly one value.
    pub fn new(
        input_size: usize,
        sample_rate: f32,
        residual: bool,
        layers: Vec<Layer>,
    ) -> Result<Self, ModelError> {
        if input_size != 1 && input_size != 2 {
            return Err(ModelError::Size(format!("input_size = {input_size} (must be 1 or 2)")));
        }
        if !(1000.0..=768_000.0).contains(&sample_rate) {
            return Err(ModelError::Field(format!(
                "sample_rate = {sample_rate} (must be 1000..=768000)"
            )));
        }
        if layers.is_empty() {
            return Err(ModelError::Chain("no layers".into()));
        }
        let mut width = input_size;
        for (k, l) in layers.iter().enumerate() {
            let (i, o) = l.sizes();
            if i != width {
                return Err(ModelError::Chain(format!(
                    "layer {k} takes {i} values, the previous stage gives {width}"
                )));
            }
            width = o;
        }
        if width != 1 {
            return Err(ModelError::Chain(format!("last layer gives {width} values, need 1")));
        }
        let max = layers.iter().map(|l| l.sizes().1).max().unwrap_or(1).max(input_size);
        Ok(Self {
            layers,
            input_size,
            sample_rate,
            residual,
            fast: false,
            a: vec![0.0; max],
            b: vec![0.0; max],
        })
    }

    /// Parse the JSON format in the module docs. Allocates: call it off the
    /// audio thread (see `openplugin_core::worker`).
    pub fn from_json(text: &str) -> Result<Self, ModelError> {
        if text.len() > MAX_JSON_BYTES {
            return Err(ModelError::Json(format!("{} bytes (limit {MAX_JSON_BYTES})", text.len())));
        }
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| ModelError::Json(e.to_string()))?;
        let obj =
            v.as_object().ok_or_else(|| ModelError::Field("top level must be an object".into()))?;
        allow_keys(obj, &["version", "sample_rate", "input_size", "residual", "layers"], "model")?;
        let version = get_uint(obj, "version", "model")?;
        if version != 1 {
            return Err(ModelError::Field(format!("unsupported version {version} (this reads 1)")));
        }
        let rate = obj
            .get("sample_rate")
            .and_then(|x| x.as_f64())
            .ok_or_else(|| ModelError::Field("model.sample_rate must be a number".into()))?;
        let input_size = get_uint(obj, "input_size", "model")?;
        let residual = match obj.get("residual") {
            None => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err(ModelError::Field("model.residual must be a boolean".into())),
        };
        let list = obj
            .get("layers")
            .and_then(|x| x.as_array())
            .ok_or_else(|| ModelError::Field("model.layers must be an array".into()))?;
        let mut layers = Vec::with_capacity(list.len().min(64));
        let mut total = 0usize;
        for (k, item) in list.iter().enumerate() {
            let l = parse_layer(item, k)?;
            total = total.saturating_add(l.weight_count());
            if total > MAX_WEIGHTS {
                return Err(ModelError::Size(format!("more than {MAX_WEIGHTS} weights")));
            }
            layers.push(l);
        }
        Self::new(input_size, rate as f32, residual, layers)
    }

    /// The rate the model was trained at, in Hz.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// `host_rate / trained rate`. Anything but 1 means the model should be
    /// run on resampled audio (module docs).
    pub fn rate_ratio(&self, host_rate: f32) -> f32 {
        host_rate / self.sample_rate
    }

    /// 1 (audio) or 2 (audio and conditioning).
    pub fn input_size(&self) -> usize {
        self.input_size
    }

    /// Whether the audio input is added to the output.
    pub fn residual(&self) -> bool {
        self.residual
    }

    /// Use a rational `tanh` (max error about 1e-4, several times faster) in
    /// place of `f32::tanh` and the logistic function built on it. Off by
    /// default; a model trained with exact activations stays within a few
    /// 1e-4 of its output (see the cookbook for the measured difference).
    pub fn set_fast_activations(&mut self, on: bool) {
        self.fast = on;
    }

    /// Clear all recurrent state and convolution history.
    pub fn reset(&mut self) {
        self.layers.iter_mut().for_each(Layer::reset);
    }

    /// One sample. `cond` is used only when `input_size()` is 2. Non-finite
    /// input is treated as 0. Allocation-free.
    #[inline]
    pub fn process_sample(&mut self, x: f32, cond: f32) -> f32 {
        let x = if x.is_finite() { x } else { 0.0 };
        let cond = if cond.is_finite() { cond } else { 0.0 };
        self.a[0] = x;
        if self.input_size == 2 {
            self.a[1] = cond;
        }
        let (mut cur, mut next) = (&mut self.a, &mut self.b);
        for l in self.layers.iter_mut() {
            l.process(cur, next, self.fast);
            std::mem::swap(&mut cur, &mut next);
        }
        let y = cur[0] + if self.residual { x } else { 0.0 };
        if y.is_finite() {
            flush(y)
        } else {
            0.0
        }
    }

    /// Run a block in place with a constant conditioning value.
    pub fn process_block(&mut self, buf: &mut [f32], cond: f32) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s, cond);
        }
    }
}

type Obj = serde_json::Map<String, serde_json::Value>;

fn allow_keys(o: &Obj, keys: &[&str], what: &str) -> Result<(), ModelError> {
    for k in o.keys() {
        if !keys.contains(&k.as_str()) {
            return Err(ModelError::Field(format!("unknown field \"{k}\" in {what}")));
        }
    }
    Ok(())
}

fn get_uint(o: &Obj, key: &str, what: &str) -> Result<usize, ModelError> {
    let n = o
        .get(key)
        .and_then(|x| x.as_u64())
        .ok_or_else(|| ModelError::Field(format!("{what}.{key} must be a non-negative integer")))?;
    // Clamp far beyond any limit so later arithmetic cannot overflow.
    Ok(n.min(u64::from(u32::MAX)) as usize)
}

fn get_numbers(o: &Obj, key: &str, what: &str, expected: usize) -> Result<Vec<f32>, ModelError> {
    let arr = o
        .get(key)
        .and_then(|x| x.as_array())
        .ok_or_else(|| ModelError::Field(format!("{what}.{key} must be an array of numbers")))?;
    if arr.len() != expected {
        return Err(ModelError::Length {
            what: format!("{what}.{key}"),
            expected,
            found: arr.len(),
        });
    }
    let mut out = Vec::with_capacity(arr.len());
    for x in arr {
        let f = x
            .as_f64()
            .ok_or_else(|| ModelError::Field(format!("{what}.{key} holds a non-number")))?;
        if !f.is_finite() || f.abs() > f64::from(f32::MAX) {
            return Err(ModelError::NotFinite(format!("{what}.{key}")));
        }
        out.push(f as f32);
    }
    Ok(out)
}

fn parse_layer(v: &serde_json::Value, k: usize) -> Result<Layer, ModelError> {
    let what = format!("layers[{k}]");
    let o = v.as_object().ok_or_else(|| ModelError::Field(format!("{what} must be an object")))?;
    let ty = o
        .get("type")
        .and_then(|x| x.as_str())
        .ok_or_else(|| ModelError::Field(format!("{what}.type must be a string")))?;
    let act = |o: &Obj| -> Result<Activation, ModelError> {
        Activation::parse(
            o.get("activation")
                .and_then(|x| x.as_str())
                .ok_or_else(|| ModelError::Field(format!("{what}.activation must be a string")))?,
        )
    };
    match ty {
        "dense" => {
            allow_keys(o, &["type", "in", "out", "activation", "weights", "bias"], &what)?;
            let (i, n) = (get_uint(o, "in", &what)?, get_uint(o, "out", &what)?);
            check_size("dense in", i)?;
            check_size("dense out", n)?;
            let w = get_numbers(o, "weights", &what, mul(i, n)?)?;
            let b = get_numbers(o, "bias", &what, n)?;
            Ok(Layer::Dense(Dense::new(i, n, act(o)?, w, b)?))
        }
        "gru" | "lstm" => {
            allow_keys(o, &["type", "in", "hidden", "w_x", "w_h", "b"], &what)?;
            let g = if ty == "gru" { 3 } else { 4 };
            let (i, h) = (get_uint(o, "in", &what)?, get_uint(o, "hidden", &what)?);
            check_size("recurrent in", i)?;
            check_size("recurrent hidden", h)?;
            let wx = get_numbers(o, "w_x", &what, mul(g * h, i)?)?;
            let wh = get_numbers(o, "w_h", &what, mul(g * h, h)?)?;
            let b = get_numbers(o, "b", &what, g * h)?;
            Ok(if g == 3 {
                Layer::Gru(Gru::new(i, h, wx, wh, b)?)
            } else {
                Layer::Lstm(Lstm::new(i, h, wx, wh, b)?)
            })
        }
        "conv1d" => {
            allow_keys(
                o,
                &["type", "in", "out", "kernel", "dilation", "activation", "weights", "bias"],
                &what,
            )?;
            let (i, n) = (get_uint(o, "in", &what)?, get_uint(o, "out", &what)?);
            let (kern, dil) = (get_uint(o, "kernel", &what)?, get_uint(o, "dilation", &what)?);
            check_size("conv1d in", i)?;
            check_size("conv1d out", n)?;
            if kern == 0 || kern > MAX_KERNEL || dil == 0 || dil > MAX_DILATION {
                return Err(ModelError::Size(format!("{what}: kernel {kern}, dilation {dil}")));
            }
            let w = get_numbers(o, "weights", &what, mul(mul(kern, i)?, n)?)?;
            let b = get_numbers(o, "bias", &what, n)?;
            Ok(Layer::Conv1d(Conv1d::new(i, n, kern, dil, act(o)?, w, b)?))
        }
        other => Err(ModelError::Field(format!(
            "{what}.type \"{other}\" is not dense, gru, lstm or conv1d"
        ))),
    }
}
