use openplugin_core::{AutomationLane, LaneMode};

fn assert_punched(lane: &AutomationLane) {
    let expect = [
        (0_u64, 0.20_f32),
        (3008, 0.2376),
        (3072, 0.55),
        (3136, 0.80),
        (48_000, 0.80),
    ];
    assert_eq!(lane.points().len(), expect.len());
    for &(sample, value) in &expect {
        let got = lane
            .points()
            .iter()
            .find(|point| point.0 == sample)
            .map(|point| point.1)
            .unwrap_or_else(|| panic!("no point at {sample}"));
        assert!(
            (got - value).abs() < 1e-4,
            "point {sample} = {got}, expected {value}"
        );
    }
}

fn punch(mode: LaneMode, touch: bool) -> AutomationLane {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    lane.insert_point(0, 0.20);
    lane.insert_point(48_000, 0.80);
    lane.set_mode(mode);
    if touch {
        lane.touch(true);
    }
    lane.set_rolling(true);
    lane.set_playhead(3072);
    lane.set_hand(0.55);
    lane
}

#[test]
fn write_punches_without_touch() {
    assert_punched(&punch(LaneMode::Write, false));
}

#[test]
fn touch_punches_when_held_before_roll() {
    assert_punched(&punch(LaneMode::Touch, true));
}

#[test]
fn latch_punches_without_touch() {
    assert_punched(&punch(LaneMode::Latch, false));
}
