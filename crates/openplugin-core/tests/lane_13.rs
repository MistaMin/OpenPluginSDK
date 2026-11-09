use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn touch_up_while_rolling_hears_curve_not_hand() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(1000, 0.80);
    lane.set_mode(LaneMode::Touch);
    lane.set_rolling(true);
    lane.set_playhead(500);
    let curve = lane.curve(500);
    lane.set_hand(0.70);
    assert_eq!(lane.points().len(), 2);
    assert!((curve - 0.50).abs() < 1e-4);
    assert!((lane.heard() - curve).abs() < 1e-4);
    assert!((lane.heard() - 0.50).abs() < 1e-4);
    assert!((lane.heard() - 0.70).abs() >= 1e-4);
}
