use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn play_roll_ignores_gesture_and_stays_play() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.80);
    let points = [(0_u64, 0.20_f32), (48_000, 0.80)];
    assert_eq!(lane.points(), &points);

    lane.set_mode(LaneMode::Play);
    lane.set_rolling(true);
    lane.set_playhead(6144);

    assert!((lane.heard() - 0.2768).abs() < 1e-4);
    assert!((lane.curve(6144) - 0.2768).abs() < 1e-4);
    assert!((lane.heard() - lane.curve(6144)).abs() < 1e-4);

    lane.set_hand(0.11);
    assert!((lane.heard() - 0.2768).abs() < 1e-4);
    assert_eq!(lane.points().len(), 2);
    assert_eq!(lane.points(), &points);

    lane.set_rolling(false);
    assert_eq!(lane.mode(), LaneMode::Play);
    assert_eq!(lane.points(), &points);
}
