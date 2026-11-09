use openplugin_core::{AutomationLane, LaneMode};

#[test]
fn manual_roll_keeps_heard_and_points() {
    let mut lane = AutomationLane::new(0.0, 1.0, 0.0);
    assert_eq!(lane.mode(), LaneMode::Manual);

    lane.insert_point(0, 0.20);
    lane.insert_point(1000, 0.80);
    lane.insert_point(2000, 0.40);

    lane.set_hand(0.33);
    assert!((lane.heard() - 0.33).abs() < 1e-4);

    lane.set_rolling(true);

    assert_eq!(lane.mode(), LaneMode::Manual);
    assert!(
        (lane.heard() - 0.33).abs() < 1e-4,
        "heard {} is not within 1e-4 of 0.33",
        lane.heard()
    );

    let expect = [(0_u64, 0.20_f32), (1000, 0.80), (2000, 0.40)];
    assert_eq!(lane.points().len(), expect.len());
    for (index, &(sample, value)) in expect.iter().enumerate() {
        let got = lane.points()[index];
        assert_eq!(got.0, sample);
        assert!(
            (got.1 - value).abs() < 1e-4,
            "point {sample} = {}, expected {value}",
            got.1
        );
    }
}
