use openplugin_core::{AutomationLane, LaneMode};

fn point_at(lane: &AutomationLane, sample: u64) -> Option<f32> {
    lane.points()
        .iter()
        .find(|point| point.0 == sample)
        .map(|point| point.1)
}

#[test]
fn stopped_touch_replaces_the_point_under_the_playhead() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.2);
    lane.insert_point(0, 0.20);
    lane.insert_point(1000, 0.80);
    lane.set_mode(LaneMode::Touch);
    lane.touch(true);
    assert_eq!(lane.playhead(), 0);
    lane.set_hand(0.30);

    let at_zero = point_at(&lane, 0).expect("point at 0");
    assert!((at_zero - 0.30).abs() < 1e-4);
    assert!(point_at(&lane, 64).is_none());
    assert!((lane.heard() - 0.30).abs() < 1e-4);
    assert!(!lane.is_rolling());
    let at_thousand = point_at(&lane, 1000).expect("point at 1000");
    assert!((at_thousand - 0.80).abs() < 1e-4);
}
