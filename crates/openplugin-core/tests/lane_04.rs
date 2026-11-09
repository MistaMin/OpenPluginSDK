use openplugin_core::{AutomationLane, CurveInterpolation};

fn assert_curve(lane: &AutomationLane, samples: &[(u64, f32)]) {
    for &(sample, value) in samples {
        let got = lane.curve(sample);
        assert!(
            (got - value).abs() < 1e-4,
            "curve({sample}) = {got}, expected {value}"
        );
    }
}

#[test]
fn linear_default_then_discrete_curve() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(1000, 0.80);
    lane.insert_point(2000, 0.40);

    assert_curve(
        &lane,
        &[
            (0, 0.20),
            (500, 0.50),
            (999, 0.7994),
            (1000, 0.80),
            (1500, 0.60),
            (1999, 0.4004),
            (2000, 0.40),
            (2500, 0.40),
        ],
    );

    lane.set_interpolation(CurveInterpolation::Discrete);
    assert_curve(
        &lane,
        &[
            (0, 0.20),
            (500, 0.20),
            (999, 0.20),
            (1000, 0.80),
            (1500, 0.80),
            (1999, 0.80),
            (2000, 0.40),
            (2500, 0.40),
        ],
    );
}
