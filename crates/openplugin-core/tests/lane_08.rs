use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn stopped_play_locate_follows_curve() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(1000, 0.80);
    lane.set_mode(LaneMode::Play);
    assert!(!lane.is_rolling());

    lane.set_playhead(500);
    assert!(!lane.is_rolling());
    assert!((lane.heard() - lane.curve(500)).abs() < 1e-4);
    assert!((lane.heard() - 0.50).abs() < 1e-4);

    lane.set_playhead(0);
    assert!(!lane.is_rolling());
    assert!((lane.heard() - lane.curve(0)).abs() < 1e-4);
    assert!((lane.heard() - 0.20).abs() < 1e-4);
}
