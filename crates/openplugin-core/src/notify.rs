//! Telling the host that something it read from the plugin has changed.
//!
//! A host reads a plugin's latency and its parameter descriptions once and
//! keeps them. When either changes at run time the plugin says so through a
//! [`HostNotifier`], and the format entry tells the host the way its format
//! asks:
//!
//! | format | latency | parameter info |
//! |---|---|---|
//! | CLAP | `request_restart`, then `latency.changed` in the next `activate` | `params.rescan(INFO \| TEXT \| VALUES)` on the main thread |
//! | VST3 | `restartComponent(kLatencyChanged)` | `restartComponent(kParamTitlesChanged \| kParamValuesChanged)` |
//! | Audio Unit | `kAudioUnitProperty_Latency` changed | `kAudioUnitProperty_ParameterList` and `ParameterInfo` changed |
//! | AAX | `SetSignalLatency` | `AAX_eNotificationEvent_ParameterNameChanged` per parameter |
//! | LV2 | the latency output port | (none in LV2) |
//!
//! The format entry hands the plugin its notifier once, through
//! [`crate::Plugin::set_host_notifier`], on the main thread, before any audio.
//! The plugin keeps it (it is a cloneable handle) and calls
//! [`HostNotifier::latency_changed`] or [`HostNotifier::param_info_changed`]
//! from any thread, `process` included: each call is one atomic `fetch_or`,
//! with no allocation, no lock and no system call. The entry picks the flags
//! up on its own schedule (after a block, on the host's main-thread callback,
//! on a shell's main-thread timer) and tells the host once per change.
//!
//! The entries also compare [`crate::Plugin::latency`] after every block, so
//! a latency that changes inside `process` reaches the host without a call.
//! [`HostNotifier::latency_changed`] is for a change made elsewhere (a state
//! load, a main-thread call, an editor): CLAP then asks its host for a restart
//! at once, even if no block runs; the shells report a latency once a block, a
//! reset or a state load has read the new value, and never one the host has
//! already read.
//!
//! Parameter info means names, units and value text. They come from
//! [`crate::Plugin::info`]: a plugin that changes them returns another
//! `&'static PluginInfo` whose parameters are the same ones, in the same
//! order, with the same ids, ranges and flags, and then calls
//! [`HostNotifier::param_info_changed`]. Adding, removing or re-ranging
//! parameters at run time is not supported.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// The plugin's handle for telling its host about changes. Cheap to clone;
/// every method is safe on any thread, the audio thread included.
#[derive(Clone, Debug, Default)]
pub struct HostNotifier {
    flags: Arc<AtomicU32>,
}

impl HostNotifier {
    /// The latency ([`crate::Plugin::latency`]) changed.
    pub const LATENCY: u32 = 1;
    /// Parameter names, units or value text ([`crate::Plugin::info`]) changed.
    pub const PARAM_INFO: u32 = 2;
    /// Every flag this version defines.
    pub const ALL: u32 = Self::LATENCY | Self::PARAM_INFO;

    /// A notifier with nothing pending. A format entry makes one per instance
    /// and keeps a clone; a notifier nobody reads (in a test, or a format
    /// that cannot tell its host) costs only the flag word.
    pub fn new() -> Self {
        Self::default()
    }

    /// Say that [`crate::Plugin::latency`] now returns a new value. Any
    /// thread; no allocation, no lock.
    pub fn latency_changed(&self) {
        self.raise(Self::LATENCY);
    }

    /// Say that the parameter names, units or value text changed. Any
    /// thread; no allocation, no lock.
    pub fn param_info_changed(&self) {
        self.raise(Self::PARAM_INFO);
    }

    /// Raise `flags` (bits outside [`Self::ALL`] are ignored). Returns the
    /// flags that were already pending. Any thread; one atomic operation.
    pub fn raise(&self, flags: u32) -> u32 {
        self.flags.fetch_or(flags & Self::ALL, Ordering::AcqRel)
    }

    /// The flags raised and not yet taken. For format entries.
    pub fn pending(&self) -> u32 {
        self.flags.load(Ordering::Acquire)
    }

    /// Take every pending flag and clear them, so each change is told once.
    /// For format entries.
    pub fn take(&self) -> u32 {
        self.flags.swap(0, Ordering::AcqRel)
    }

    /// Take only `flags`, leaving the others pending. For format entries.
    pub fn take_some(&self, flags: u32) -> u32 {
        self.flags.fetch_and(!flags, Ordering::AcqRel) & flags
    }

    /// Whether `other` is a clone of this notifier (they share one flag word).
    pub fn same_as(&self, other: &HostNotifier) -> bool {
        Arc::ptr_eq(&self.flags, &other.flags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_taken_once() {
        let host = HostNotifier::new();
        let plugin = host.clone();
        assert!(plugin.same_as(&host));
        assert_eq!(host.take(), 0);
        plugin.latency_changed();
        plugin.latency_changed();
        assert_eq!(host.pending(), HostNotifier::LATENCY);
        assert_eq!(host.take(), HostNotifier::LATENCY);
        assert_eq!(host.take(), 0);
        plugin.param_info_changed();
        plugin.latency_changed();
        assert_eq!(host.take_some(HostNotifier::PARAM_INFO), HostNotifier::PARAM_INFO);
        assert_eq!(host.take(), HostNotifier::LATENCY);
    }

    #[test]
    fn unknown_bits_are_ignored() {
        let host = HostNotifier::new();
        assert_eq!(host.raise(0xF0), 0);
        assert_eq!(host.pending(), 0);
        assert!(!host.same_as(&HostNotifier::new()));
    }

    #[test]
    fn many_threads_raise_without_loss() {
        let host = HostNotifier::new();
        let threads: Vec<_> = (0..4)
            .map(|i| {
                let notifier = host.clone();
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        if i % 2 == 0 {
                            notifier.latency_changed();
                        } else {
                            notifier.param_info_changed();
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(host.take(), HostNotifier::ALL);
    }
}
