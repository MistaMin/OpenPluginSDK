//! The CLAP bridge's event path with many parameters and many event batches per
//! block, through `openplugin-testing`'s bench. Ignored by default; run with
//! `cargo test --release -p openplugin-targets --test clap_param_bench -- --ignored --nocapture`.

use std::ffi::c_void;
use std::sync::OnceLock;

use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use openplugin_targets::clap_abi::*;
use openplugin_targets::ClapAdapter;
use openplugin_testing::bench::{bench, BenchOptions};

const PARAMS: usize = 500;
const BATCHES: usize = 200;
const BLOCK: usize = 512;

fn info() -> &'static PluginInfo {
    static INFO: OnceLock<PluginInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let params: Vec<ParamInfo> = (0..PARAMS)
            .map(|n| {
                let id: &'static str = Box::leak(format!("p{n}").into_boxed_str());
                ParamInfo::new(id, id, 0.0, 1.0, 0.0, Unit::None)
            })
            .collect();
        PluginInfo {
            id: "com.opengrid.test.parambench",
            name: "Param Bench",
            vendor: "OpenGrid",
            url: "https://github.com/MistaMin/OpenPluginSDK",
            email: "contact@mistamin.com",
            version: "0.0.1",
            category: Category::Utility,
            params: Box::leak(params.into_boxed_slice()),
            is_instrument: false,
            has_editor: false,
        }
    })
}

/// The plugin behind the bridge: it only stores what it is given.
struct Many(Vec<f32>);

impl Plugin for Many {
    fn info(&self) -> &'static PluginInfo {
        info()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn set_param_index(&mut self, index: usize, value: f32) -> bool {
        self.0[index] = value;
        true
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
}

/// Drives a CLAP instance of [`Many`] as the bench's plugin: each block carries
/// `BATCHES` parameter events at distinct sample positions.
struct Driver {
    adapter: ClapAdapter<Many>,
    lists: [Vec<clap_event_param_value>; 2],
    turn: usize,
}

// The bench owns the driver on one thread.
unsafe impl Send for Driver {}

unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
    unsafe { &*((*list).ctx as *const Vec<clap_event_param_value>) }.len() as u32
}

unsafe extern "C" fn get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let events = unsafe { &*((*list).ctx as *const Vec<clap_event_param_value>) };
    &events[index as usize].header
}

impl Driver {
    fn new() -> Self {
        let list = |value: f64| {
            (0..BATCHES)
                .map(|n| clap_event_param_value {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_param_value>() as u32,
                        time: (n * BLOCK / BATCHES) as u32,
                        space_id: 0,
                        type_: EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id: ((n * 7) % PARAMS) as u32,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value,
                })
                .collect()
        };
        Self { adapter: ClapAdapter::new(Many(vec![0.0; PARAMS])), lists: [list(0.25), list(0.75)], turn: 0 }
    }
}

impl Plugin for Driver {
    fn info(&self) -> &'static PluginInfo {
        info()
    }
    fn prepare(&mut self, sample_rate: f32, max_block: usize, _: usize) {
        let plugin = self.adapter.as_plugin();
        assert!(unsafe { (*plugin).activate.unwrap()(plugin, f64::from(sample_rate), 1, max_block as u32) });
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.turn ^= 1;
        let events = clap_input_events {
            ctx: &self.lists[self.turn] as *const Vec<clap_event_param_value> as *mut c_void,
            size: Some(size),
            get: Some(get),
        };
        let mut channels: Vec<*mut f32> = io.iter_mut().map(|channel| channel.as_mut_ptr()).collect();
        let mut buffer = clap_audio_buffer {
            data32: channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: channels.len() as u32,
            latency: 0,
            constant_mask: 0,
        };
        let buffer = std::ptr::addr_of_mut!(buffer);
        let process = clap_process {
            steady_time: 0,
            frames_count: frames as u32,
            transport: std::ptr::null(),
            audio_inputs: buffer,
            audio_outputs: buffer,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &events,
            out_events: std::ptr::null(),
        };
        let plugin = self.adapter.as_plugin();
        assert_eq!(unsafe { (*plugin).process.unwrap()(plugin, &process) }, PROCESS_CONTINUE);
    }
}

#[test]
#[ignore = "a measurement; run it in release by hand"]
fn many_parameters_many_batches() {
    let options = BenchOptions {
        sample_rates: vec![48_000.0],
        block_sizes: vec![BLOCK],
        channel_counts: Some(vec![2]),
        ..BenchOptions::default()
    };
    println!("{PARAMS} parameters, {BATCHES} event batches per block");
    println!("{}", bench(Driver::new, &options).to_text());
}
