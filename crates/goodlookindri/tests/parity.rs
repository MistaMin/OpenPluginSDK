use goodlookindri::design::*;
use goodlookindri::level::LevelTracker;
use goodlookindri::looks::{Look, LookTable};
use goodlookindri::settings::SettingsTable;
use goodlookindri::spectrum::*;
use goodlookindri::water::{brit_knob, panel, WaterResult};
use goodlookindri::scene::WaterCmd;

fn near(a: f64, b: f64, t: f64) -> bool { (a - b).abs() <= t }

#[test]
fn design_roundtrip_and_motion() {
    let mut i = Item::new("gain", "preampGain", "Gain, \"input\"", "console", "#AAbb01", 12.0, 15.0, 70.0, 76.0, 11.0);
    let file = write_design(&[i.clone()]).unwrap();
    assert_eq!(read_design(&file).unwrap(), vec![i.clone()]);
    let legacy = file.replacen("GoodLookinUI", "HardwareUI", 1);
    assert_eq!(read_design(&legacy).unwrap(), vec![i.clone()]);
    assert!(read_design("GoodLookinUI design version 2\n").is_err());
    assert_eq!(find_style("metal").unwrap().style, KnobStyle::Mtl);
    assert_eq!(find_style("bakelite").unwrap().style, KnobStyle::Bk);
    assert_eq!(find_style("ivory").unwrap().style, KnobStyle::Slv);
    assert_eq!(find_style("console").unwrap().style, KnobStyle::Brit);
    assert!(find_style("bogus").is_none());
    assert!(find_faceplate("Navy").is_some() && find_faceplate("Graphite").is_some());
    assert!(find_faceplate("Nope").is_none());
    assert_eq!(FACEPLATES.len(), 40);
    assert_eq!(KNOB_STYLES.len(), 47);
    let banned = ["neve","ssl","api","focusrite","pultec","teletronix","dbx","spl","1176","la-2a","corvette","honda","commodore","arasaka","militech","netwatch","kiroshi","edgerunner","radial","crawford"];
    for f in FACEPLATES {
        let s = format!("{} {}", f.code, f.name).to_lowercase();
        assert!((0..=7).contains(&f.finish));
        assert!(banned.iter().all(|w| !s.contains(w)));
    }
    for k in KNOB_STYLES {
        let s = format!("{} {}", k.code, k.description).to_lowercase();
        assert!(banned.iter().all(|w| !s.contains(w)));
    }
    let original = format!("GoodLookinUI design version 1\n{HEADER_V1}\nlcFreq,lcFreq,FREQ,console,#CBCBBC,12,52,150,98,10\nlcQ,lcQ,Q,console,#CBCBBC,12,152,150,98,10\n");
    let items = read_design(&original).unwrap();
    assert_eq!(write_design(&items).unwrap(), original);
    i.marks = "dark".into();
    let with = write_design(&[i.clone()]).unwrap();
    assert!(with.contains("marks"));
    assert_eq!(read_design(&with).unwrap()[0].marks, "dark");
    let mut m = Motion::default();
    for _ in 0..600 { m.step(1.0, 1.0/60.0, 18.0); assert!(m.position >= 0.0 && m.position <= 1.00001); }
    assert!((m.position - 1.0).abs() < 1e-8);
}

#[test]
fn settings_and_looks() {
    let mut t = SettingsTable::default();
    t.set("meterFace", "neonbars");
    t.set("note", "has \"quotes\", commas");
    t.set("", "x");
    t.set("bad\nkey", "x");
    assert_eq!(t.all().len(), 2);
    let mut u = SettingsTable::default();
    assert!(u.from_csv(&t.to_csv()));
    assert_eq!(u, t);
    let mut a = SettingsTable::default();
    let mut b = SettingsTable::default();
    a.set("z", "1"); a.set("a", "2"); b.set("a", "2"); b.set("z", "1");
    assert_eq!(a.to_csv(), b.to_csv());
    assert!(a.to_csv().find("\"a\",\"2\"").unwrap() < a.to_csv().find("\"z\",\"1\"").unwrap());

    let defaults = vec![
        Look { parameter: "mid1Type".into(), choice: "N-EQ".into(), colour: "#5793B8".into(), faceplate: "".into(), style: "N".into(), plate: "Marine".into() },
        Look { parameter: "preampType".into(), choice: "Brit".into(), colour: "#C96F4A".into(), faceplate: "Silver".into(), style: "Brit".into(), plate: "Granite".into() },
    ];
    let mut table = LookTable::from_defaults(defaults.clone());
    let mut e = table.find("mid1Type", "N-EQ").unwrap().clone();
    e.style = "Pie".into(); e.colour = "#12AB34".into();
    assert!(table.set(e));
    assert!(table.find("preampType", "Brit").unwrap().style == "Brit");
    let mut bad = table.find("mid1Type", "N-EQ").unwrap().clone();
    bad.style = "Nope".into();
    assert!(!table.set(bad));
    let mut u = LookTable::from_defaults(defaults.clone());
    assert!(u.from_csv(&table.to_csv()));
    assert_eq!(u.all(), table.all());
    table.reset_entry("mid1Type", "N-EQ");
    assert_eq!(table.find("mid1Type", "N-EQ").unwrap().style, "N");
}

#[test]
fn level_tracker_peak_and_power() {
    let t = LevelTracker::new();
    let n = 4800;
    let l: Vec<f32> = (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin()).collect();
    let r: Vec<f32> = (0..n).map(|i| if i % 2 == 0 { -0.25 } else { 0.25 }).collect();
    t.push(&[&l, &r]);
    let a = t.read();
    assert!(near(a.peak[0] as f64, 0.5, 0.001));
    assert!(near(a.peak[1] as f64, 0.25, 1e-6));
    assert!(near(a.mean_square[0] as f64, 0.125, 0.002));
    assert!(near(a.mean_square[1] as f64, 0.0625, 1e-5));
    let b = t.read();
    assert_eq!(b.peak[0], 0.0);
}

#[test]
fn fft_matches_dft_and_sine_calibration() {
    for &n in &[16usize, 64, 512] {
        let mut x = vec![0.0f32; n];
        let mut state = 7u32;
        for v in &mut x { state = state.wrapping_mul(1664525).wrapping_add(1013904223); *v = (state >> 8) as f32 / u32::MAX as f32 * 2.0 - 1.0; }
        let mut fft = RealFft::new(n);
        let mut re = vec![0.0; n / 2 + 1];
        let mut im = vec![0.0; n / 2 + 1];
        fft.forward(&x, &mut re, &mut im);
        let mut worst = 0.0f64;
        for k in (0..=n / 2).step_by(if n > 512 { 17 } else { 1 }) {
            let mut sr = 0.0; let mut si = 0.0;
            for t in 0..n {
                let ang = -2.0 * std::f64::consts::PI * k as f64 * t as f64 / n as f64;
                sr += x[t] as f64 * ang.cos();
                si += x[t] as f64 * ang.sin();
            }
            worst = worst.max(((sr - re[k] as f64).powi(2) + (si - im[k] as f64).powi(2)).sqrt());
        }
        assert!(worst < 2e-3 * (n as f64).sqrt(), "n {n} worst {worst}");
    }
    for &n in &FFT_SIZES {
        let n = n as usize;
        let mut a = Analyser::default();
        a.configure(n, 48000.0);
        let bin = n / 8;
        let mut s = vec![0.0f32; n];
        for &amp in &[1.0f64, 0.5, 0.1] {
            a.reset();
            for i in 0..n { s[i] = (amp * (2.0 * std::f64::consts::PI * bin as f64 * i as f64 / n as f64).sin()) as f32; }
            a.process(&s);
            assert!(near(a.display_db()[bin] as f64, 20.0 * amp.log10(), 0.05), "n {n} amp {amp} got {}", a.display_db()[bin]);
        }
    }
}

#[test]
fn columns_colours_smooth_history() {
    let mut a = Analyser::default();
    a.configure(4096, 48000.0);
    a.set_tilt(4.5);
    let b2 = 171;
    let mut s = vec![0.0f32; 4096];
    for i in 0..4096 { s[i] = (0.2 * (2.0 * std::f64::consts::PI * b2 as f64 * i as f64 / 4096.0).sin()) as f32; }
    a.process(&s);
    let f2 = b2 as f64 * 48000.0 / 4096.0;
    assert!(near(a.display_db()[b2] as f64, 20.0 * 0.2f64.log10() + 4.5 * (f2 / 1000.0).log2(), 0.15));

    assert_eq!(colour_at((RANGES[0].lo * RANGES[0].hi).sqrt(), &DEFAULT_COLOURS), DEFAULT_COLOURS[0]);
    assert_eq!(colour_at(20.0, &DEFAULT_COLOURS), DEFAULT_COLOURS[0]);
    assert_eq!(range_index_for(1000.0), 3);

    let mut h = SpectrogramHistory::new();
    h.configure(4, 3);
    h.push(&[-90.0, -60.0, -30.0, 0.0], -90.0, 0.0);
    assert_eq!(h.row(0)[0], 0);
    assert_eq!(h.row(0)[3], 255);
    assert_eq!(heat_colour(0), 0xff000000);
}

#[test]
fn every_knob_style_emits_drawing() {
    assert!(goodlookindri::gallery::every_style_draws(false));
    assert!(goodlookindri::gallery::every_style_draws(true));
    for page in 0..6 {
        let mut s = goodlookindri::scene::Scene::new();
        goodlookindri::gallery::paint_page(&mut s, page, false, true);
        if page < 5 { assert!(!s.cmds.is_empty(), "page {page}"); }
    }
    let settings = goodlookindri::spectrum_draw::SpectrumSettings::default();
    let text = settings.to_string();
    let back = goodlookindri::spectrum_draw::SpectrumSettings::from_string(&text);
    assert_eq!(back.style, settings.style);
    assert!((back.tilt - settings.tilt).abs() < 1e-4);
}

#[test]
fn water_canvas_refuses_unported_styles() {
    let mut cmds = Vec::new();
    let item = Item::new("g", "g", "G", "Brit", "#BFA878", 0.0, 0.0, 80.0, 80.0, 10.0);
    assert_eq!(brit_knob(&mut cmds, goodlookindri::scene::Rect::new(0.0, 0.0, 100.0, 100.0), 0.4, &item, true), WaterResult::Ok);
    assert!(cmds.iter().any(|c| matches!(c, WaterCmd::Line { .. })));
    let mut pie = item.clone();
    pie.style = "Pie".into();
    assert_eq!(brit_knob(&mut cmds, goodlookindri::scene::Rect::new(0.0, 0.0, 100.0, 100.0), 0.4, &pie, true), WaterResult::UnsupportedStyle);
    let plate = find_faceplate("Graphite").unwrap();
    assert_eq!(panel(&mut cmds, goodlookindri::scene::Rect::new(0.0, 0.0, 100.0, 100.0), plate), WaterResult::Ok);
}
