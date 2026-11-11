use openplugin::prelude::*;

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new("low_gain", "Low Gain", -15.0, 15.0, 0.0, Unit::Db),
    ParamInfo::new("low_freq", "Low Freq", 30.0, 450.0, 100.0, Unit::Hz).with_taper(Taper::Log),
    ParamInfo::new("high_gain", "High Gain", -15.0, 15.0, 0.0, Unit::Db),
    ParamInfo::new("high_freq", "High Freq", 1500.0, 16000.0, 8000.0, Unit::Hz).with_taper(Taper::Log),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.mistamin.example.console_eq",
    name: "Console EQ 70",
    vendor: "MistaMin",
    url: "https://github.com/MistaMin/OpenPluginSDK",
    email: "contact@mistamin.com",
    version: "0.1.1-beta.1",
    category: Category::Eq,
    params: &PARAMS,
    is_instrument: false,
    has_editor: true,
};

pub struct ConsoleEqPlugin {
    low_shelf: [Biquad; 2],
    high_shelf: [Biquad; 2],
    low_gain: f32,
    low_freq: f32,
    high_gain: f32,
    high_freq: f32,
    sample_rate: f32,
}

impl Default for ConsoleEqPlugin {
    fn default() -> Self {
        Self {
            low_shelf: [Biquad::new(), Biquad::new()],
            high_shelf: [Biquad::new(), Biquad::new()],
            low_gain: 0.0,
            low_freq: 100.0,
            high_gain: 0.0,
            high_freq: 8000.0,
            sample_rate: 44100.0,
        }
    }
}

impl ConsoleEqPlugin {
    fn update_filters(&mut self) {
        for f in &mut self.low_shelf {
            f.configure(FilterType::LowShelf, self.sample_rate, self.low_freq, 0.707, self.low_gain);
        }
        for f in &mut self.high_shelf {
            f.configure(FilterType::HighShelf, self.sample_rate, self.high_freq, 0.707, self.high_gain);
        }
    }

    /// Renders the GoodLookin DRI hardware faceplate and knobs.
    pub fn render_gui(&self, dl: &mut DrawList) {
        // 1. British Console olive/slate faceplate
        draw_faceplate(dl, 420.0, 220.0, FaceplateStyle::BritishConsole);

        // 2. Low band knob (British Red cap)
        let norm_lg = PARAMS[0].to_normalized(self.low_gain);
        draw_knob(dl, 110.0, 90.0, 36.0, norm_lg, "LOW GAIN", &PARAMS[0].format(self.low_gain), KnobStyle::Brit, Color::BRIT_RED);

        // 3. High band knob (British Blue cap)
        let norm_hg = PARAMS[2].to_normalized(self.high_gain);
        draw_knob(dl, 310.0, 90.0, 36.0, norm_hg, "HIGH GAIN", &PARAMS[2].format(self.high_gain), KnobStyle::Brit, Color::BRIT_BLUE);

        // 4. Center VU Meter
        draw_vu_meter(dl, 180.0, 50.0, 60.0, 45.0, -6.0, MeterStyle::AnalogVu);
    }
}

impl Plugin for ConsoleEqPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sample_rate = sample_rate;
        self.update_filters();
    }

    fn reset(&mut self) {
        for f in &mut self.low_shelf {
            f.reset();
        }
        for f in &mut self.high_shelf {
            f.reset();
        }
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match id {
            "low_gain" => {
                self.low_gain = PARAMS[0].clamp(value);
                self.update_filters();
                true
            }
            "low_freq" => {
                self.low_freq = PARAMS[1].clamp(value);
                self.update_filters();
                true
            }
            "high_gain" => {
                self.high_gain = PARAMS[2].clamp(value);
                self.update_filters();
                true
            }
            "high_freq" => {
                self.high_freq = PARAMS[3].clamp(value);
                self.update_filters();
                true
            }
            _ => false,
        }
    }

    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "low_gain" => Some(self.low_gain),
            "low_freq" => Some(self.low_freq),
            "high_gain" => Some(self.high_gain),
            "high_freq" => Some(self.high_freq),
            _ => None,
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for (ch, channel_vec) in io.iter_mut().enumerate() {
            let slice = &mut channel_vec[..frames];
            let ch_idx = ch.min(1);
            for s in slice {
                let y1 = self.low_shelf[ch_idx].process_sample(*s);
                let y2 = self.high_shelf[ch_idx].process_sample(y1);
                *s = y2;
            }
        }
    }
}

fn main() {
    println!("Console EQ 70 initialized.");
    let mut eq = ConsoleEqPlugin::default();
    eq.prepare(44100.0, 256, 2);
    eq.set_param("low_gain", 3.0);
    eq.set_param("high_gain", -2.0);

    let mut dl = DrawList::new();
    eq.render_gui(&mut dl);
    println!("GUI DrawList generated: {} drawing commands rendered.", dl.commands.len());
}
