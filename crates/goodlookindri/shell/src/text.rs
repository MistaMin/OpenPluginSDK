//! Outline text for [`goodlookindri::scene::Cmd::Text`], plus measuring, wrapping and ellipsis
//! for widgets.
//!
//! The default face is Source Sans Pro Semibold (holds up at small sizes on 1x screens), unmodified, under the SIL Open Font License 1.1
//! (`fonts/OFL.txt`). `fontdue` rasterises it. Sizes are in scene units, the same as
//! `Cmd::Text::size`; the rasteriser passes `size * scale` as pixels.
//!
//! More faces: [`register`] one of the [`bundled`] fonts (or any TrueType/OpenType bytes), or
//! look one up among the fonts installed on this computer with [`system`]. Either gives a
//! [`FontId`] for [`goodlookindri::scene::Scene::font`] and the `*_in` measuring functions.
//!
//! Nothing here uses thread-local storage, so the code still loads on macOS 10.6.

use fontdue::{Font, FontSettings};
use goodlookindri::scene::FontId;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Mutex, MutexGuard, OnceLock};

pub use crate::sysfont::system;

const FACE: &[u8] = include_bytes!("../fonts/SourceSansPro-Semibold.otf");

/// Where the anchor of `Cmd::Text` sits, as a share of the pixel size below the baseline. This is
/// about half the cap height, so a line centred on `at.y` looks centred.
pub const MIDDLE: f32 = 0.35;

pub fn face() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(|| Font::from_bytes(FACE, FontSettings::default()).expect("bundled Source Sans Pro"))
}

/// Registered faces, `FontId` order, and the bytes of the `&'static` ones (to hand the same id back).
struct Faces { faces: Vec<&'static Font>, statics: Vec<(usize, usize, FontId)> }

fn faces() -> MutexGuard<'static, Faces> {
    static FACES: OnceLock<Mutex<Faces>> = OnceLock::new();
    let m = FACES.get_or_init(|| Mutex::new(Faces { faces: vec![face()], statics: Vec::new() }));
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The face for `font`; the default face for [`FontId::DEFAULT`] and for an id never handed out.
pub fn face_of(font: FontId) -> &'static Font {
    faces().faces.get(font.0 as usize).copied().unwrap_or_else(face)
}

/// Registers a TrueType or OpenType face (the first in a collection) and returns its id. The same
/// bytes give the same id again. `None` if the bytes are not a font.
pub fn register(bytes: &'static [u8]) -> Option<FontId> {
    let key = (bytes.as_ptr() as usize, bytes.len());
    if let Some(&(_, _, id)) = faces().statics.iter().find(|(p, l, _)| (*p, *l) == key) {
        return Some(id);
    }
    let id = register_at(bytes, 0)?;
    faces().statics.push((key.0, key.1, id));
    Some(id)
}

/// Parses face `index` of `bytes` and adds it. Faces live as long as the process (there are few).
pub(crate) fn register_at<D: Deref<Target = [u8]>>(bytes: D, index: u32) -> Option<FontId> {
    let font = Font::from_bytes(bytes, FontSettings { collection_index: index, ..FontSettings::default() }).ok()?;
    let mut f = faces();
    let id = FontId(u16::try_from(f.faces.len()).ok()?);
    f.faces.push(Box::leak(Box::new(font)));
    Some(id)
}

/// Open-licence faces that come with the toolkit, unmodified. Pass one to [`register`]. A plug-in
/// carries only the ones it names (the linker drops the rest). Each folder in `shell/fonts` holds
/// the font's licence (SIL Open Font License 1.1), which ships with a plug-in that uses it.
pub mod bundled {
    /// Inter 4.1, a screen UI face.
    pub static INTER_REGULAR: &[u8] = include_bytes!("../fonts/inter/Inter-Regular.ttf");
    pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../fonts/inter/Inter-SemiBold.ttf");
    pub static INTER_BOLD: &[u8] = include_bytes!("../fonts/inter/Inter-Bold.ttf");
    /// Source Sans 3 (3.052), the newer cut of the default face.
    pub static SOURCE_SANS_3_REGULAR: &[u8] = include_bytes!("../fonts/source-sans-3/SourceSans3-Regular.otf");
    pub static SOURCE_SANS_3_SEMIBOLD: &[u8] = include_bytes!("../fonts/source-sans-3/SourceSans3-Semibold.otf");
    pub static SOURCE_SANS_3_BOLD: &[u8] = include_bytes!("../fonts/source-sans-3/SourceSans3-Bold.otf");
    /// JetBrains Mono 2.304, monospaced: readouts whose digits do not move.
    pub static JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Regular.ttf");
    pub static JETBRAINS_MONO_BOLD: &[u8] = include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Bold.ttf");
    /// Barlow Condensed, narrow panel lettering.
    pub static BARLOW_CONDENSED_REGULAR: &[u8] = include_bytes!("../fonts/barlow-condensed/BarlowCondensed-Regular.ttf");
    pub static BARLOW_CONDENSED_SEMIBOLD: &[u8] = include_bytes!("../fonts/barlow-condensed/BarlowCondensed-SemiBold.ttf");
    /// DSEG 0.46: 7-segment (digits and a few letters) and 14-segment LCD/LED displays.
    pub static DSEG7_CLASSIC_REGULAR: &[u8] = include_bytes!("../fonts/dseg/DSEG7Classic-Regular.ttf");
    pub static DSEG7_CLASSIC_BOLD: &[u8] = include_bytes!("../fonts/dseg/DSEG7Classic-Bold.ttf");
    pub static DSEG14_CLASSIC_REGULAR: &[u8] = include_bytes!("../fonts/dseg/DSEG14Classic-Regular.ttf");
}

/// Glyph sizes are cached per 1/16 pixel, so a size that differs only by float noise reuses one bitmap.
pub fn quantise(px: f32) -> f32 {
    (px.max(0.0) * 16.0).round() / 16.0
}

/// Pen positions for each character, with kerning, and the total advance, all in pixels.
fn layout(font: &Font, text: &str, px: f32) -> (Vec<(char, f32)>, f32) {
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
    width_in(FontId::DEFAULT, text, size)
}

/// [`width`] in `font`.
pub fn width_in(font: FontId, text: &str, size: f32) -> f32 {
    layout(face_of(font), text, quantise(size)).1
}

/// Distance from the top of a line to the next line's top, at `size`, in scene units.
pub fn line_height(size: f32) -> f32 {
    line_height_in(FontId::DEFAULT, size)
}

/// [`line_height`] in `font`.
pub fn line_height_in(font: FontId, size: f32) -> f32 {
    let px = quantise(size);
    face_of(font)
        .horizontal_line_metrics(px)
        .map(|m| m.new_line_size)
        .unwrap_or(px * 1.2)
}

/// Word-wraps `text` to `max_w`. `\n` starts a new line. A word wider than `max_w` is cut between
/// characters. Empty lines are kept.
pub fn wrap(text: &str, size: f32, max_w: f32) -> Vec<String> {
    wrap_in(FontId::DEFAULT, text, size, max_w)
}

/// [`wrap`] in `font`.
pub fn wrap_in(font: FontId, text: &str, size: f32, max_w: f32) -> Vec<String> {
    let width = |t: &str, size: f32| width_in(font, t, size);
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
    ellipsize_in(FontId::DEFAULT, text, size, max_w)
}

/// [`ellipsize`] in `font`.
pub fn ellipsize_in(font: FontId, text: &str, size: f32, max_w: f32) -> String {
    let width = |t: &str, size: f32| width_in(font, t, size);
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
    glyphs: HashMap<(u16, char, u32), Glyph>,
}

impl GlyphCache {
    pub fn get(&mut self, c: char, px: f32) -> &Glyph {
        self.get_in(FontId::DEFAULT, c, px)
    }

    /// [`Self::get`] in `font`.
    pub fn get_in(&mut self, font: FontId, c: char, px: f32) -> &Glyph {
        let px = quantise(px);
        self.glyphs.entry((font.0, c, (px * 16.0).round() as u32)).or_insert_with(|| {
            let (m, coverage) = face_of(font).rasterize(c, px);
            Glyph { width: m.width, height: m.height, left: m.xmin, top: m.ymin + m.height as i32, coverage }
        })
    }
}

/// Pen positions for drawing `text` with `Cmd::Text`'s rules. Used by the rasteriser.
pub(crate) fn pen_positions(font: FontId, text: &str, px: f32) -> (Vec<(char, f32)>, f32) {
    layout(face_of(font), text, quantise(px))
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
