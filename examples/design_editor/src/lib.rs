//! A small plugin whose editor is built from a GoodLookin design file.
//!
//! `design.csv` is the whole layout: five rows, each naming a parameter by id. The CSV is
//! embedded with `include_str!` and [`openplugin::dri::DesignEditor`] turns it into the
//! controls: a knob for Input and Tone, a fader for Drive (the row is tall and narrow), a
//! switch for Warm (a toggle parameter) and a key bank for Mode (a choice parameter).
//! Edit the CSV, rebuild, and the editor follows; no drawing or hit-testing code here.
//! Exported as CLAP.

use openplugin::core::{Editor, EditorHost};
use openplugin::dri::DesignEditor;
use openplugin::prelude::*;

static PARAMS: [ParamInfo; 5] = [
    ParamInfo::new("gain", "Input", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("tone", "Tone", 200.0, 12000.0, 12000.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("drive", "Drive", 0.0, 100.0, 0.0, Unit::Percent),
    ParamInfo::new("warm", "Warm", 0.0, 1.0, 0.0, Unit::Toggle),
    ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::Choice).with_choices(&["Clean", "Soft", "Hard"]),
];

pub static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.example.design-editor",
    name: "Design Editor",
    vendor: "OpenGrid",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "",
    version: "0.3.1",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: true,
};

/// The layout, embedded in the binary.
pub const DESIGN: &str = include_str!("../design.csv");

/// A gain, a tone filter and a clipper. Only enough DSP to have something to move.
pub struct DesignDemo {
    gain: f32,
    tone: f32,
    drive: f32,
    warm: bool,
    mode: u32,
    sample_rate: f32,
    low: Vec<f32>,
}

impl Default for DesignDemo {
    fn default() -> Self {
        Self { gain: 1.0, tone: 12000.0, drive: 0.0, warm: false, mode: 0, sample_rate: 48000.0, low: Vec::new() }
    }
}

impl Plugin for DesignDemo {
    fn state_is_parameters_only(&self) -> bool {
        true
    }
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _: usize, channels: usize) {
        self.sample_rate = sample_rate;
        self.low = vec![0.0; channels.max(2)];
    }
    fn reset(&mut self) {
        self.low.iter_mut().for_each(|s| *s = 0.0);
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(info) = INFO.param(id) else { return false };
        let v = info.clamp(value);
        match id {
            "gain" => self.gain = 10f32.powf(v / 20.0),
            "tone" => self.tone = v,
            "drive" => self.drive = v / 100.0,
            "warm" => self.warm = v > 0.5,
            "mode" => self.mode = v as u32,
            _ => return false,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        Some(match id {
            "gain" => 20.0 * self.gain.log10(),
            "tone" => self.tone,
            "drive" => self.drive * 100.0,
            "warm" => f32::from(u8::from(self.warm)),
            "mode" => self.mode as f32,
            _ => return None,
        })
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let a = 1.0 - (-std::f32::consts::TAU * self.tone / self.sample_rate.max(1.0)).exp();
        let pre = 1.0 + self.drive * if self.mode == 2 { 12.0 } else { 6.0 };
        for (ch, channel) in io.iter_mut().enumerate() {
            let state = self.low.get_mut(ch);
            let mut low = state.as_deref().copied().unwrap_or(0.0);
            for s in &mut channel[..frames] {
                low += a * (*s * self.gain - low);
                // A NaN or infinity in the state would stay for good, and a decaying
                // state would pass through denormal numbers on its way to zero.
                if !low.is_finite() || low.abs() < 1e-20 {
                    low = 0.0;
                }
                let x = low * pre;
                let y = match self.mode {
                    0 => x,
                    1 => x.tanh(),
                    _ => x.clamp(-1.0, 1.0),
                };
                *s = if self.warm { y.tanh() } else { y } / pre.sqrt();
            }
            if let Some(state) = state {
                *state = low;
            }
        }
    }
    fn note_ports(&self) -> NotePorts {
        NotePorts { input: false, output: false }
    }
}

/// The editor factory: the design, bound to [`INFO`]'s parameters.
pub fn make_editor(host: EditorHost) -> Option<Box<dyn Editor>> {
    DesignEditor::from_csv(DESIGN, &INFO, host).map(|e| Box::new(e) as Box<dyn Editor>)
}

openplugin::targets::export_clap!(DesignDemo, editor = make_editor);

#[cfg(test)]
mod tests {
    use super::*;
    use openplugin::dri::DesignOptions;

    #[test]
    fn every_row_binds_and_every_parameter_has_a_control() {
        let e = DesignEditor::try_new(DESIGN, &INFO, EditorHost::detached(), &DesignOptions::default().strict(true)).unwrap();
        assert!(e.issues().is_empty());
        assert_eq!(e.controls().len(), PARAMS.len());
        for p in PARAMS.iter() {
            assert!(e.control_for(p.id).is_some(), "{}", p.id);
        }
    }

    #[test]
    fn the_editor_renders() {
        let mut e = make_editor(EditorHost::detached()).unwrap();
        let (w, h) = e.size();
        let f = e.render(1.0);
        assert_eq!((f.width, f.height), (w as usize, h as usize));
        assert!(f.pixels.iter().any(|p| *p != f.pixels[0]));
    }

    #[test]
    fn the_engine_follows_its_parameters() {
        let mut p = DesignDemo::default();
        p.prepare(48000.0, 64, 2);
        assert!(p.set_param("gain", 6.0));
        assert!((p.param("gain").unwrap() - 6.0).abs() < 1e-4);
        assert!(!p.set_param("nope", 1.0));
        let mut io = vec![vec![0.5f32; 64]; 2];
        p.process(&mut io, 64);
        assert!(io.iter().flatten().all(|s| s.is_finite()));
    }
}
