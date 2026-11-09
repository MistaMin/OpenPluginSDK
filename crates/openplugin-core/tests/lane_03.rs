use openplugin_core::AutomationLane;

#[test]
fn manual_hand_clamps_to_limits_and_ignores_nan() {
    let mut lane = AutomationLane::new(0.0, 2.0, 0.0);
    let mode = lane.mode();

    lane.set_hand(3.0);
    assert!((lane.heard() - 2.0).abs() < 1e-4);

    lane.set_hand(-1.0);
    assert!((lane.heard() - 0.0).abs() < 1e-4);

    lane.set_hand(f32::INFINITY);
    assert!((lane.heard() - 2.0).abs() < 1e-4);

    lane.set_hand(f32::NEG_INFINITY);
    assert!((lane.heard() - 0.0).abs() < 1e-4);

    lane.set_hand(f32::NAN);
    assert!((lane.heard() - 0.0).abs() < 1e-4);

    assert_eq!(lane.mode(), mode);
}
