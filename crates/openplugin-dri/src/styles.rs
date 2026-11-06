//! GoodLookin Procedural Styles (Knobs, Faceplates, Meters).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnobStyle {
    /// Classic British console fluted knob with colored pointer cap
    Brit,
    /// Vintage discrete channel knob with knurled grip
    N,
    /// Precision studio pointer knob
    A,
    /// Authentic fluted Bakelite with white engraved indicator
    Bakelite,
    /// Machined metallic aluminum with radial specular reflection
    Metal,
    /// Chrome polished dome knob
    Chrome,
    /// Hardwood mahogany knob with brass indicator
    Wood,
    /// Neon glow cyberpunk dial
    Neon,
    /// Segmented digital encoder style
    Digital,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceplateStyle {
    /// Classic brushed aluminum with directional grain
    BrushedAluminum,
    /// Matte dark anodized rack face
    DarkAnodized,
    /// British vintage console olive/navy finish
    BritishConsole,
    /// American console slate gray
    AmericanConsole,
    /// Neon wireframe synth aesthetic
    Cyberpunk,
    /// Walnut studio equipment cheeks and chassis
    WoodChassis,
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const VINTAGE_CREAM: Self = Self::rgb(0.96, 0.94, 0.86);
    pub const BRIT_BLUE: Self = Self::rgb(0.12, 0.35, 0.58);
    pub const BRIT_RED: Self = Self::rgb(0.72, 0.16, 0.14);
    pub const BRIT_GREEN: Self = Self::rgb(0.18, 0.48, 0.32);
    pub const BAKELITE_BROWN: Self = Self::rgb(0.22, 0.14, 0.10);
    pub const VU_YELLOW: Self = Self::rgb(0.98, 0.92, 0.72);
}
