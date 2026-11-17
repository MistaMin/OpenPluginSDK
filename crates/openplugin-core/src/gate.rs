//! Who may use the plugin object: the audio thread or a state call.
//!
//! `Plugin::save_state` and `load_state` take `&mut self` on a host thread
//! while the audio thread may be inside `process`. One atomic word arbitrates,
//! and the audio thread never waits on it:
//!
//! - The audio thread takes the gate with one compare-exchange
//!   ([`PluginGate::enter_audio`]). If a state call holds or has asked for the
//!   gate, it gets `None` and skips the block.
//! - A state call ([`PluginGate::enter_state`]) raises a request bit, then
//!   waits a bounded time for the block already running. Blocks that start
//!   meanwhile see the bit and skip, so the wait cannot be starved.
//!
//! Both bits live in one atomic, so for any block and any state call either
//! the block sees the request and skips, or the call sees the block and waits.
//! The format entries serialise state calls against each other themselves.
//!
//! A plugin without a chunk ([`has_state_chunk`]) needs no gate for its state:
//! the parameter plane is the whole of it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::Plugin;

const AUDIO: u32 = 1;
const STATE: u32 = 2;

/// The gate of one plugin instance. See the module documentation.
#[derive(Debug, Default)]
pub struct PluginGate(AtomicU32);

/// The audio thread's hold, for the length of one block.
#[derive(Debug)]
pub struct AudioHold<'a>(&'a AtomicU32);

impl Drop for AudioHold<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!AUDIO, Ordering::Release);
    }
}

/// A state call's hold on the plugin object. Released on drop, panic included.
#[derive(Debug)]
pub struct StateHold<'a>(&'a AtomicU32);

impl Drop for StateHold<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!STATE, Ordering::Release);
    }
}

impl PluginGate {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Audio side. `None` when a state call holds or wants the plugin object:
    /// the caller skips the block. Never waits, never allocates.
    pub fn enter_audio(&self) -> Option<AudioHold<'_>> {
        self.0.compare_exchange(0, AUDIO, Ordering::Acquire, Ordering::Relaxed).ok()?;
        Some(AudioHold(&self.0))
    }

    /// State side (a host thread, never the audio thread). Announces itself,
    /// so a block that starts from now on is skipped, then waits for the block
    /// already running. `None` after `wait`; the request is withdrawn.
    pub fn enter_state(&self, wait: Duration) -> Option<StateHold<'_>> {
        self.0.fetch_or(STATE, Ordering::AcqRel);
        let hold = StateHold(&self.0);
        let deadline = Instant::now() + wait;
        let mut spins = 0u32;
        while self.0.load(Ordering::Acquire) & AUDIO != 0 {
            if Instant::now() >= deadline {
                return None;
            }
            spins += 1;
            if spins < 64 {
                std::hint::spin_loop();
            } else {
                // Yield, not sleep: every block that starts while this waits is skipped.
                std::thread::yield_now();
            }
        }
        Some(hold)
    }
}

/// Whether the plugin has state beyond its parameters, asked once when a
/// format entry builds its instance: false when the plugin says so
/// ([`Plugin::state_is_parameters_only`]) or when the fresh plugin's
/// [`Plugin::save_state`] returns `None`. Without a chunk, saves and loads use
/// the parameter plane alone and never take the gate, so no block is skipped.
pub fn has_state_chunk(plugin: &mut dyn Plugin) -> bool {
    !plugin.state_is_parameters_only() && plugin.save_state().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_skips_while_a_state_call_holds_and_a_state_call_gives_up_on_a_block() {
        let gate = PluginGate::new();
        let state = gate.enter_state(Duration::ZERO).expect("idle gate");
        assert!(gate.enter_audio().is_none(), "a block during a state call skips");
        drop(state);
        let audio = gate.enter_audio().expect("free again");
        assert!(gate.enter_audio().is_none());
        assert!(gate.enter_state(Duration::from_millis(5)).is_none(), "bounded wait");
        assert!(gate.enter_audio().is_none(), "the block still holds");
        drop(audio);
        assert!(gate.enter_audio().is_some(), "the timed-out request was withdrawn");
    }
}
