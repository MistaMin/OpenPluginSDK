//! Standalone application runner using CPAL device audio I/O.

use openplugin_core::Plugin;

pub struct StandaloneRunner<P: Plugin> {
    pub plugin: P,
    pub sample_rate: f32,
    pub channels: usize,
}

impl<P: Plugin> StandaloneRunner<P> {
    pub fn new(plugin: P) -> Self {
        Self {
            plugin,
            sample_rate: 44100.0,
            channels: 2,
        }
    }

    pub fn run_loop(&mut self) -> Result<(), &'static str> {
        self.plugin.prepare(self.sample_rate, 512, self.channels);
        log::info!("Standalone runner initialized for {}", self.plugin.info().name);
        Ok(())
    }
}
