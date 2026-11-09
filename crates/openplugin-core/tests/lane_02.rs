use openplugin_core::AutomationLane;

#[test]
fn empty_curve_stays_at_initial_after_hand() {
    let mut lane = AutomationLane::new(0.0, 2.0, 1.0);
    lane.set_hand(0.25);
    assert!((lane.curve(0) - 1.0).abs() < 1e-4);
    assert!((lane.hand() - 0.25).abs() < 1e-4);
}
