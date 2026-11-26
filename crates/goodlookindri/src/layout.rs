//! Bounded layouts for readout cards and clearly associated caption/switch pairs.
use crate::scene::Rect;
/// Returns one equal-height card with equal outer/inter-card spacing.
/// Gap is reduced to fit small panels; invalid geometry or indices return None.
pub fn readout_card(panel: Rect, index: usize, count: usize, gap: f32) -> Option<Rect> {
    if count == 0
        || index >= count
        || ![panel.x, panel.y, panel.w, panel.h, gap]
            .iter()
            .all(|v| v.is_finite())
        || panel.w < 0.0
        || panel.h < 0.0
        || gap < 0.0
    {
        return None;
    }
    let n = count as f32;
    let g = gap.min(panel.h / (n + 1.0)).min(panel.w * 0.5);
    let h = ((panel.h - g * (n + 1.0)) / n).max(0.0);
    Some(Rect::new(
        panel.x + g,
        panel.y + g + index as f32 * (h + g),
        (panel.w - 2.0 * g).max(0.0),
        h,
    ))
}
/// Label on the left, switch on the right; both remain inside their own row.
pub fn caption_switch(row: Rect, switch_width: f32, gap: f32) -> Option<(Rect, Rect)> {
    if ![row.x, row.y, row.w, row.h, switch_width, gap]
        .iter()
        .all(|v| v.is_finite())
        || row.w < 0.0
        || row.h < 0.0
        || switch_width < 0.0
        || gap < 0.0
    {
        return None;
    }
    let w = switch_width.min(row.w);
    let label_w = (row.w - w - gap).max(0.0);
    Some((
        Rect::new(row.x, row.y, label_w, row.h),
        Rect::new(row.right() - w, row.y, w, row.h),
    ))
}
