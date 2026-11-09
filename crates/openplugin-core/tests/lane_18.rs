use openplugin_core::{AutomationLane, LaneMode};

fn rolling_write_at_3072() -> AutomationLane {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.2);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.80);
    lane.set_mode(LaneMode::Write);
    lane.set_rolling(true);
    lane.set_playhead(3072);
    lane
}

fn point_value(lane: &AutomationLane, sample: u64) -> f32 {
    lane.points()
        .iter()
        .find(|point| point.0 == sample)
        .map(|point| point.1)
        .unwrap_or_else(|| panic!("no point at {sample}"))
}

#[test]
fn nan_while_rolling_in_write_does_not_add_points_or_change_heard() {
    let mut lane = rolling_write_at_3072();
    let heard = lane.heard();
    let len = lane.points().len();
    lane.set_hand(f32::NAN);
    assert_eq!(lane.points().len(), len);
    assert!((lane.heard() - heard).abs() < 1e-4);
}

#[test]
fn finite_gesture_above_upper_limit_is_stored_clamped() {
    let mut lane = rolling_write_at_3072();
    lane.set_hand(1.5);
    let got = point_value(&lane, 3072);
    assert!((got - 1.0).abs() < 1e-4, "point 3072 = {got}, expected 1.0");
    assert!(lane.points().iter().any(|point| point.0 == 3008));
    let end = point_value(&lane, 48_000);
    assert!(
        (end - 0.80).abs() < 1e-4,
        "point 48000 = {end}, expected 0.80"
    );
}
