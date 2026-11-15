use crate::manifest::*;
use crate::snippet::*;

fn param(points: &[(f64, f64, &str)], min: f64, max: f64) -> Param {
    Param {
        id: 7,
        name: "Cutoff".into(),
        min,
        max,
        default: min,
        flags: vec!["automatable".into()],
        samples: points.iter().map(|&(n, p, t)| Sample { normalized: n, plain: p, text: t.into() }).collect(),
        ..Param::default()
    }
}

#[test]
fn value_strings_parse() {
    assert_eq!(parse_value("-12.5 dB"), Some((-12.5, "dB".into())));
    assert_eq!(parse_value("1.5 kHz"), Some((1500.0, "Hz".into())));
    assert_eq!(parse_value("50%"), Some((50.0, "%".into())));
    assert_eq!(parse_value("4:1"), Some((4.0, ":1".into())));
    assert_eq!(parse_value("-inf dB"), None);
    assert_eq!(parse_value("Off"), None);
}

#[test]
fn a_line_and_a_curve_are_told_apart() {
    let line: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 10.0 + 90.0 * t)).collect();
    let f = fit(&line).unwrap();
    assert!(f.linear_error < 1e-9 && !is_log(&f));
    let curve: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 20.0 * (1000.0f64).powf(t))).collect();
    let f = fit(&curve).unwrap();
    assert!(f.log_error.unwrap() < 1e-9 && is_log(&f));
}

#[test]
fn a_plain_log_range_gives_a_log_taper() {
    let owned: Vec<(f64, f64)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, 20.0 * (1000.0f64).powf(t))).collect();
    let pts: Vec<(f64, f64, &str)> = owned.iter().map(|&(a, b)| (a, b, "")).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 20.0, 20000.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Log");
    assert_eq!((i[0].min, i[0].max), (20.0, 20000.0));
}

#[test]
fn value_strings_reveal_a_hidden_log_display() {
    // Plain 0..1, linear in the position, but the text runs 20 Hz to 20 kHz on a curve.
    let texts = ["20 Hz", "112 Hz", "632 Hz", "3.56 kHz", "20 kHz"];
    let pts: Vec<(f64, f64, &str)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().zip(texts).map(|(&t, s)| (t, t, s)).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 0.0, 1.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Log");
    assert_eq!((i[0].min, i[0].max), (20.0, 20000.0));
    assert_eq!(i[0].unit, "Hz");
}

#[test]
fn no_text_means_a_commented_linear_guess() {
    let pts: Vec<(f64, f64, &str)> = [0.0, 0.25, 0.5, 0.75, 1.0].iter().map(|&t| (t, t, "")).collect();
    let i = infer(&Plugin { params: vec![param(&pts, 0.0, 1.0)], ..Plugin::default() });
    assert_eq!(i[0].taper, "Linear");
    assert!(i[0].notes.iter().any(|n| n.contains("guessed")));
}

#[test]
fn choices_toggles_and_unique_ids() {
    let mut choice = param(&[(0.0, 0.0, ""), (1.0, 2.0, "")], 0.0, 2.0);
    choice.name = "Mode".into();
    choice.step_count = 2;
    choice.step_texts = vec!["Clean".into(), "Warm".into(), "Hot".into()];
    let mut toggle = param(&[(0.0, 0.0, "Off"), (1.0, 1.0, "On")], 0.0, 1.0);
    toggle.name = "Mode".into();
    toggle.step_count = 1;
    toggle.step_texts = vec!["Off".into(), "On".into()];
    let i = infer(&Plugin { params: vec![choice, toggle], ..Plugin::default() });
    assert_eq!((i[0].unit, i[0].choices.len(), i[0].max), ("Choice", 3, 2.0));
    assert_eq!(i[1].unit, "Toggle");
    assert_eq!((i[0].id.as_str(), i[1].id.as_str()), ("mode", "mode_2"));
}
