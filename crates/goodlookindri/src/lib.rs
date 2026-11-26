//! GoodLookin DRI — Rust toolkit for generated plugin interfaces.
//!
//! The design model, CSV, motion, level tap, look table, settings and spectrum match
//! [GoodLookinUI](https://github.com/MistaMin/GoodLookinUI). Drawing is a [`scene::Scene`]
//! of commands. The host replays them in its own window.

pub mod autosave;
pub mod color;
pub mod design;
pub mod editor;
pub mod host;
pub mod level;
pub mod light;
pub mod looks;
pub mod scene;
pub mod settings;
pub mod spectrum;
pub mod water;

pub mod hardware;
pub mod knobs;
pub mod knobs_flat;

#[cfg(test)]
mod string_tests {
    #[test]
    fn settings_string() {
        let s = crate::spectrum_draw::SpectrumSettings::default();
        assert_eq!(
            s.to_string(),
            "1,3,standard,1,3,24,0.85,1,ff8fe0c0,ffff3b4a,ffff9a1f,ffffe23a,ff5be85a,ff35e0d0,ff3f8ff0,ffc05cff,0,0"
        );
    }
}
pub mod meters;
pub mod retro;
pub mod switches;
pub mod spectrum_draw;
pub mod draw;
pub mod gallery;

pub mod readout;
pub mod layout;
pub mod knob3d;
