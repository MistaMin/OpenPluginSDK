use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn touch_release_while_rolling_hears_curve_and_stays_touch() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.2);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.20);

    lane.set_mode(LaneMode::Touch);
    lane.touch(true);
    lane.set_rolling(true);
    lane.set_playhead(2048);
    lane.set_hand(0.60);
    assert!((lane.heard() - 0.60).abs() < 1e-4);

    lane.set_playhead(3000);
    assert!((lane.heard() - 0.60).abs() < 1e-4);

    lane.touch(false);
    assert!(lane.is_rolling());
    assert!((lane.curve(3000) - 0.20).abs() < 1e-4);
    assert!((lane.heard() - lane.curve(3000)).abs() < 1e-4);
    assert!((lane.heard() - 0.20).abs() < 1e-4);

    lane.set_rolling(false);
    assert_eq!(lane.mode(), LaneMode::Touch);
    assert!(!lane.is_rolling());
}
