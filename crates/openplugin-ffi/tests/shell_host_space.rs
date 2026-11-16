//! Parameters shown normalized in the shells (`HostSpace::SHELLS`), as a plugin ported from an SDK
//! that published 0..1 values needs: the descriptor, conversions, text and the stored values speak
//! host values while the plugin keeps plain units. Its own test binary: the shell caches one plugin's
//! descriptor per binary.

use openplugin_core::{Category, HostSpace, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use openplugin_ffi::shell::{self, OpParamDesc, PARAM_DISCRETE, PARAM_LOG};

fn hz_text(v: f32) -> String {
    format!("{} Hz", v.round())
}
fn hz_parse(t: &str) -> Option<f32> {
    t.trim_end_matches("Hz").trim().parse().ok()
}

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new("freq", "Freq", 20.0, 1000.0, 100.0, Unit::Hz)
        .with_taper(Taper::Log)
        .with_normalized_in(HostSpace::SHELLS)
        .with_text(hz_text, hz_parse),
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
];

static INFO: PluginInfo = PluginInfo {
    id: "com.opengrid.test.host-space",
    name: "HostSpace",
    vendor: "Test",
    url: "",
    email: "",
    version: "1.0.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

#[derive(Default)]
struct Shown {
    values: [f32; 2],
}

impl Plugin for Shown {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        match INFO.params.iter().position(|p| p.id == id) {
            Some(i) => {
                self.values[i] = value;
                true
            }
            None => false,
        }
    }
    fn param(&self, id: &str) -> Option<f32> {
        INFO.params.iter().position(|p| p.id == id).map(|i| self.values[i])
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

#[test]
fn a_parameter_shown_normalized_speaks_0_to_1_through_the_shell_abi() {
    let mut desc = unsafe { std::mem::zeroed::<OpParamDesc>() };
    assert_eq!(unsafe { shell::write_param::<Shown>(0, &mut desc) }, 1);
    assert_eq!((desc.min, desc.max, desc.steps), (0.0, 1.0, 0));
    assert_eq!(desc.flags & (PARAM_LOG | PARAM_DISCRETE), 0, "linear continuous travel to the host");
    let expect = PARAMS[0].to_normalized(100.0) as f64;
    assert!((desc.default_plain - expect).abs() < 1e-7 && (desc.default_normalized - expect).abs() < 1e-7);

    // normalized travel is the host value itself
    assert!((shell::to_plain::<Shown>(0, 0.3) - 0.3).abs() < 1e-6);
    assert!((shell::to_normalized::<Shown>(0, 0.3) - 0.3).abs() < 1e-6);

    // text is the plugin's, for the host value; typed text comes back as a host value
    let mut buf = [0u8; 32];
    let n = unsafe { shell::param_format::<Shown>(0, 1.0, buf.as_mut_ptr(), buf.len()) };
    assert_eq!(std::str::from_utf8(&buf[..n]).unwrap(), "1000 Hz");
    let mut out = 0.0f64;
    assert_eq!(unsafe { shell::param_parse::<Shown>(0, c"20 Hz".as_ptr(), &mut out) }, 1);
    assert!(out.abs() < 1e-6);

    // the plugin's plane holds plain units; the shell stores and reads host values
    let instance = shell::create::<Shown>(48_000.0, 64, 2);
    assert!(!instance.is_null());
    unsafe {
        assert_eq!(shell::set_plain(instance, 0, 1.0), 1);
        assert!((shell::get_plain(instance, 0) - 1.0).abs() < 1e-6);
        assert!((shell::get_normalized(instance, 0) - 1.0).abs() < 1e-6);
        // the other parameter is unchanged: plain units, as before
        assert_eq!(shell::set_plain(instance, 1, -6.0), 1);
        assert_eq!(shell::get_plain(instance, 1), -6.0);
        let mut d1 = std::mem::zeroed::<OpParamDesc>();
        shell::write_param::<Shown>(1, &mut d1);
        assert_eq!((d1.min, d1.max), (-24.0, 24.0));
        shell::destroy(instance);
    }
}
