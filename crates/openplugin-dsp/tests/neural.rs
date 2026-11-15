//! Proofs for `openplugin_dsp::neural`: outputs against an independent f64
//! reference of the same equations, closed-form cases, hostile JSON, and an
//! ignored CPU benchmark
//! (`cargo test --release -p openplugin-dsp --test neural -- --ignored --nocapture`).

use openplugin_dsp::neural::{Model, ModelError};
use serde_json::{json, Value};
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
    fn vec(&mut self, n: usize, scale: f64) -> Vec<f32> {
        (0..n).map(|_| (self.next() * scale) as f32).collect()
    }
}

/// Layer description used to build both the JSON and nothing else.
fn dense(r: &mut Rng, i: usize, o: usize, act: &str) -> Value {
    let s = 1.5 / (i as f64).sqrt();
    json!({"type":"dense","in":i,"out":o,"activation":act,"weights":r.vec(i*o,s),"bias":r.vec(o,0.3)})
}
fn recurrent(r: &mut Rng, ty: &str, i: usize, h: usize) -> Value {
    let g = if ty == "gru" { 3 } else { 4 };
    let s = 1.5 / ((i + h) as f64).sqrt();
    json!({"type":ty,"in":i,"hidden":h,"w_x":r.vec(g*h*i,s),"w_h":r.vec(g*h*h,s),"b":r.vec(g*h,0.3)})
}
fn conv(r: &mut Rng, i: usize, o: usize, k: usize, d: usize, act: &str) -> Value {
    let s = 1.5 / ((i * k) as f64).sqrt();
    json!({"type":"conv1d","in":i,"out":o,"kernel":k,"dilation":d,"activation":act,
           "weights":r.vec(o*k*i,s),"bias":r.vec(o,0.3)})
}
fn model_json(input: usize, residual: bool, layers: Vec<Value>) -> String {
    json!({"version":1,"sample_rate":44100,"input_size":input,"residual":residual,"layers":layers})
        .to_string()
}

// ---------- independent reference: plain f64 loops over the JSON ----------

fn nums(v: &Value, k: &str) -> Vec<f64> {
    v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}
fn sig(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}
fn act(name: &str, x: f64) -> f64 {
    match name {
        "tanh" => x.tanh(),
        "relu" => x.max(0.0),
        "sigmoid" => sig(x),
        _ => x,
    }
}

struct RefLayer {
    spec: Value,
    h: Vec<f64>,
    c: Vec<f64>,
    hist: Vec<Vec<f64>>, // conv: all past inputs
}

fn reference(text: &str, xs: &[f32], cond: f32) -> Vec<f64> {
    let m: Value = serde_json::from_str(text).unwrap();
    let residual = m["residual"].as_bool().unwrap_or(false);
    let input = m["input_size"].as_u64().unwrap() as usize;
    let mut layers: Vec<RefLayer> = m["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let n = l["hidden"].as_u64().unwrap_or(0) as usize;
            RefLayer { spec: l.clone(), h: vec![0.0; n], c: vec![0.0; n], hist: vec![] }
        })
        .collect();
    let mut out = Vec::new();
    for &x in xs {
        let mut v: Vec<f64> = vec![f64::from(x)];
        if input == 2 {
            v.push(f64::from(cond));
        }
        for l in layers.iter_mut() {
            let ty = l.spec["type"].as_str().unwrap();
            let i = l.spec["in"].as_u64().unwrap() as usize;
            match ty {
                "dense" => {
                    let o = l.spec["out"].as_u64().unwrap() as usize;
                    let (w, b) = (nums(&l.spec, "weights"), nums(&l.spec, "bias"));
                    let a = l.spec["activation"].as_str().unwrap();
                    v = (0..o)
                        .map(|r| act(a, b[r] + (0..i).map(|c| w[r * i + c] * v[c]).sum::<f64>()))
                        .collect();
                }
                "gru" => {
                    let h = l.h.len();
                    let (wx, wh, b) = (nums(&l.spec, "w_x"), nums(&l.spec, "w_h"), nums(&l.spec, "b"));
                    let gate = |blk: usize, j: usize, hv: &[f64]| -> f64 {
                        let r = blk * h + j;
                        b[r] + (0..i).map(|c| wx[r * i + c] * v[c]).sum::<f64>()
                            + (0..h).map(|c| wh[r * h + c] * hv[c]).sum::<f64>()
                    };
                    let z: Vec<f64> = (0..h).map(|j| sig(gate(0, j, &l.h))).collect();
                    let rr: Vec<f64> = (0..h).map(|j| sig(gate(1, j, &l.h))).collect();
                    let rh: Vec<f64> = (0..h).map(|j| rr[j] * l.h[j]).collect();
                    let n: Vec<f64> = (0..h).map(|j| gate(2, j, &rh).tanh()).collect();
                    l.h = (0..h).map(|j| z[j] * l.h[j] + (1.0 - z[j]) * n[j]).collect();
                    v = l.h.clone();
                }
                "lstm" => {
                    let h = l.h.len();
                    let (wx, wh, b) = (nums(&l.spec, "w_x"), nums(&l.spec, "w_h"), nums(&l.spec, "b"));
                    let gate = |blk: usize, j: usize| -> f64 {
                        let r = blk * h + j;
                        b[r] + (0..i).map(|c| wx[r * i + c] * v[c]).sum::<f64>()
                            + (0..h).map(|c| wh[r * h + c] * l.h[c]).sum::<f64>()
                    };
                    let g: Vec<[f64; 4]> = (0..h)
                        .map(|j| [sig(gate(0, j)), sig(gate(1, j)), gate(2, j).tanh(), sig(gate(3, j))])
                        .collect();
                    for (j, gj) in g.iter().enumerate() {
                        l.c[j] = gj[1] * l.c[j] + gj[0] * gj[2];
                        l.h[j] = gj[3] * l.c[j].tanh();
                    }
                    v = l.h.clone();
                }
                "conv1d" => {
                    let o = l.spec["out"].as_u64().unwrap() as usize;
                    let k = l.spec["kernel"].as_u64().unwrap() as usize;
                    let d = l.spec["dilation"].as_u64().unwrap() as usize;
                    let (w, b) = (nums(&l.spec, "weights"), nums(&l.spec, "bias"));
                    let a = l.spec["activation"].as_str().unwrap();
                    l.hist.push(v.clone());
                    let t = l.hist.len() - 1;
                    v = (0..o)
                        .map(|r| {
                            let mut s = b[r];
                            for j in 0..k {
                                if t >= j * d {
                                    for c in 0..i {
                                        s += w[(r * k + j) * i + c] * l.hist[t - j * d][c];
                                    }
                                }
                            }
                            act(a, s)
                        })
                        .collect();
                }
                _ => unreachable!(),
            }
        }
        out.push(v[0] + if residual { f64::from(x) } else { 0.0 });
    }
    out
}

fn signal(n: usize, seed: u64) -> Vec<f32> {
    let mut r = Rng(seed);
    (0..n)
        .map(|t| {
            let tone = (t as f64 * 0.05).sin() * 0.6;
            (tone + r.next() * 0.3) as f32
        })
        .collect()
}

fn compare(text: &str, cond: f32, samples: usize, tol: f64) -> f64 {
    let xs = signal(samples, 99);
    let want = reference(text, &xs, cond);
    let mut m = Model::from_json(text).unwrap();
    let mut worst = 0.0f64;
    for (x, w) in xs.iter().zip(&want) {
        let got = f64::from(m.process_sample(*x, cond));
        worst = worst.max((got - w).abs());
    }
    assert!(worst <= tol, "worst error {worst} over tolerance {tol}");
    worst
}

const TOL: f64 = 1e-5;

#[test]
fn stacks_match_the_reference() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    let mut worst = 0.0f64;
    let stacks: Vec<(usize, bool, Vec<Value>)> = vec![
        (1, false, vec![dense(&mut r, 1, 1, "identity")]),
        (1, false, vec![dense(&mut r, 1, 7, "tanh"), dense(&mut r, 7, 5, "relu"), dense(&mut r, 5, 1, "sigmoid")]),
        (1, true, vec![recurrent(&mut r, "gru", 1, 8), dense(&mut r, 8, 1, "identity")]),
        (1, false, vec![recurrent(&mut r, "lstm", 1, 9), dense(&mut r, 9, 1, "tanh")]),
        (2, true, vec![recurrent(&mut r, "lstm", 2, 16), recurrent(&mut r, "gru", 16, 12), dense(&mut r, 12, 1, "identity")]),
        (2, false, vec![recurrent(&mut r, "gru", 2, 33), dense(&mut r, 33, 1, "identity")]),
        (1, true, vec![recurrent(&mut r, "lstm", 1, 64), dense(&mut r, 64, 1, "identity")]),
        (1, false, vec![recurrent(&mut r, "gru", 1, 64), dense(&mut r, 64, 1, "identity")]),
        (1, false, vec![
            conv(&mut r, 1, 6, 3, 1, "tanh"),
            conv(&mut r, 6, 6, 3, 2, "tanh"),
            conv(&mut r, 6, 4, 2, 7, "relu"),
            dense(&mut r, 4, 1, "identity"),
        ]),
        (2, true, vec![
            conv(&mut r, 2, 5, 4, 3, "tanh"),
            recurrent(&mut r, "gru", 5, 6),
            dense(&mut r, 6, 1, "identity"),
        ]),
    ];
    for (k, (input, res, layers)) in stacks.into_iter().enumerate() {
        let text = model_json(input, res, layers);
        let e = compare(&text, 0.4, 10_000, TOL);
        println!("stack {k}: worst abs error {e:.2e}");
        worst = worst.max(e);
    }
    println!("worst over all stacks: {worst:.2e} (tolerance {TOL:.0e})");
}

#[test]
fn fast_activations_stay_close() {
    let mut r = Rng(77);
    let text = model_json(1, false, vec![recurrent(&mut r, "lstm", 1, 16), recurrent(&mut r, "gru", 16, 16), dense(&mut r, 16, 1, "tanh")]);
    let xs = signal(10_000, 4);
    let want = reference(&text, &xs, 0.0);
    let mut m = Model::from_json(&text).unwrap();
    m.set_fast_activations(true);
    let worst = xs.iter().zip(&want).map(|(x, w)| (f64::from(m.process_sample(*x, 0.0)) - w).abs()).fold(0.0, f64::max);
    println!("fast activations: worst abs error {worst:.2e}");
    assert!(worst < 2e-3, "{worst}");
}

#[test]
fn reset_restores_the_first_run() {
    let mut r = Rng(5);
    let text = model_json(1, false, vec![recurrent(&mut r, "lstm", 1, 8), conv(&mut r, 8, 3, 3, 5, "tanh"), dense(&mut r, 3, 1, "identity")]);
    let xs = signal(500, 3);
    let mut m = Model::from_json(&text).unwrap();
    let a: Vec<f32> = xs.iter().map(|&x| m.process_sample(x, 0.0)).collect();
    m.reset();
    let b: Vec<f32> = xs.iter().map(|&x| m.process_sample(x, 0.0)).collect();
    assert_eq!(a, b);
    // Block processing equals per-sample processing.
    m.reset();
    let mut buf = xs.clone();
    for chunk in buf.chunks_mut(37) {
        m.process_block(chunk, 0.0);
    }
    assert_eq!(buf, a);
}

fn single_unit(layer: Value) -> Model {
    let dense = json!({"type":"dense","in":1,"out":1,"activation":"identity","weights":[1.0],"bias":[0.0]});
    Model::from_json(&model_json(1, false, vec![layer, dense])).unwrap()
}

#[test]
fn single_unit_gru_by_hand() {
    // wz=wr=0, bz=br=0 (z = r = 1/2); wn = un = 1, bn = 0, x = 1:
    // h1 = (1/2)tanh(1); h2 = h1/2 + tanh(1 + h1/2)/2; and so on.
    // Values below computed by hand-iterating those formulas at f64.
    let mut m = single_unit(json!({"type":"gru","in":1,"hidden":1,
        "w_x":[0.0,0.0,1.0],"w_h":[0.0,0.0,1.0],"b":[0.0,0.0,0.0]}));
    let want = [0.3807970779778824, 0.6057497539949555, 0.734105585564186];
    for w in want {
        let got = m.process_sample(1.0, 0.0);
        assert!((f64::from(got) - w).abs() < 1e-6, "{got} vs {w}");
    }
    // First step in closed form: tanh(1)/2.
    assert!((want[0] - 1f64.tanh() / 2.0).abs() < 1e-15);
}

#[test]
fn single_unit_lstm_by_hand() {
    // Input weights i=1, f=0, g=2, o=1; recurrent weights 0; biases f=1, others 0; x=1.
    // i = o = sigmoid(1), f = sigmoid(1), g = tanh(2).
    // c1 = i g, h1 = o tanh(c1); c2 = f c1 + i g, ...
    let mut m = single_unit(json!({"type":"lstm","in":1,"hidden":1,
        "w_x":[1.0,0.0,2.0,1.0],"w_h":[0.0,0.0,0.0,0.0],"b":[0.0,1.0,0.0,0.0]}));
    let want = [0.4440309787835238, 0.6138324932435952, 0.6734226855373425];
    for w in want {
        let got = m.process_sample(1.0, 0.0);
        assert!((f64::from(got) - w).abs() < 1e-6, "{got} vs {w}");
    }
    let (i, g) = (1.0 / (1.0 + (-1f64).exp()), 2f64.tanh());
    assert!((want[0] - i * (i * g).tanh()).abs() < 1e-15);
}

#[test]
fn conv_impulse_response_is_the_kernel() {
    // One output, kernel 3, dilation 4: taps land at 0, 4 and 8 samples.
    let conv = json!({"type":"conv1d","in":1,"out":1,"kernel":3,"dilation":4,"activation":"identity",
        "weights":[0.5,-0.25,0.125],"bias":[0.0]});
    let mut m = single_unit(conv);
    let y: Vec<f32> = (0..12).map(|t| m.process_sample(if t == 0 { 1.0 } else { 0.0 }, 0.0)).collect();
    let mut want = vec![0.0f32; 12];
    want[0] = 0.5;
    want[4] = -0.25;
    want[8] = 0.125;
    assert_eq!(y, want);
}

#[test]
fn residual_and_conditioning() {
    let d = json!({"type":"dense","in":2,"out":1,"activation":"identity","weights":[0.0,3.0],"bias":[0.0]});
    let mut m = Model::from_json(&model_json(2, true, vec![d])).unwrap();
    assert_eq!(m.process_sample(0.25, 0.5), 1.5 + 0.25);
    assert_eq!(m.input_size(), 2);
    assert!(m.residual());
    assert_eq!(m.sample_rate(), 44100.0);
    assert!((m.rate_ratio(88200.0) - 2.0).abs() < 1e-6);
    // Non-finite input is silenced, not propagated.
    assert_eq!(m.process_sample(f32::NAN, 0.0), 0.0);
}

#[test]
fn tiny_example_file_loads() {
    let text = include_str!("data/tiny_model.json");
    let mut m = Model::from_json(text).unwrap();
    let e = compare(text, 0.0, 2000, TOL);
    assert!(e <= TOL);
    assert!(m.process_sample(0.5, 0.0).is_finite());
}

// ---------- hostile input ----------

fn err(text: &str) -> ModelError {
    Model::from_json(text).expect_err(text)
}

fn good() -> Value {
    let mut r = Rng(1);
    serde_json::from_str(&model_json(1, false, vec![recurrent(&mut r, "gru", 1, 3), dense(&mut r, 3, 1, "identity")])).unwrap()
}

#[test]
fn malformed_files_are_refused_with_clear_errors() {
    let text = |v: &Value| v.to_string();
    // Not JSON / wrong top level.
    assert!(matches!(err("{"), ModelError::Json(_)));
    assert!(matches!(err(""), ModelError::Json(_)));
    assert!(matches!(err("[1,2]"), ModelError::Field(_)));
    // Version.
    let mut v = good();
    v["version"] = json!(2);
    assert!(err(&text(&v)).to_string().contains("version 2"));
    v = good();
    v.as_object_mut().unwrap().remove("version");
    assert!(err(&text(&v)).to_string().contains("version"));
    // Unknown field.
    v = good();
    v["extra"] = json!(1);
    assert!(err(&text(&v)).to_string().contains("unknown field"));
    v = good();
    v["layers"][0]["extra"] = json!(1);
    assert!(err(&text(&v)).to_string().contains("unknown field"));
    // Wrong array length.
    v = good();
    v["layers"][0]["w_h"].as_array_mut().unwrap().pop();
    let e = err(&text(&v));
    assert!(matches!(e, ModelError::Length { expected: 27, found: 26, .. }), "{e}");
    assert!(e.to_string().contains("layers[0].w_h"));
    // Declared size disagrees with data.
    v = good();
    v["layers"][0]["hidden"] = json!(4);
    assert!(matches!(err(&text(&v)), ModelError::Length { .. }));
    // Layers that do not chain / bad ends.
    v = good();
    v["layers"][1]["in"] = json!(4);
    v["layers"][1]["weights"] = json!([0.0, 0.0, 0.0, 0.0]);
    assert!(matches!(err(&text(&v)), ModelError::Chain(_)));
    v = good();
    v["input_size"] = json!(2);
    assert!(matches!(err(&text(&v)), ModelError::Chain(_)));
    v = good();
    v["input_size"] = json!(3);
    assert!(matches!(err(&text(&v)), ModelError::Size(_)));
    v = good();
    v["layers"] = json!([]);
    assert!(matches!(err(&text(&v)), ModelError::Chain(_)));
    v = good();
    v["layers"].as_array_mut().unwrap().pop();
    assert!(matches!(err(&text(&v)), ModelError::Chain(_)));
    // Types.
    v = good();
    v["layers"][0]["type"] = json!("transformer");
    assert!(err(&text(&v)).to_string().contains("transformer"));
    v = good();
    v["layers"][1]["activation"] = json!("swish");
    assert!(err(&text(&v)).to_string().contains("swish"));
    v = good();
    v["layers"][0]["b"][0] = json!("x");
    assert!(matches!(err(&text(&v)), ModelError::Field(_)));
    v = good();
    v["sample_rate"] = json!(5);
    assert!(matches!(err(&text(&v)), ModelError::Field(_)));
    v = good();
    v["layers"][0]["in"] = json!(-1);
    assert!(matches!(err(&text(&v)), ModelError::Field(_)));
}

#[test]
fn non_finite_and_huge_numbers_are_refused() {
    // JSON has no NaN literal; serde_json rejects the extension spellings.
    let base = model_json(1, false, vec![json!({"type":"dense","in":1,"out":1,"activation":"identity","weights":[0.5],"bias":[0.0]})]);
    for bad in ["NaN", "Infinity", "-Infinity", "null"] {
        let t = base.replace("[0.5]", &format!("[{bad}]"));
        assert!(Model::from_json(&t).is_err(), "{bad}");
    }
    // Out of f64 range and out of f32 range.
    for bad in ["1e999", "1e39", "-1e39"] {
        let t = base.replace("[0.5]", &format!("[{bad}]"));
        let e = Model::from_json(&t).expect_err(bad);
        assert!(matches!(e, ModelError::NotFinite(_) | ModelError::Json(_)), "{bad}: {e}");
    }
}

#[test]
fn huge_sizes_are_refused_without_allocating_them() {
    // Declared sizes far beyond what the data could be, or the limits.
    for (i, h) in [(1u64, 1u64 << 40), (1 << 40, 1), (1, 513), (1, 0), (u64::MAX, 1)] {
        let t = json!({"version":1,"sample_rate":48000,"input_size":1,"layers":[
            {"type":"gru","in":i,"hidden":h,"w_x":[],"w_h":[],"b":[]}]})
        .to_string();
        let e = err(&t);
        assert!(matches!(e, ModelError::Size(_) | ModelError::Field(_)), "{i} {h}: {e}");
    }
    let t = json!({"version":1,"sample_rate":48000,"input_size":1,"layers":[
        {"type":"conv1d","in":1,"out":1,"kernel":65,"dilation":1,"activation":"tanh","weights":[],"bias":[0.0]}]})
    .to_string();
    assert!(matches!(err(&t), ModelError::Size(_)));
    let t = t.replace("65", "2").replace("\"dilation\":1", "\"dilation\":4097");
    assert!(matches!(err(&t), ModelError::Size(_)));
    // A legal size with empty arrays is a length error, found before any allocation.
    let big = r#"{"version":1,"sample_rate":48000,"input_size":1,"layers":[{"type":"lstm","in":512,"hidden":512,"w_x":[],"w_h":[],"b":[]}]}"#;
    assert!(matches!(err(big), ModelError::Length { expected: 1_048_576, found: 0, .. }));
    // Deep nesting does not crash the parser.
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(matches!(err(&deep), ModelError::Json(_)));
}

// ---------- CPU ----------

#[test]
#[ignore]
fn cpu_per_block() {
    let (rate, block) = (48_000.0f64, 512usize);
    println!("model        us/block   real-time fraction (block {block} at 48 kHz)");
    for (ty, fast) in [("lstm", false), ("gru", false), ("lstm", true), ("gru", true)] {
        for h in [16usize, 32, 64] {
            let mut r = Rng(42 + h as u64);
            let text = model_json(1, false, vec![recurrent(&mut r, ty, 1, h), dense(&mut r, h, 1, "identity")]);
            let mut m = Model::from_json(&text).unwrap();
            m.set_fast_activations(fast);
            let mut buf = signal(block, 1);
            let mut best = f64::MAX;
            for _ in 0..30 {
                let t0 = Instant::now();
                for _ in 0..20 {
                    m.process_block(&mut buf, 0.0);
                    buf.iter_mut().for_each(|x| *x = x.clamp(-1.0, 1.0));
                }
                best = best.min(t0.elapsed().as_secs_f64() / 20.0);
            }
            let budget = block as f64 / rate;
            println!("{ty} {h:>3} {}  {:>9.1}   {:.4} ({:.2}% CPU)", if fast { "fast " } else { "exact" }, best * 1e6, best / budget, 100.0 * best / budget);
        }
    }
}
