//! Small vector, colour and noise helpers shared by the 3D knob renderer.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
pub(crate) const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}
impl std::ops::Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl std::ops::Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl std::ops::Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}
impl std::ops::Mul<V3> for V3 {
    type Output = V3;
    fn mul(self, o: V3) -> V3 {
        v3(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}
impl std::ops::AddAssign for V3 {
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    v3(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x)
}
pub(crate) fn norm(a: V3) -> V3 {
    a * (1.0 / dot(a, a).sqrt().max(1e-12))
}
pub(crate) fn grey(g: f32) -> V3 {
    v3(g, g, g)
}
pub(crate) fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    a + (b - a) * t
}
pub(crate) fn reflect(i: V3, n: V3) -> V3 {
    i - n * (2.0 * dot(i, n))
}
pub(crate) fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
/// Smoothstep that also accepts `e0 > e1` (falling edge).
pub(crate) fn ss(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
pub(crate) fn mixf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
pub(crate) fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
/// sRGB triple picked by eye, converted to linear.
pub(crate) fn srgb(r: f32, g: f32, b: f32) -> V3 {
    v3(to_linear(r), to_linear(g), to_linear(b))
}

pub(crate) fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xff_ffff) as f32 / 16_777_216.0
}
pub(crate) fn vnoise(x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let fx = x - xi;
    let fy = y - yi;
    let (xi, yi) = (xi as i32, yi as i32);
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash2(xi, yi);
    let b = hash2(xi + 1, yi);
    let c = hash2(xi, yi + 1);
    let d = hash2(xi + 1, yi + 1);
    mixf(mixf(a, b, sx), mixf(c, d, sx), sy)
}
pub(crate) struct Rng(pub u32);
impl Rng {
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 16_777_216.0
    }
}

/// Linear up to the knee, then a soft shoulder towards 1.
pub(crate) fn tonemap(c: f32) -> f32 {
    let knee = 0.7;
    if c <= knee {
        c
    } else {
        knee + (1.0 - knee) * (1.0 - (-(c - knee) / (1.0 - knee)).exp())
    }
}
/// Linear scene value to sRGB in 0..1 after tone mapping.
pub(crate) fn to_srgb(c: f32) -> f32 {
    let c = tonemap(c.max(0.0));
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
