use openplugin_core::{AutomationLane, LaneMode};

fn flat_lane() -> AutomationLane {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.2);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.20);
    lane
}

fn punch_three(lane: &mut AutomationLane) {
    for sample in [2048_u64, 4096, 6144] {
        lane.set_playhead(sample);
        lane.set_hand(0.50);
    }
}

#[test]
fn write_touch_stays_up_and_curve_at_3000_stays_flat() {
    let mut lane = flat_lane();
    lane.set_mode(LaneMode::Write);
    assert!(!lane.touching());
    lane.set_rolling(true);
    punch_three(&mut lane);
    assert!(!lane.touching());
    assert!((lane.curve(3000) - 0.20).abs() < 1e-4);
}

#[test]
fn touch_stays_held_for_three_gestures_and_curve_at_3000_stays_flat() {
    let mut lane = flat_lane();
    lane.set_mode(LaneMode::Touch);
    lane.touch(true);
    assert!(lane.touching());
    lane.set_rolling(true);
    for sample in [2048_u64, 4096, 6144] {
        assert!(lane.touching());
        lane.set_playhead(sample);
        lane.set_hand(0.50);
        assert!(lane.touching());
    }
    assert!((lane.curve(3000) - 0.20).abs() < 1e-4);
}

#[test]
fn latch_touch_stays_up_and_curve_at_3000_stays_flat() {
    let mut lane = flat_lane();
    lane.set_mode(LaneMode::Latch);
    assert!(!lane.touching());
    lane.set_rolling(true);
    punch_three(&mut lane);
    assert!(!lane.touching());
    assert!((lane.curve(3000) - 0.20).abs() < 1e-4);
}
