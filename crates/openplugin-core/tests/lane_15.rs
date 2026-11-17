use openplugin_core::{AutomationLane, AutomationMode};

#[test]
fn latch_without_gesture_hears_the_curve() {
    let mut lane = AutomationLane::new(0.0, 1.0, 1.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.80);
    assert_eq!(lane.points().len(), 2);

    lane.set_mode(AutomationMode::Latch);
    lane.set_rolling(true);
    lane.set_playhead(3072);

    assert!((lane.heard() - 0.2384).abs() < 1e-4);
    assert_eq!(lane.points().len(), 2);

    lane.set_rolling(false);
    assert_eq!(lane.mode(), AutomationMode::Latch);
    assert_eq!(lane.points().len(), 2);
}
