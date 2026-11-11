use openplugin::prelude::*;

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db).with_taper(Taper::Linear),
    ParamInfo::new("drive", "Drive", 1.0, 10.0, 1.0, Unit::None).with_taper(Taper::Log),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.mistamin.example.gain",
    name: "OpenPlugin Gain",
    vendor: "MistaMin",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.1.1-beta.1",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

pub struct GainPlugin {
    gain_db: f32,
    drive: f32,
    linear_gain: f32,
}

impl Default for GainPlugin {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            drive: 1.0,
            linear_gain: 1.0,
        }
    }
}

impl Plugin for GainPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {
        self.linear_gain = 10.0_f32.powf(self.gain_db / 20.0);
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "gain" => {
                self.gain_db = PARAMS[0].clamp(value);
                self.linear_gain = 10.0_f32.powf(self.gain_db / 20.0);
                true
            }
            "drive" => {
                self.drive = PARAMS[1].clamp(value);
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "gain" => Some(self.gain_db),
            "drive" => Some(self.drive),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for channel in io.iter_mut() {
            let slice = &mut channel[..frames];
            for sample in slice {
                let driven = saturate_sample(*sample * self.linear_gain, self.drive, SaturationMode::Tape);
                *sample = driven;
            }
        }
    }
}

fn main() {
    println!("OpenPlugin Gain initialized successfully.");
    let mut plugin = GainPlugin::default();
    plugin.prepare(44100.0, 512, 2);
    plugin.set_param("gain", 6.0);
    let mut buffer = vec![vec![0.5; 512], vec![0.5; 512]];
    plugin.process(&mut buffer, 512);
    println!("Processed 512 frames, output sample 0 = {:.4}", buffer[0][0]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gain_processing() {
        let mut plugin = GainPlugin::default();
        plugin.prepare(48000.0, 64, 2);
        plugin.set_param("gain", 0.0);
        plugin.set_param("drive", 1.0);
        let mut buffer = vec![vec![0.2; 64], vec![0.2; 64]];
        plugin.process(&mut buffer, 64);
        assert!((buffer[0][0] - 0.2_f32.tanh()).abs() < 1e-4);
    }
}
