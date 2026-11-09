use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn write_roll_keeps_hand_set_before_mode_then_hears_curve() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.80);
    let points = [(0_u64, 0.20_f32), (48_000, 0.80)];

    lane.set_hand(0.30);
    lane.set_mode(LaneMode::Write);
    lane.set_rolling(true);
    lane.set_playhead(8192);

    assert!((lane.heard() - 0.30).abs() < 1e-4);
    assert_eq!(lane.points(), &points);

    lane.set_rolling(false);
    assert_eq!(lane.mode(), LaneMode::Touch);
    assert!((lane.heard() - lane.curve(8192)).abs() < 1e-4);
    assert!((lane.curve(8192) - 0.3024).abs() < 1e-4);
    assert_eq!(lane.points(), &points);
}
