use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn latch_holds_hand_until_stop_on_flat_line() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.2);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.20);

    lane.set_mode(LaneMode::Latch);
    lane.set_rolling(true);
    lane.set_playhead(2048);
    lane.set_hand(0.50);

    lane.set_playhead(8000);
    assert!((lane.heard() - 0.50).abs() < 1e-4);
    assert!(lane.is_rolling());

    lane.set_rolling(false);
    assert_eq!(lane.mode(), LaneMode::Latch);
    assert!((lane.curve(8000) - 0.20).abs() < 1e-4);
    assert!((lane.heard() - lane.curve(8000)).abs() < 1e-4);
}
