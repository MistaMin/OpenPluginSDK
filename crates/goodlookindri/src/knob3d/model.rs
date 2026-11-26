//! Public description of a knob: radial profile, angular detail, materials, light rig.

use crate::color::Color;
use super::vec::to_linear;

/// Surface description. `base` is linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub base: [f32; 3],
    /// 0 = dielectric (paint, plastic), 1 = bare metal.
    pub metallic: f32,
    pub roughness: f32,
    /// Reflectance at normal incidence for the dielectric part.
    pub f0: f32,
    /// Spread of the reflection across lathe grooves (spun metal), 0 = isotropic.
    pub anisotropy: f32,
    /// Glossy lacquer layer on top, 0..1.
    pub clear_coat: f32,
    /// Strength of the fine surface texture (lathe rings or blasting speckle).
    pub grain: f32,
    /// How much of the caller's accent colour replaces `base`, 0..1.
    pub accent: f32,
    /// Brightness of the accent colour on this material (1 = as given). Below 1 gives the
    /// darker side wall of a coloured cap in the same hue.
    pub accent_shade: f32,
    /// Concentric lathe rings cut into a turned face, 0..1.
    pub rings: f32,
    /// Straight wood figure (grain lines, pores, slow colour bands) along the pointer axis.
    pub wood: f32,
    /// Soft-touch sheen: a velvety lift of the colour at grazing angles.
    pub sheen: f32,
    /// Light that enters and comes back out tinted: makes glossy plastic look see-through.
    pub translucency: f32,
}

impl Material {
    /// Bare metal with an sRGB tint.
    pub fn metal(r: f32, g: f32, b: f32, roughness: f32) -> Self {
        Self { base: [to_linear(r), to_linear(g), to_linear(b)], metallic: 1.0, roughness, f0: 0.04, anisotropy: 0.0, clear_coat: 0.0, grain: 0.0, accent: 0.0, accent_shade: 1.0, rings: 0.0, wood: 0.0, sheen: 0.0, translucency: 0.0 }
    }
    /// Paint or plastic with an sRGB colour.
    pub fn plastic(r: f32, g: f32, b: f32, roughness: f32) -> Self {
        Self { metallic: 0.0, ..Self::metal(r, g, b, roughness) }
    }
    pub fn with_aniso(mut self, a: f32) -> Self {
        self.anisotropy = a;
        self
    }
    pub fn with_coat(mut self, c: f32) -> Self {
        self.clear_coat = c;
        self
    }
    pub fn with_grain(mut self, g: f32) -> Self {
        self.grain = g;
        self
    }
    pub fn with_accent(mut self, a: f32) -> Self {
        self.accent = a;
        self
    }
    /// Accent share and the brightness of the accent on this material.
    pub fn with_accent_shade(mut self, a: f32, shade: f32) -> Self {
        self.accent = a;
        self.accent_shade = shade;
        self
    }
    pub fn with_rings(mut self, r: f32) -> Self {
        self.rings = r;
        self
    }
    pub fn with_wood(mut self, w: f32) -> Self {
        self.wood = w;
        self
    }
    pub fn with_sheen(mut self, s: f32) -> Self {
        self.sheen = s;
        self
    }
    pub fn with_translucency(mut self, t: f32) -> Self {
        self.translucency = t;
        self
    }
    pub fn with_roughness(mut self, r: f32) -> Self {
        self.roughness = r;
        self
    }

    /// Lacquered wood in any sRGB colour: straight figure under a clear coat.
    pub fn wood(r: f32, g: f32, b: f32) -> Self {
        Self::plastic(r, g, b, 0.55).with_wood(1.0).with_coat(0.85)
    }
    /// Dark brown walnut under lacquer.
    pub fn walnut() -> Self {
        Self::wood(0.36, 0.20, 0.10)
    }
    /// Pale maple under lacquer.
    pub fn maple() -> Self {
        Self::wood(0.80, 0.62, 0.38).with_wood(0.6)
    }
    /// Polished yellow brass.
    pub fn brass() -> Self {
        Self::metal(0.93, 0.74, 0.36, 0.16)
    }
    /// Darker, redder bronze with a satin finish.
    pub fn bronze() -> Self {
        Self::metal(0.70, 0.46, 0.26, 0.30).with_grain(0.03)
    }
    /// Mirror chrome.
    pub fn chrome() -> Self {
        Self::metal(0.95, 0.95, 0.96, 0.04)
    }
    /// Satin (bead-blasted or fine-brushed) aluminium.
    pub fn satin_aluminium() -> Self {
        Self::metal(0.93, 0.93, 0.925, 0.42).with_grain(0.02)
    }
    /// Turned aluminium face: lathe rings that spread the highlight into a fan.
    pub fn spun_aluminium() -> Self {
        Self::metal(0.90, 0.905, 0.91, 0.09).with_aniso(0.55).with_rings(0.5)
    }
    /// Die-cast zinc or aluminium: so rough that it is lit almost like paint.
    pub fn die_cast() -> Self {
        Self { metallic: 0.5, ..Self::plastic(0.74, 0.75, 0.76, 0.5) }.with_grain(0.05)
    }
    /// Dyed anodised aluminium: a coloured metal with a fine satin finish.
    pub fn anodised(r: f32, g: f32, b: f32) -> Self {
        Self::metal(r, g, b, 0.34).with_grain(0.02)
    }
    /// Matte rubber or soft-touch paint.
    pub fn rubber(r: f32, g: f32, b: f32) -> Self {
        Self { f0: 0.03, ..Self::plastic(r, g, b, 0.85) }.with_sheen(0.6)
    }
    /// Cream plastic that has yellowed a little with age.
    pub fn cream() -> Self {
        Self::plastic(0.86, 0.80, 0.64, 0.42).with_coat(0.35)
    }
    /// Deep glossy black moulding compound.
    pub fn black_gloss() -> Self {
        Self::plastic(0.055, 0.054, 0.053, 0.5).with_coat(0.9)
    }
    /// Polished black moulding compound for skirted and pointer knobs: nearly no haze
    /// under the lacquer, so the black stays deep between crisp reflections.
    pub fn black_polished() -> Self {
        Self { f0: 0.02, ..Self::plastic(0.07, 0.07, 0.072, 0.34) }.with_coat(0.85).with_grain(0.02)
    }
    /// Matte to eggshell black plastic.
    pub fn black_matte() -> Self {
        Self::plastic(0.13, 0.13, 0.135, 0.5)
    }
    /// Glossy coloured plastic that light travels through.
    pub fn translucent(r: f32, g: f32, b: f32) -> Self {
        Self::plastic(r, g, b, 0.5).with_coat(1.0).with_translucency(0.8)
    }
    /// Glossy opaque plastic.
    pub fn gloss(r: f32, g: f32, b: f32) -> Self {
        Self::plastic(r, g, b, 0.5).with_coat(0.9)
    }
    /// Off-white paint for pointer lines.
    pub fn white_paint() -> Self {
        Self::plastic(0.94, 0.93, 0.89, 0.6)
    }
    /// Black paint for pointer lines.
    pub fn ink() -> Self {
        Self::plastic(0.03, 0.03, 0.033, 0.9)
    }

    pub(crate) fn tinted(mut self, accent: Color) -> Self {
        if self.accent > 0.0 {
            let c = [accent.r(), accent.g(), accent.b()];
            for i in 0..3 {
                let l = to_linear(c[i] as f32 / 255.0) * self.accent_shade;
                self.base[i] += (l - self.base[i]) * self.accent.min(1.0);
            }
        }
        self
    }
}

/// How the height moves from `h0` (at `r0`) to `h1` (at `r1`) inside a segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Constant `h0`.
    Flat,
    /// Straight: cone, chamfer or wall.
    Line,
    /// Convex quarter ellipse, level at `r0`, steep at `r1`.
    Dome,
    /// Concave quarter ellipse, steep at `r0`, level at `r1`.
    Fillet,
}

/// One ring of the radial profile. Radii and heights are in units of the knob radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub r0: f32,
    pub r1: f32,
    pub h0: f32,
    pub h1: f32,
    pub shape: Shape,
    pub material: Material,
}

impl Segment {
    pub fn new(shape: Shape, r0: f32, r1: f32, h0: f32, h1: f32, material: Material) -> Self {
        Self { r0, r1, h0, h1, shape, material }
    }
    pub(crate) fn height(&self, u: f32) -> f32 {
        let t = ((u - self.r0) / (self.r1 - self.r0).max(1e-6)).clamp(0.0, 1.0);
        match self.shape {
            Shape::Flat => self.h0,
            Shape::Line => self.h0 + (self.h1 - self.h0) * t,
            Shape::Dome => self.h1 + (self.h0 - self.h1) * (1.0 - t * t).sqrt(),
            Shape::Fillet => self.h1 + (self.h0 - self.h1) * (1.0 - (1.0 - (1.0 - t) * (1.0 - t)).sqrt()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AngularKind {
    /// Fine straight ridges; fades out when ridges get close to a pixel wide.
    Knurl,
    /// Smooth sinusoidal finger flutes.
    Flutes,
    /// Circular cuts between rounded lobes.
    Scallops,
    /// Flat-topped lugs with flat gaps between them, one lug centred on the pointer.
    /// `duty` is the share of the pitch a lug takes, 0..1.
    Teeth { duty: f32 },
    /// Asymmetric plan shape: the profile is squeezed into a tapered bar that points along
    /// the knob's zero direction. The tip sits at the full radius, the tail at `tail`
    /// behind the axis; widths are half-widths. All in fractions of the radius.
    /// `count` and `depth` are ignored.
    Bar { tip_width: f32, tail_width: f32, tail: f32 },
}

/// Angular detail that pulls the profile inwards, so it also shapes the silhouette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angular {
    pub kind: AngularKind,
    pub count: u32,
    /// Deepest inward cut as a fraction of the radius.
    pub depth: f32,
    /// Radial band (fractions of the radius) in which the detail acts.
    pub r_in: f32,
    pub r_out: f32,
}

impl Angular {
    /// Fine straight knurling from `r_in` outwards.
    pub fn knurl(count: u32, depth: f32, r_in: f32) -> Self {
        Self { kind: AngularKind::Knurl, count, depth, r_in, r_out: 1.0 }
    }
    /// Smooth finger flutes from `r_in` outwards.
    pub fn flutes(count: u32, depth: f32, r_in: f32) -> Self {
        Self { kind: AngularKind::Flutes, count, depth, r_in, r_out: 1.0 }
    }
    /// Scalloped lobes from `r_in` outwards; a lobe is centred on the pointer.
    pub fn scallops(count: u32, depth: f32, r_in: f32) -> Self {
        Self { kind: AngularKind::Scallops, count, depth, r_in, r_out: 1.0 }
    }
    /// `count` square lugs that stand `depth` proud of the gaps; `duty` is their share of
    /// the pitch.
    pub fn teeth(count: u32, depth: f32, duty: f32, r_in: f32) -> Self {
        Self { kind: AngularKind::Teeth { duty }, count, depth, r_in, r_out: 1.0 }
    }
}

/// Plan shape of a body: what a horizontal slice through it looks like.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Plan {
    /// Circle: the profile is revolved around the axis.
    #[default]
    Round,
    /// Regular polygon with corners at the profile radius and a flat facing the pointer.
    /// `rounding` 0..1 rounds the corners (1 is nearly a circle). 4 sides with rounding
    /// gives a rounded square, 6 a hexagon, 8 an octagon.
    Polygon { sides: u32, rounding: f32 },
    /// Tapered bar along the pointer axis from `-tail` behind the axis to `tip` ahead of
    /// it, with half-widths `tail_width` and `tip_width`; `rounding` is the corner radius.
    /// All in fractions of the knob radius. The profile is read by distance from the
    /// bar's outline: profile radius 1 is the outline and `1 - d` lies `d` inside it, so
    /// a chamfer drawn from 0.9 to 1 is a bevel of constant width all the way round.
    Bar { tip: f32, tail: f32, tip_width: f32, tail_width: f32, rounding: f32 },
}

/// An extra solid on the knob (fin, beak, hub, second tier). The surface is the highest
/// of the body, the under-body and every part.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub profile: Vec<Segment>,
    pub plan: Plan,
    pub angular: Vec<Angular>,
}

impl Part {
    pub fn new(profile: Vec<Segment>, plan: Plan) -> Self {
        Self { profile, plan, angular: Vec::new() }
    }
    pub fn with_angular(mut self, a: Angular) -> Self {
        self.angular.push(a);
        self
    }
}

/// Disc let into the top face: sunk by `depth` and given its own material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Insert {
    pub r: f32,
    pub depth: f32,
    pub material: Material,
}

/// Grub-screw hole in the side wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SetScrew {
    /// Clockwise from the pointer, radians.
    pub angle: f32,
    /// Height of the hole centre above the panel, fraction of the radius.
    pub height: f32,
    /// Hole radius, fraction of the radius.
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerKind {
    /// Painted line, flush with the surface.
    Line,
    /// Milled groove filled with the pointer material.
    Groove,
    /// Round dot centred at `r1` with diameter `width`; `depth` > 0 sinks it as a dimple.
    Dot,
    /// Raised bar of height `depth` with rounded flanks.
    Ridge,
}

/// Radial indicator from `r0` to `r1` along the knob's zero direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub kind: PointerKind,
    pub r0: f32,
    pub r1: f32,
    /// Full width as a fraction of the radius.
    pub width: f32,
    /// Groove depth as a fraction of the radius (ignored for `Line`).
    pub depth: f32,
    pub material: Material,
}

impl Pointer {
    /// Painted line from `r0` to `r1`.
    pub fn line(r0: f32, r1: f32, width: f32, material: Material) -> Self {
        Self { kind: PointerKind::Line, r0, r1, width, depth: 0.0, material }
    }
    /// Milled and filled groove.
    pub fn groove(r0: f32, r1: f32, width: f32, depth: f32, material: Material) -> Self {
        Self { kind: PointerKind::Groove, r0, r1, width, depth, material }
    }
    /// Dot at radius `r`; `depth` > 0 makes it a dimple.
    pub fn dot(r: f32, diameter: f32, depth: f32, material: Material) -> Self {
        Self { kind: PointerKind::Dot, r0: r, r1: r, width: diameter, depth, material }
    }
    /// Raised bar standing `height` proud of the surface.
    pub fn ridge(r0: f32, r1: f32, width: f32, height: f32, material: Material) -> Self {
        Self { kind: PointerKind::Ridge, r0, r1, width, depth: height, material }
    }
}

/// A knob as a height field over its base circle.
#[derive(Clone, Debug, PartialEq)]
pub struct KnobModel {
    /// Rings from the axis outwards. Beyond the last `r1` the height is zero.
    pub profile: Vec<Segment>,
    /// Optional round under-body (skirt, hub, bezel) that the angular detail does not touch.
    /// The surface is the higher of the two.
    pub under: Vec<Segment>,
    pub angular: Vec<Angular>,
    pub pointer: Option<Pointer>,
    /// Scales every profile height (1 = as authored).
    pub height: f32,
    /// Edge rounding radius as a fraction of the radius.
    pub edge: f32,
    /// Plan shape of `profile` (the under-body is always round).
    pub plan: Plan,
    /// Extra solids: fins, beaks, tiers.
    pub parts: Vec<Part>,
    /// More indicators besides `pointer` (a second line on the skirt, a dot, ...).
    pub marks: Vec<Pointer>,
    pub insert: Option<Insert>,
    pub screw: Option<SetScrew>,
}

/// Fixed studio lighting and camera, defined in panel space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    /// Key light azimuth in radians, measured from straight-up towards the left.
    pub key_azimuth: f32,
    /// Key light elevation above the panel, radians.
    pub key_elevation: f32,
    pub key_intensity: f32,
    /// Half-height of the key softbox as seen in reflections (tangent of the half angle).
    pub key_size: f32,
    /// Penumbra growth of the cast shadow per unit distance from the occluder.
    pub shadow_softness: f32,
    pub ambient: f32,
    /// Brightness of the studio environment reflections.
    pub environment: f32,
    /// Colour of the panel under the knob: tints wall reflections and bounce light.
    pub floor: Color,
    /// Camera tilt in radians: the viewer looks from below-front.
    pub tilt: f32,
    /// Darkness of the cast shadow on the panel, 0..1.
    pub shadow: f32,
    /// Darkness of the contact shadow / base occlusion on the panel, 0..1.
    pub occlusion: f32,
    /// Rim light: a low ring of light around the set plus the glow dark panels throw back
    /// at the walls. Keeps black knobs readable on black panels; fades on bright ones.
    pub rim: f32,
}

impl Rig {
    pub fn studio() -> Self {
        Self {
            key_azimuth: 20f32.to_radians(),
            key_elevation: 65f32.to_radians(),
            key_intensity: 1.0,
            key_size: 0.15,
            shadow_softness: 0.7,
            ambient: 1.0,
            environment: 1.0,
            floor: Color(0xffc0_c0c0),
            tilt: 14f32.to_radians(),
            shadow: 0.72,
            occlusion: 0.6,
            rim: 1.0,
        }
    }
}

impl Default for Rig {
    fn default() -> Self {
        Self::studio()
    }
}

/// Premultiplied sRGB sprite, `0xAARRGGBB`, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Sprite {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
    /// Where the knob's base centre sits inside the sprite, device pixels.
    pub origin: (f32, f32),
}

impl KnobModel {
    /// Round body from a radial profile; everything else is added with the `with_*` calls.
    pub fn new(profile: Vec<Segment>) -> Self {
        Self { profile, under: Vec::new(), angular: Vec::new(), pointer: None, height: 1.0, edge: 0.015, plan: Plan::Round, parts: Vec::new(), marks: Vec::new(), insert: None, screw: None }
    }
    /// Stepped two-tier body: a wide lower ring of radius 1 and height `lower_h`, and a
    /// cap of radius `upper_r` that stands `upper_h` tall, each with a small chamfer.
    pub fn two_tier(lower_h: f32, upper_r: f32, upper_h: f32, lower: Material, upper: Material) -> Self {
        let c = 0.035;
        Self::new(vec![
            Segment::new(Shape::Dome, 0.0, upper_r - c, upper_h + 0.012, upper_h, upper),
            Segment::new(Shape::Line, upper_r - c, upper_r, upper_h, upper_h - c, upper),
            Segment::new(Shape::Line, upper_r, upper_r + 0.012, upper_h - c, lower_h, upper),
            Segment::new(Shape::Flat, upper_r + 0.012, 0.95 - c, lower_h, lower_h, lower),
            Segment::new(Shape::Line, 0.95 - c, 0.95, lower_h, lower_h - c, lower),
            Segment::new(Shape::Line, 0.95, 0.975, lower_h - c, 0.03, lower),
        ])
    }
    /// Round under-body (skirt, hub, bezel) that angular detail and `plan` do not touch.
    pub fn with_under(mut self, under: Vec<Segment>) -> Self {
        self.under = under;
        self
    }
    pub fn with_angular(mut self, a: Angular) -> Self {
        self.angular.push(a);
        self
    }
    /// Polygon or bar silhouette for the main profile.
    pub fn with_plan(mut self, plan: Plan) -> Self {
        self.plan = plan;
        self
    }
    pub fn with_part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }
    pub fn with_pointer(mut self, p: Pointer) -> Self {
        self.pointer = Some(p);
        self
    }
    /// A further indicator in addition to the pointer.
    pub fn with_mark(mut self, p: Pointer) -> Self {
        self.marks.push(p);
        self
    }
    /// Recessed insert disc of radius `r`, sunk by `depth`.
    pub fn with_insert(mut self, r: f32, depth: f32, material: Material) -> Self {
        self.insert = Some(Insert { r, depth, material });
        self
    }
    /// Set-screw hole in the wall, `angle` radians clockwise from the pointer.
    pub fn with_set_screw(mut self, angle: f32, height: f32, radius: f32) -> Self {
        self.screw = Some(SetScrew { angle, height, radius });
        self
    }
    pub fn with_edge(mut self, edge: f32) -> Self {
        self.edge = edge;
        self
    }
    pub fn with_height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
    /// Where the knob appears to sit under the rig's tilted camera, in units of the knob
    /// radius: how far the centre of its silhouette lies above the base centre, and the
    /// radius of the circle around that point that encloses the silhouette. A printed
    /// scale drawn around this circle looks centred on tall knobs too.
    pub fn visual_frame(&self, rig: &Rig) -> (f32, f32) {
        let t = rig.tilt.tan() * self.height;
        let (mut up, mut down, mut wide) = (0.0f32, 0.0f32, 0.0f32);
        let mut point = |r: f32, h: f32| {
            up = up.max(r + h * t);
            down = down.max(r - h * t);
            wide = wide.max(r);
        };
        for s in self.profile.iter().chain(self.under.iter()) {
            point(s.r0, s.h0);
            point(s.r1, if s.shape == Shape::Flat { s.h0 } else { s.h1 });
        }
        for p in &self.parts {
            let top = p.profile.iter().map(|s| s.h0.max(s.h1)).fold(0.0, f32::max);
            match p.plan {
                // a bar turns with the knob: take the circle its tip sweeps, half way up
                Plan::Bar { tip, .. } => point(tip, 0.5 * top),
                _ => {
                    for s in &p.profile {
                        point(s.r0, s.h0);
                        point(s.r1, if s.shape == Shape::Flat { s.h0 } else { s.h1 });
                    }
                }
            }
        }
        (0.5 * (up - down), wide.max(0.5 * (up + down)))
    }
    /// Stable-within-process identity of the model for cache keys.
    pub(crate) fn fingerprint(&self) -> u64 {
        hash_debug(self)
    }
}

impl Rig {
    pub(crate) fn fingerprint(&self) -> u64 {
        hash_debug(self)
    }
    /// Identity of everything but the panel colour (what the environment maps depend on).
    pub(crate) fn env_fingerprint(&self) -> u64 {
        hash_debug(&Rig { floor: Color(0), ..*self })
    }
}

fn hash_debug<T: std::fmt::Debug>(v: &T) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}
