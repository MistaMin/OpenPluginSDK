//! Outline text for [`goodlookindri::scene::Cmd::Text`], plus measuring, wrapping and ellipsis
//! for widgets.
//!
//! The face is Source Sans Pro Regular, unmodified, under the SIL Open Font License 1.1
//! (`fonts/OFL.txt`). `fontdue` rasterises it. Sizes are in scene units, the same as
//! `Cmd::Text::size`; the rasteriser passes `size * scale` as pixels.
//!
//! Nothing here uses thread-local storage, so the code still loads on macOS 10.6.

use fontdue::{Font, FontSettings};
use std::collections::HashMap;
use std::sync::OnceLock;

const FACE: &[u8] = include_bytes!("../fonts/SourceSansPro-Regular.otf");

/// Where the anchor of `Cmd::Text` sits, as a share of the pixel size below the baseline. This is
/// about half the cap height, so a line centred on `at.y` looks centred.
pub const MIDDLE: f32 = 0.35;

pub fn face() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| Font::from_bytes(FACE, FontSettings::default()).expect("bundled Source Sans Pro"))
}

/// Glyph sizes are cached per 1/16 pixel, so a size that differs only by float noise reuses one bitmap.
pub fn quantise(px: f32) -> f32 {
    (px.max(0.0) * 16.0).round() / 16.0
}

/// Pen positions for each character, with kerning, and the total advance, all in pixels.
fn layout(text: &str, px: f32) -> (Vec<(char, f32)>, f32) {
    let font = face();
    let mut pen = 0.0;
    let mut prev: Option<char> = None;
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        if let Some(p) = prev {
            pen += font.horizontal_kern(p, c, px).unwrap_or(0.0);
        }
        out.push((c, pen));
        pen += font.metrics(c, px).advance_width;
        prev = Some(c);
    }
    (out, pen)
}

/// Advance width of `text` at `size`, in scene units.
pub fn width(text: &str, size: f32) -> f32 {
    layout(text, quantise(size)).1
}

/// Distance from the top of a line to the next line's top, at `size`, in scene units.
pub fn line_height(size: f32) -> f32 {
    let px = quantise(size);
    face()
        .horizontal_line_metrics(px)
        .map(|m| m.new_line_size)
        .unwrap_or(px * 1.2)
}

/// Word-wraps `text` to `max_w`. `\n` starts a new line. A word wider than `max_w` is cut between
/// characters. Empty lines are kept.
pub fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let joined = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
            if width(&joined, size) <= max_w {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let mut piece = String::new();
            for c in word.chars() {
                piece.push(c);
                if piece.chars().count() > 1 && width(&piece, size) > max_w {
                    piece.pop();
                    lines.push(std::mem::take(&mut piece));
                    piece.push(c);
                }
            }
            line = piece;
        }
        lines.push(line);
    }
    lines
}

/// Shortens `text` with "..." until it fits `max_w`. If even "..." does not fit, that is returned
/// and the caller clips it.
pub fn ellipsize(text: &str, size: f32, max_w: f32) -> String {
    if width(text, size) <= max_w {
        return text.to_string();
    }
    let mut keep: Vec<char> = text.chars().collect();
    while !keep.is_empty() {
        keep.pop();
        let candidate: String = keep.iter().collect::<String>() + "...";
        if width(&candidate, size) <= max_w {
            return candidate;
        }
    }
    "...".to_string()
}

/// One rasterised glyph. `top` is the height of the bitmap's top row above the baseline.
pub struct Glyph {
    pub width: usize,
    pub height: usize,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

#[derive(Default)]
pub struct GlyphCache {
    glyphs: HashMap<(char, u32), Glyph>,
}

impl GlyphCache {
    pub fn get(&mut self, c: char, px: f32) -> &Glyph {
        let px = quantise(px);
        self.glyphs.entry((c, (px * 16.0).round() as u32)).or_insert_with(|| {
            let (m, coverage) = face().rasterize(c, px);
            Glyph { width: m.width, height: m.height, left: m.xmin, top: m.ymin + m.height as i32, coverage }
        })
    }
}

/// Pen positions for drawing `text` with `Cmd::Text`'s rules. Used by the rasteriser.
pub(crate) fn pen_positions(text: &str, px: f32) -> (Vec<(char, f32)>, f32) {
    layout(text, quantise(px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_grows_with_text_and_size() {
        assert!(width("", 12.0) == 0.0);
        assert!(width("WWW", 12.0) > width("W", 12.0));
        assert!(width("WWW", 24.0) > width("WWW", 12.0) * 1.9);
    }

    #[test]
    fn wrap_respects_width_and_keeps_words() {
        let lines = wrap("the quick brown fox jumps over the lazy dog", 12.0, 80.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(width(l, 12.0) <= 80.0 || !l.contains(' '), "line too wide: {l:?}");
        }
        assert_eq!(lines.join(" "), "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn wrap_cuts_long_words_and_keeps_blank_lines() {
        let lines = wrap("supercalifragilisticexpialidocious\n\nend", 12.0, 60.0);
        assert!(lines.len() >= 4);
        assert_eq!(lines[lines.len() - 1], "end");
        assert_eq!(lines[lines.len() - 2], "");
        assert_eq!(lines[..lines.len() - 2].concat(), "supercalifragilisticexpialidocious");
        assert!(lines[0].len() < "supercalifragilisticexpialidocious".len());
    }

    #[test]
    fn ellipsize_fits_or_returns_dots() {
        assert_eq!(ellipsize("short", 12.0, 500.0), "short");
        let s = ellipsize("a much longer label that will not fit", 12.0, 70.0);
        assert!(s.ends_with("..."));
        assert!(width(&s, 12.0) <= 70.0);
        assert_eq!(ellipsize("anything", 12.0, 1.0), "...");
    }

    #[test]
    fn glyph_cache_returns_ink_for_letters_and_none_for_space() {
        let mut cache = GlyphCache::default();
        let g = cache.get('H', 16.0);
        assert!(g.width > 0 && g.height > 0);
        assert!(g.coverage.iter().any(|c| *c > 200));
        let s = cache.get(' ', 16.0);
        assert!(s.coverage.iter().all(|c| *c == 0));
    }
}
