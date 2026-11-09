use openplugin_core::{AutomationLane, CurveInterpolation};

#[test]
fn single_point_holds_for_linear_and_discrete() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(1000, 0.80);

    for interpolation in [CurveInterpolation::Linear, CurveInterpolation::Discrete] {
        lane.set_interpolation(interpolation);
        for sample in [0_u64, 500, 1000, 2000] {
            let got = lane.curve(sample);
            assert!(
                (got - 0.80).abs() < 1e-4,
                "{interpolation:?} curve({sample}) = {got}"
            );
        }
        assert_eq!(lane.points().len(), 1);
        assert_eq!(lane.points()[0].0, 1000);
        assert!((lane.points()[0].1 - 0.80).abs() < 1e-4);
    }
}
