use openplugin_core::{AutomationLane, CurveInterpolation, LaneMode};

#[test]
fn new_lane_starts_manual_linear_and_clamps() {
    let lane = AutomationLane::new(0.0, 1.0, 0.25);
    assert_eq!(lane.mode(), LaneMode::Manual);
    assert_eq!(lane.interpolation(), CurveInterpolation::Linear);
    assert!(!lane.touching());
    assert!(!lane.is_rolling());
    assert_eq!(lane.playhead(), 0);
    assert!(lane.points().is_empty());
    assert!((lane.hand() - 0.25).abs() < 1e-4);
    assert!((lane.heard() - 0.25).abs() < 1e-4);

    let high = AutomationLane::new(0.0, 1.0, 2.0);
    assert!((high.hand() - 1.0).abs() < 1e-4);
    assert!((high.heard() - 1.0).abs() < 1e-4);

    let swapped = AutomationLane::new(1.0, 0.0, 0.4);
    assert!((swapped.lower() - 0.0).abs() < 1e-4);
    assert!((swapped.upper() - 1.0).abs() < 1e-4);
    assert!((swapped.heard() - 0.4).abs() < 1e-4);
}
