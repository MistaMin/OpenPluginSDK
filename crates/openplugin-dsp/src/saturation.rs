//! Non-linear tape/tube saturation and harmonic drive curves.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaturationMode {
    Tape,
    Tube,
    HardClip,
    SoftClip,
}

#[inline]
pub fn saturate_sample(x: f32, drive: f32, mode: SaturationMode) -> f32 {
    let driven = x * drive.max(1.0);
    match mode {
        SaturationMode::Tape => {
            // Smooth hyperbolic tangent with odd harmonic compression
            driven.tanh()
        }
        SaturationMode::Tube => {
            // Asymmetric soft saturation producing 2nd-order harmonics
            if driven >= 0.0 {
                driven / (1.0 + driven)
            } else {
                -((-driven) / (1.0 + (-driven * 0.6)))
            }
        }
        SaturationMode::SoftClip => {
            // Classic polynomial soft clipper
            if driven > 1.5 {
                1.0
            } else if driven < -1.5 {
                -1.0
            } else {
                driven - (driven * driven * driven) / 6.75
            }
        }
        SaturationMode::HardClip => driven.clamp(-1.0, 1.0),
    }
}
