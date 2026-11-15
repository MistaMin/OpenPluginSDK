//! Built-in test signals and the input spec of `openplugin-port null`.

use std::path::Path;

use crate::wav::{self, Audio};

pub const SIGNALS: [&str; 5] = ["sweep", "noise", "impulses", "burst", "silence"];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Two channels of the named signal, `seconds` long.
#[allow(clippy::needless_range_loop)]
pub fn generate(name: &str, rate: u32, seconds: f64) -> Result<Audio, String> {
    let n = (seconds * f64::from(rate)).round() as usize;
    if n == 0 {
        return Err("the input would be empty".into());
    }
    let r = f64::from(rate);
    let tau = 2.0 * std::f64::consts::PI;
    let mut ch = vec![vec![0f32; n]; 2];
    match name {
        "sweep" => {
            // Logarithmic sweep from 20 Hz to 20 kHz (kept below Nyquist); the right channel is offset in phase.
            let (f0, f1) = (20.0f64, 20_000.0f64.min(r * 0.45));
            let k = (f1 / f0).ln();
            let t = n as f64 / r;
            for i in 0..n {
                let x = i as f64 / r;
                let phase = tau * f0 * t / k * ((x / t * k).exp() - 1.0);
                ch[0][i] = (0.5 * phase.sin()) as f32;
                ch[1][i] = (0.5 * (phase + 1.0).sin()) as f32;
            }
        }
        "noise" => {
            for (c, seed) in [(0usize, 0x9E37_79B9_7F4A_7C15u64), (1, 0xD1B5_4A32_D192_ED03)] {
                let mut rng = Rng(seed);
                for s in ch[c].iter_mut() {
                    *s = 0.25 * rng.next();
                }
            }
        }
        "impulses" => {
            for (k, i) in (0..n).step_by(997).enumerate() {
                ch[0][i] = if k % 2 == 0 { 1.0 } else { -0.5 };
                ch[1][i] = if k % 3 == 0 { 0.8 } else { 0.4 };
            }
        }
        "burst" => {
            // Silence for the first quarter, a quarter of 997 Hz and 1523 Hz tones, then silence.
            for i in n / 4..n / 2 {
                let x = i as f64 / r;
                ch[0][i] = (0.5 * (tau * 997.0 * x).sin()) as f32;
                ch[1][i] = (0.4 * (tau * 1523.0 * x).sin()) as f32;
            }
        }
        "silence" => {}
        other => return Err(format!("unknown signal {other}: use {} or a .wav path", SIGNALS.join(", "))),
    }
    Ok(Audio { rate, channels: ch })
}

/// `name[:seconds]` or a WAV path. `rate` is `None` to take a WAV file's own rate (48000 for generated signals).
pub fn from_spec(spec: &str, rate: Option<u32>) -> Result<Audio, String> {
    let is_wav = spec.to_ascii_lowercase().ends_with(".wav") || Path::new(spec).is_file();
    if is_wav {
        let a = wav::read(Path::new(spec))?;
        if let Some(r) = rate {
            if r != a.rate {
                return Err(format!("{spec} is {} Hz but --rate is {r}: resample the file or drop --rate", a.rate));
            }
        }
        return Ok(a);
    }
    let (name, secs) = match spec.split_once(':') {
        Some((n, s)) => (n, s.parse::<f64>().map_err(|_| format!("bad length in input spec {spec:?}"))?),
        None => (spec, 2.0),
    };
    generate(name, rate.unwrap_or(48_000), secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_are_deterministic_and_shaped() {
        for s in SIGNALS {
            assert_eq!(generate(s, 48000, 0.5).unwrap(), generate(s, 48000, 0.5).unwrap());
        }
        let b = generate("burst", 48000, 1.0).unwrap();
        assert!(b.channels[0][..12000].iter().all(|v| *v == 0.0));
        assert!(b.channels[0][12000..24000].iter().any(|v| *v != 0.0));
        assert!(b.channels[0][24000..].iter().all(|v| *v == 0.0));
        let i = generate("impulses", 48000, 0.5).unwrap();
        assert_eq!(i.channels[0][0], 1.0);
        assert_eq!(i.channels[0][997], -0.5);
        assert!(generate("nope", 48000, 1.0).is_err());
        assert_eq!(from_spec("noise:0.25", None).unwrap().frames(), 12000);
    }
}
