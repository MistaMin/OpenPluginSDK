//! Meter styles and colours. Knob and faceplate styles are GoodLookin DRI codes
//! ([`crate::draw_knob`], [`crate::draw_faceplate`]).

/// How [`crate::draw_vu_meter`] draws a level.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MeterStyle {
    /// Traditional analog VU meter with illuminated incandescent warm yellow dial
    AnalogVu,
    /// Fast DIN peak programme meter (PPM)
    Ppm,
    /// Multi-segment LED ladder with Green/Yellow/Red tiers
    LedLadder,
    /// 80s arcade neon vector bar
    RetroBar,
}

/// A straight (not premultiplied) RGBA colour, each part 0 to 1. Drawing clamps parts
/// outside that range.
///
/// ```
/// use openplugin_dri::Color;
///
/// let amber = Color::rgb(1.0, 0.6, 0.1);
/// assert_eq!(amber.a, 1.0);
/// let glass = Color::rgba(1.0, 1.0, 1.0, 0.25);
/// assert_eq!(Color::WHITE, Color::rgba(1.0, 1.0, 1.0, 1.0));
/// # let _ = glass;
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Red, 0 to 1.
    pub r: f32,
    /// Green, 0 to 1.
    pub g: f32,
    /// Blue, 0 to 1.
    pub b: f32,
    /// Opacity, 0 (transparent) to 1 (opaque).
    pub a: f32,
}

impl Color {
    /// A colour from its four parts, 0 to 1. Not checked here.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// An opaque colour from its three parts, 0 to 1. Not checked here.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Opaque white.
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    /// The cream of a vintage dial face.
    pub const VINTAGE_CREAM: Self = Self::rgb(0.96, 0.94, 0.86);
    /// A muted console blue.
    pub const BLUE: Self = Self::rgb(0.12, 0.35, 0.58);
    /// A muted console red.
    pub const RED: Self = Self::rgb(0.72, 0.16, 0.14);
    /// A muted console green.
    pub const GREEN: Self = Self::rgb(0.18, 0.48, 0.32);
    /// The dark brown of Bakelite.
    pub const BAKELITE_BROWN: Self = Self::rgb(0.22, 0.14, 0.10);
    /// The warm yellow of a lit VU face.
    pub const VU_YELLOW: Self = Self::rgb(0.98, 0.92, 0.72);
}
