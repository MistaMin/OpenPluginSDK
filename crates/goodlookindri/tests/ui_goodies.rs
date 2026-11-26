use goodlookindri::{layout::*, readout::ReadoutWindow, scene::Rect};
#[test]
fn readout_window() {
    for rate in [44100.0, 48000.0, 96000.0] {
        let mut a = ReadoutWindow::new(rate);
        assert_eq!(a.loudness(), -200.0);
        a.add(-20.0, 12.0, 1000);
        a.add(-10.0, 6.0, 1000);
        assert!((a.loudness() - -12.596373).abs() < 0.0001);
        assert_eq!(a.crest(), 12.0);
        a.add(-70.0, f32::NAN, 100);
        assert!((a.loudness() - -12.596373).abs() < 0.0001);
        a.add(f32::INFINITY, f32::NAN, (rate * 2.1) as u64);
        assert_eq!(a.loudness(), -200.0);
        assert_eq!(a.crest(), -100.0);
        a.add(-18.0, 7.0, 100);
        a.reset();
        assert_eq!(a.loudness(), -200.0);
        let mut b = ReadoutWindow::new(rate);
        a.add(-23.0, 9.0, 12345);
        for _ in 0..12345 {
            b.add(-23.0, 9.0, 1);
        }
        assert!((a.loudness() - b.loudness()).abs() < 0.0001);
        assert_eq!(a.crest(), b.crest());
    }
    assert_eq!(ReadoutWindow::new(f64::NAN).loudness(), -200.0);
}
#[test]
fn bounded_layouts() {
    for h in [0.0, 12.0, 80.0, 400.0] {
        let panel = Rect::new(10.0, 20.0, 100.0, h);
        let mut bottom = panel.y;
        for i in 0..5 {
            let card = readout_card(panel, i, 5, 8.0).unwrap();
            assert!(card.y >= bottom - 0.001 && card.bottom() <= panel.bottom() + 0.001);
            assert!(card.x >= panel.x && card.right() <= panel.right());
            bottom = card.bottom();
        }
    }
    assert!(readout_card(Rect::new(0.0, 0.0, 100.0, 100.0), 0, 0, 8.0).is_none());
    assert!(readout_card(Rect::new(0.0, 0.0, -1.0, 100.0), 0, 5, 8.0).is_none());
    for width in [0.0, 10.0, 100.0] {
        let (caption, switch) =
            caption_switch(Rect::new(0.0, 0.0, width, 20.0), 24.0, 8.0).unwrap();
        assert!(caption.right() <= switch.x && switch.right() <= width);
    }
}
