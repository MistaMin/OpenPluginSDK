//! Conformance to EBU Tech 3341 v4 (November 2023), table 1, and EBU Tech
//! 3342 v4 (November 2023), table 1, with the test signals generated from the
//! descriptions in those tables ("similar to #1": a stereo 1 kHz sine, the
//! same in both channels, level given as per-channel peak dBFS).
//!
//! Covered: Tech 3341 cases 1 to 6, 9 to 14 and 15 to 19 (the latter in
//! `true_peak.rs` unit tests, with 20 to 23 only loosely); Tech 3342 cases 1
//! to 4. Not covered: cases 7 and 8 of Tech 3341 and 5 and 6 of Tech 3342
//! (authentic programme recordings, which only the EBU download provides),
//! and cases 20 to 23 at the standard's own tolerance (the signals are
//! synthesised and decimated by the test, not the EBU files).

use openplugin_dsp::{EbuMeter, Loudness};

const FS: f32 = 48_000.0;

fn tone(level_dbfs: f32, seconds: f32) -> Vec<f32> {
    let amp = 10f32.powf(level_dbfs / 20.0);
    let n = (seconds * FS).round() as usize;
    (0..n).map(|i| amp * (std::f32::consts::TAU * 1000.0 * i as f32 / FS).sin()).collect()
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * FS).round() as usize]
}

fn stereo(parts: &[Vec<f32>]) -> Vec<f32> {
    parts.concat()
}

fn measure(signal: &[f32]) -> Loudness {
    let mut m = Loudness::new(FS);
    m.push(&[signal, signal]);
    m
}

fn close(value: f32, expected: f32, tol: f32, what: &str) {
    assert!((value - expected).abs() <= tol, "{what}: {value}, expected {expected} +/- {tol}");
}

#[test]
fn tech3341_case_1_and_2_levels() {
    for level in [-23.0f32, -33.0] {
        let m = measure(&tone(level, 20.0));
        close(m.momentary_lufs(), level, 0.1, "M");
        close(m.short_term_lufs(), level, 0.1, "S");
        close(m.integrated_lufs(), level, 0.1, "I");
    }
}

#[test]
fn tech3341_case_3_absolute_gate() {
    let s = stereo(&[tone(-36.0, 10.0), tone(-23.0, 60.0), tone(-36.0, 10.0)]);
    close(measure(&s).integrated_lufs(), -23.0, 0.1, "I");
}

#[test]
fn tech3341_case_4_absolute_gate_with_silence() {
    let s = stereo(&[tone(-72.0, 10.0), tone(-36.0, 10.0), tone(-23.0, 60.0), tone(-36.0, 10.0), tone(-72.0, 10.0)]);
    close(measure(&s).integrated_lufs(), -23.0, 0.1, "I");
}

#[test]
fn tech3341_case_5_relative_gate() {
    let s = stereo(&[tone(-26.0, 20.0), tone(-20.0, 20.1), tone(-26.0, 20.0)]);
    close(measure(&s).integrated_lufs(), -23.0, 0.1, "I");
}

/// Case 6: 5.0, 1 kHz, 20 s; L and R at -28, C at -24, Ls and Rs at -30 dBFS peak.
#[test]
fn tech3341_case_6_five_channel_weights() {
    use openplugin_core::SpeakerLayout;
    let mut m = Loudness::for_layout(FS, SpeakerLayout::Surround50).unwrap();
    let l = tone(-28.0, 20.0);
    let c = tone(-24.0, 20.0);
    let s = tone(-30.0, 20.0);
    // Channel order of the SDK 5.0 map: FL FR FC BL BR.
    m.push(&[&l, &l, &c, &s, &s]);
    close(m.integrated_lufs(), -23.0, 0.1, "I");
}

#[test]
fn lfe_is_excluded_and_surrounds_weigh_plus_1_5_db() {
    use openplugin_core::SpeakerLayout;
    let t = tone(-20.0, 10.0);
    let z = silence(10.0);
    // Left alone in a 5.1 bus.
    let mut front = Loudness::for_layout(FS, SpeakerLayout::Surround51).unwrap();
    front.push(&[&t, &z, &z, &z, &z, &z]);
    // The same tone on the back left (channel 4 in 5.1: FL FR FC LFE BL BR).
    let mut back = Loudness::for_layout(FS, SpeakerLayout::Surround51).unwrap();
    back.push(&[&z, &z, &z, &z, &t, &z]);
    close(back.integrated_lufs() - front.integrated_lufs(), 10.0 * 1.41f32.log10(), 0.02, "surround");
    // LFE alone is silence.
    let mut lfe = Loudness::for_layout(FS, SpeakerLayout::Surround51).unwrap();
    lfe.push(&[&z, &z, &z, &t, &z, &z]);
    assert_eq!(lfe.integrated_lufs(), f32::NEG_INFINITY);
    // 7.1.4: heights are weighed at 1.0 (a choice, documented), sides like backs.
    let mut m = Loudness::for_layout(FS, SpeakerLayout::Surround714).unwrap();
    let mut chans = vec![&z[..]; 12];
    chans[8] = &t; // TFL
    m.push(&chans);
    close(m.integrated_lufs() - front.integrated_lufs(), 0.0, 0.02, "height");
}

/// Case 9: (1.34 s at -20, 1.66 s at -30) x5; S = -23.0 +/- 0.1, constant after 3 s.
#[test]
fn tech3341_case_9_short_term_is_constant_after_3_s() {
    let cycle = stereo(&[tone(-20.0, 1.34), tone(-30.0, 1.66)]);
    let signal: Vec<f32> = cycle.iter().cycle().take(cycle.len() * 5).copied().collect();
    let mut m = Loudness::new(FS);
    for (i, chunk) in signal.chunks(480).enumerate() {
        m.push(&[chunk, chunk]);
        if (i + 1) * 480 >= 3 * FS as usize {
            close(m.short_term_lufs(), -23.0, 0.1, &format!("S at step {i}"));
        }
    }
}

/// Case 12: (0.18 s at -20, 0.22 s at -30) x25; M = -23.0 +/- 0.1, constant after 1 s.
#[test]
fn tech3341_case_12_momentary_is_constant_after_1_s() {
    let cycle = stereo(&[tone(-20.0, 0.18), tone(-30.0, 0.22)]);
    let signal: Vec<f32> = cycle.iter().cycle().take(cycle.len() * 25).copied().collect();
    let mut m = Loudness::new(FS);
    for (i, chunk) in signal.chunks(480).enumerate() {
        m.push(&[chunk, chunk]);
        if (i + 1) * 480 >= FS as usize {
            close(m.momentary_lufs(), -23.0, 0.1, &format!("M at step {i}"));
        }
    }
}

/// Case 10 (file-based): i*0.15 s silence, 3 s at -23, 1 s silence; Max S = -23.0 for each.
#[test]
fn tech3341_case_10_max_short_term_per_segment() {
    for i in 0..20 {
        let s = stereo(&[silence(i as f32 * 0.15), tone(-23.0, 3.0), silence(1.0)]);
        close(measure(&s).max_short_term_lufs(), -23.0, 0.1, &format!("segment {i}"));
    }
}

/// Case 11 (live): i*0.15 s silence, 3 s at -38+i, 3 - i*0.15 s silence; Max S = -38 .. -19.
#[test]
fn tech3341_case_11_max_short_term_live() {
    let mut m = Loudness::new(FS);
    for i in 0..20 {
        let s = stereo(&[silence(i as f32 * 0.15), tone(-38.0 + i as f32, 3.0), silence(3.0 - i as f32 * 0.15)]);
        m.reset_max();
        m.push(&[&s, &s]);
        close(m.max_short_term_lufs(), -38.0 + i as f32, 0.1, &format!("segment {i}"));
    }
}

/// Case 13 (file-based): i*20 ms silence, 400 ms at -23, 1 s silence; Max M = -23.0 for each.
#[test]
fn tech3341_case_13_max_momentary_per_segment() {
    for i in 0..20 {
        let s = stereo(&[silence(i as f32 * 0.02), tone(-23.0, 0.4), silence(1.0)]);
        close(measure(&s).max_momentary_lufs(), -23.0, 0.1, &format!("segment {i}"));
    }
}

/// Case 14 (live): i*20 ms silence, 400 ms at -38+i, 400 - i*20 ms silence; Max M = -38 .. -19.
#[test]
fn tech3341_case_14_max_momentary_live() {
    let mut m = Loudness::new(FS);
    for i in 0..20 {
        let s = stereo(&[silence(i as f32 * 0.02), tone(-38.0 + i as f32, 0.4), silence(0.4 - i as f32 * 0.02)]);
        m.reset_max();
        m.push(&[&s, &s]);
        close(m.max_momentary_lufs(), -38.0 + i as f32, 0.1, &format!("segment {i}"));
    }
}

fn lra_of(levels: &[f32]) -> f32 {
    let s: Vec<f32> = levels.iter().flat_map(|&l| tone(l, 20.0)).collect();
    measure(&s).loudness_range_lu()
}

/// Tech 3342 table 1, cases 1 to 4, each 20 s per tone, +/- 1 LU.
#[test]
fn tech3342_cases_1_to_4() {
    close(lra_of(&[-20.0, -30.0]), 10.0, 1.0, "case 1");
    close(lra_of(&[-20.0, -15.0]), 5.0, 1.0, "case 2");
    close(lra_of(&[-40.0, -20.0]), 20.0, 1.0, "case 3");
    close(lra_of(&[-50.0, -35.0, -20.0, -35.0, -50.0]), 15.0, 1.0, "case 4");
}

/// "The expected response is unchanged if the test signal is repeated."
#[test]
fn tech3342_repeat_is_unchanged() {
    let one: Vec<f32> = [-20.0f32, -30.0].iter().flat_map(|&l| tone(l, 20.0)).collect();
    let two = [one.clone(), one].concat();
    close(measure(&two).loudness_range_lu(), 10.0, 1.0, "repeat");
}

#[test]
fn ebu_meter_reads_relative_and_absolute_and_resets() {
    let mut m = EbuMeter::stereo(FS);
    let t = tone(-20.0, 10.0);
    m.push(&[&t, &t]);
    close(m.integrated_lufs(), -20.0, 0.1, "abs");
    close(m.integrated_lu(), 3.0, 0.1, "rel");
    close(m.momentary_lu(), 3.0, 0.1, "rel M");
    close(m.max_true_peak_dbtp(), -20.0, 0.4, "true peak");
    assert!(m.scale_position(3.0).unwrap() > 0.77 && m.scale_position(3.0).unwrap() < 0.79);
    assert_eq!(m.scale().range_lu(), (-18.0, 9.0));
    // Pause: integrated holds while a loud passage goes by.
    m.set_running(false);
    let loud = tone(-10.0, 5.0);
    m.push(&[&loud, &loud]);
    close(m.integrated_lufs(), -20.0, 0.1, "held");
    assert!(m.loudness().max_momentary_lufs() > -11.0, "maxima follow in stand-by");
    m.set_running(true);
    m.reset();
    assert_eq!(m.integrated_lufs(), f32::NEG_INFINITY);
    assert_eq!(m.loudness_range_lu(), 0.0);
    assert!(openplugin_dsp::Target::EBU_R128.met_by(-23.2, -1.5));
    assert!(!openplugin_dsp::Target::EBU_R128.met_by(-22.0, -1.5));
    assert!(!openplugin_dsp::Target::ATSC_A85.met_by(-24.0, -1.0));
}
