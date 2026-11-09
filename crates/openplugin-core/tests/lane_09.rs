use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn entering_play_does_not_replace_heard() {
    let mut lane = AutomationLane::new(0.0, 1.0, 1.0);
    lane.insert_point(0, 0.20);
    assert!((lane.curve(0) - 0.20).abs() < 1e-4);
    assert!((lane.heard() - 1.0).abs() < 1e-4);

    lane.set_mode(LaneMode::Play);

    assert_eq!(lane.mode(), LaneMode::Play);
    assert!((lane.curve(0) - 0.20).abs() < 1e-4);
    assert!((lane.heard() - 1.0).abs() < 1e-4);
}
