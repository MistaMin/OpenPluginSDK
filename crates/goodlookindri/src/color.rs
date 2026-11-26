//! Straight-alpha sRGB `0xAARRGGBB`, matching GoodLookinUI.
//! `brighter` / `darker` follow JUCE `Colour`: move toward white, or scale toward black.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub u32);

impl Color {
    pub const fn rgba(r: u32, g: u32, b: u32, a: u32) -> Self {
        Self((a << 24) | (r << 16) | (g << 8) | b)
    }
    pub const fn a(self) -> u8 { (self.0 >> 24) as u8 }
    pub const fn r(self) -> u8 { (self.0 >> 16) as u8 }
    pub const fn g(self) -> u8 { (self.0 >> 8) as u8 }
    pub const fn b(self) -> u8 { self.0 as u8 }

    pub fn with_alpha(self, alpha: f32) -> Self {
        let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        Color((self.0 & 0x00ff_ffff) | (a << 24))
    }
    pub fn alpha_f(self) -> f32 { self.a() as f32 / 255.0 }

    /// JUCE `Colour::brighter(amount)`: each channel moves toward 255 by `amount`.
    pub fn brighter(self, amount: f32) -> Self {
        let amt = amount.max(0.0);
        let ch = |c: u8| {
            let c = c as f32;
            (c + amt * (255.0 - c)).clamp(0.0, 255.0).round() as u32
        };
        Color::rgba(ch(self.r()), ch(self.g()), ch(self.b()), self.a() as u32)
    }
    /// JUCE `Colour::darker(amount)`: each channel is multiplied by `1 - amount`.
    pub fn darker(self, amount: f32) -> Self {
        let amt = amount.clamp(0.0, 1.0);
        let ch = |c: u8| (c as f32 * (1.0 - amt)).clamp(0.0, 255.0).round() as u32;
        Color::rgba(ch(self.r()), ch(self.g()), ch(self.b()), self.a() as u32)
    }
    pub fn mix(self, other: Color, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let ch = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t).round() as u32;
        Color::rgba(ch(self.r(), other.r()), ch(self.g(), other.g()), ch(self.b(), other.b()), ch(self.a(), other.a()))
    }
    /// JUCE perceived brightness in 0..1.
    pub fn perceived_brightness(self) -> f32 {
        let r = self.r() as f32 / 255.0;
        let g = self.g() as f32 / 255.0;
        let b = self.b() as f32 / 255.0;
        (r * r * 0.241 + g * g * 0.691 + b * b * 0.068).sqrt()
    }
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(1.0);
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let i = (h * 6.0).floor();
        let f = h * 6.0 - i;
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i as i32 % 6 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color::rgba((r * 255.0).round() as u32, (g * 255.0).round() as u32, (b * 255.0).round() as u32, 255)
    }
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let r = self.r() as f32 / 255.0;
        let g = self.g() as f32 / 255.0;
        let b = self.b() as f32 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let s = if max == 0.0 { 0.0 } else { d / max };
        (h, s, max)
    }
    pub fn from_hex(hex: &str) -> Option<Self> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
        Some(Color(0xff00_0000 | u32::from_str_radix(h, 16).ok()?))
    }
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r(), self.g(), self.b())
    }
}

pub fn parse_hex(hex: &str) -> Option<Color> { Color::from_hex(hex) }
