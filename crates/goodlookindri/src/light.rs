//! One light for the whole panel, so every control's highlight, shading and shadow agree.
//!
//! Panel space: x right, y down, z toward the viewer. `key_dir` points from the surface toward
//! the light. Colours are linear-ish 0..1 multipliers, not sRGB bytes.

use crate::scene::Point;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightRig {
    /// Unit vector from the surface toward the key light.
    pub key_dir: [f32; 3],
    pub key_color: [f32; 3],
    /// Penumbra growth: shadow sigma is `height * softness` (see [`LightRig::shadow_sigma`]).
    pub softness: f32,
    /// Share of light that reaches every surface regardless of facing.
    pub ambient: f32,
    pub fill_color: [f32; 3],
}

impl Default for LightRig { fn default() -> Self { Self::studio() } }

impl LightRig {
    /// Key 20° left of straight up on screen, 65° above the panel; warm key, cool fill.
    pub fn studio() -> Self {
        Self { key_dir: Self::key_from(20.0, 65.0), key_color: [1.0, 0.97, 0.92], softness: 0.27, ambient: 0.32, fill_color: [0.78, 0.84, 0.95] }
    }

    /// Unit direction for a light `left_of_up_deg` degrees anticlockwise from screen-up
    /// (negative is to the right) and `elevation_deg` above the panel plane.
    pub fn key_from(left_of_up_deg: f32, elevation_deg: f32) -> [f32; 3] {
        let (az, el) = (left_of_up_deg.to_radians(), elevation_deg.to_radians());
        [-az.sin() * el.cos(), -az.cos() * el.cos(), el.sin()]
    }

    /// Elevation above the panel, radians.
    pub fn elevation(&self) -> f32 { self.key_dir[2].clamp(-1.0, 1.0).asin() }

    /// Unit on-screen direction toward the light; (0, 0) when it is straight in front.
    pub fn screen_dir(&self) -> Point {
        let [x, y, _] = self.key_dir;
        let l = (x * x + y * y).sqrt();
        if l < 1e-6 { Point::new(0.0, 0.0) } else { Point::new(x / l, y / l) }
    }

    /// Where the shadow of something `height` above the panel lands, relative to it: away from
    /// the light, `height * tan(90° − elevation)` long.
    pub fn shadow_offset(&self, height: f32) -> Point {
        let [x, y, z] = self.key_dir;
        let k = height / z.max(0.05);
        Point::new(-x * k, -y * k)
    }

    /// Blur for that shadow, for [`crate::scene::Cmd::Shadow`].
    pub fn shadow_sigma(&self, height: f32) -> f32 { (height * self.softness).max(0.5) }

    /// Lambert term for a unit normal, 0..1.
    pub fn diffuse(&self, n: [f32; 3]) -> f32 { dot(n, self.key_dir).max(0.0) }

    /// Blinn-Phong highlight toward a viewer on +z.
    pub fn specular(&self, n: [f32; 3], shininess: f32) -> f32 {
        let [x, y, z] = self.key_dir;
        let h = [x, y, z + 1.0];
        let l = dot(h, h).sqrt().max(1e-6);
        (dot(n, h) / l).max(0.0).powf(shininess)
    }

    /// Ambient fill plus diffuse key, per channel, for a unit normal.
    pub fn shade(&self, n: [f32; 3]) -> [f32; 3] {
        let d = self.diffuse(n) * (1.0 - self.ambient);
        let ch = |i: usize| self.fill_color[i] * self.ambient + self.key_color[i] * d;
        [ch(0), ch(1), ch(2)]
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_geometry() {
        let l = LightRig::studio();
        let [x, y, z] = l.key_dir;
        assert!(((x * x + y * y + z * z) - 1.0).abs() < 1e-5);
        assert!(x < 0.0 && y < 0.0 && z > 0.0);
        assert!((l.elevation().to_degrees() - 65.0).abs() < 1e-3);
        let d = l.screen_dir();
        assert!((d.y.atan2(d.x).to_degrees() - (-110.0)).abs() < 1e-3);
        let o = l.shadow_offset(10.0);
        assert!(o.x > 0.0 && o.y > 0.0);
        let len = (o.x * o.x + o.y * o.y).sqrt();
        assert!((len - 10.0 * 25f32.to_radians().tan()).abs() < 1e-3);
        assert!(l.diffuse([0.0, 0.0, 1.0]) > l.diffuse([0.6, 0.6, 0.53]));
    }
}
