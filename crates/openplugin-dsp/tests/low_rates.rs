//! Every DSP block that takes a sample rate works at the rates the format
//! bridges accept, down to `openplugin_core::MIN_SAMPLE_RATE` (1 kHz): its time
//! constants follow the real rate (not a 48 kHz stand-in), its output stays
//! finite, and its constructors do not refuse a rate the bridges accept.

use openplugin_core::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE};
use openplugin_dsp::{
    effective_rate, BlockLevel, Delta, GainComp, KWeighting, Loudness, LoudnessMatch, MeterBank, SoftBypass,
};

const RATES: [f32; 4] = [1_000.0, 1_234.567_8, 4_000.0, 48_000.0];

fn sine(freq: f32, amp: f32, rate: f32, frames: usize) -> Vec<f32> {
    (0..frames).map(|n| amp * (std::f32::consts::TAU * freq * n as f32 / rate).sin()).collect()
}

#[test]
fn effective_rate_keeps_every_bridge_rate() {
    for rate in RATES {
        assert_eq!(effective_rate(rate), rate);
    }
    assert_eq!(effective_rate(MIN_SAMPLE_RATE), MIN_SAMPLE_RATE);
    assert_eq!(effective_rate(MAX_SAMPLE_RATE), MAX_SAMPLE_RATE);
    assert_eq!(effective_rate(10.0), MIN_SAMPLE_RATE);
    assert_eq!(effective_rate(1.0e9), MAX_SAMPLE_RATE);
    assert_eq!(effective_rate(f32::NAN), 48_000.0);
    assert_eq!(effective_rate(f32::INFINITY), 48_000.0);
}

/// The 10 ms bypass crossfade takes 10 ms of samples at every rate.
#[test]
fn bypass_fade_is_ten_milliseconds_at_every_rate() {
    for rate in RATES {
        let mut bypass = SoftBypass::prepare(1, 0, rate);
        bypass.set_bypassed(true);
        let frames = (rate * 0.01).ceil() as usize + 1;
        let dry = vec![0.0f32; frames];
        let mut wet = vec![1.0f32; frames];
        bypass.mix(&[&dry], &mut [&mut wet]);
        assert_eq!(bypass.wet(), 0.0, "{rate} Hz: faded out in {frames} samples");
        let mut bypass = SoftBypass::prepare(1, 0, rate);
        bypass.set_bypassed(true);
        let half = (rate * 0.005) as usize;
        let mut wet = vec![1.0f32; half];
        bypass.mix(&[&dry[..half]], &mut [&mut wet]);
        assert!((bypass.wet() - 0.5).abs() < 0.15, "{rate} Hz: halfway after 5 ms, got {}", bypass.wet());
    }
}

/// The Delta glide takes its glide time at every rate.
#[test]
fn delta_glide_follows_the_rate() {
    for rate in RATES {
        let mut delta = Delta::prepare(1, 0, rate, 20.0);
        delta.set_enabled(true);
        let frames = (rate * 0.02).ceil() as usize + 1;
        let dry = vec![1.0f32; frames];
        let mut out = vec![1.0f32; frames];
        delta.process(&[&dry], &mut [&mut out]);
        assert_eq!(delta.amount(), 1.0, "{rate} Hz: full difference after 20 ms");
        assert!(out.iter().all(|v| v.is_finite()));
    }
}

/// K-weighting stays stable at low rates, where its shelf would sit above Nyquist.
#[test]
fn k_weighting_is_stable_at_low_rates() {
    for rate in RATES {
        let mut k = KWeighting::new(rate);
        let mut peak = 0.0f64;
        let mut seed = 0x1234_5678u32;
        for _ in 0..(rate as usize * 4) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
            let y = k.process(x);
            assert!(y.is_finite(), "{rate} Hz");
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "{rate} Hz: bounded, peak {peak}");
    }
}

/// A tone well inside the band reads its level at every rate: a stereo sine of
/// amplitude A dBFS at a frequency where K-weighting is near flat reads close to A.
#[test]
fn loudness_reads_a_tone_at_every_rate() {
    for rate in RATES {
        let freq = (rate * 0.1).min(1_000.0);
        let amp = 10f32.powf(-20.0 / 20.0);
        let tone = sine(freq, amp, rate, (rate * 4.0) as usize);
        let mut meter = Loudness::new(rate);
        meter.push(&[&tone, &tone]);
        let short = meter.short_term_lufs();
        assert!(short.is_finite(), "{rate} Hz");
        assert!((short - -20.0).abs() < 1.5, "{rate} Hz, {freq} Hz tone: {short} LUFS");
    }
}

/// Loudness matching converges at low rates, in roughly the same wall time.
#[test]
fn loudness_match_converges_at_low_rates() {
    for rate in RATES {
        let freq = (rate * 0.1).min(220.0);
        let reference = sine(freq, 0.5, rate, (rate * 6.0) as usize);
        let mut processed: Vec<f32> = reference.iter().map(|x| x * 0.25).collect();
        let mut auto = LoudnessMatch::new(rate);
        auto.process(&[&reference], &mut [&mut processed]);
        assert!((auto.gain_db() - 12.04).abs() < 0.3, "{rate} Hz: {} dB", auto.gain_db());
    }
}

/// Meters and gain compensation accept every bridge rate and stay finite.
#[test]
fn meters_and_gain_comp_accept_bridge_rates() {
    for rate in RATES {
        assert!(MeterBank::new(rate as u32).is_some(), "{rate} Hz");
        let mut level = BlockLevel::new(rate);
        let tone = sine(rate * 0.1, 0.5, rate, rate as usize);
        level.push_stereo(&tone, &tone);
        assert!(level.short_db().is_finite(), "{rate} Hz");
        let mut comp = GainComp::new(rate);
        for x in &tone {
            assert!(comp.process([*x, *x], [*x * 0.5, *x * 0.5], true).is_finite());
        }
    }
    assert!(MeterBank::new(999).is_none());
    assert!(MeterBank::new(768_001).is_none());
}
