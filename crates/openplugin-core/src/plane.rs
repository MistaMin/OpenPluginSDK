//! Lock-free parameter plane.
//!
//! OpenGridder's native insert keeps one `AtomicU32` per parameter. The editor
//! and the host write bits; the audio thread calls [`ParamCursor::pull`] and
//! only then touches the plugin, and only for slots that changed. Water Channel
//! uses the same rule: the audio thread does not take a mutex to read a knob.
//!
//! Storage is allocated in [`ParamPlane::new`]. `store` and `load` do not allocate.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{ParamInfo, Plugin, PluginInfo};

/// Shared parameter bits. `Sync` so the editor thread and the audio thread can
/// both reach it. The plugin object itself stays on the audio thread.
pub struct ParamPlane {
    values: Box<[AtomicU32]>,
    infos: &'static [ParamInfo],
    revision: AtomicU64,
}

impl ParamPlane {
    /// One slot per parameter, initialised to the clamped default.
    pub fn new(info: &'static PluginInfo) -> Self {
        let values = info
            .params
            .iter()
            .map(|param| AtomicU32::new(param.clamp(param.default).to_bits()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            values,
            infos: info.params,
            revision: AtomicU64::new(1),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn infos(&self) -> &'static [ParamInfo] {
        self.infos
    }

    /// Monotonic count of stores that changed a slot. Never reset, so a reader
    /// cannot confuse a cleared counter with "nothing happened".
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Clamps `value` (NaN becomes the default, infinities sit on the ends) and
    /// publishes the bits. Returns false when `index` is outside the plane.
    pub fn store(&self, index: usize, value: f32) -> bool {
        let Some(info) = self.infos.get(index) else {
            return false;
        };
        let bits = info.clamp(value).to_bits();
        let previous = self.values[index].swap(bits, Ordering::Relaxed);
        if previous != bits {
            self.revision.fetch_add(1, Ordering::Release);
        }
        true
    }

    pub fn store_id(&self, id: &str, value: f32) -> bool {
        match self.infos.iter().position(|param| param.id == id) {
            Some(index) => self.store(index, value),
            None => false,
        }
    }

    /// Latest clamped value. A torn or non-finite bit pattern is clamped again.
    pub fn load(&self, index: usize) -> Option<f32> {
        let info = self.infos.get(index)?;
        let raw = f32::from_bits(self.values[index].load(Ordering::Relaxed));
        Some(info.clamp(raw))
    }

    pub fn load_id(&self, id: &str) -> Option<f32> {
        let index = self.infos.iter().position(|param| param.id == id)?;
        self.load(index)
    }

    pub(crate) fn bits(&self, index: usize) -> Option<u32> {
        self.values
            .get(index)
            .map(|slot| slot.load(Ordering::Relaxed))
    }

    /// Copy the plugin's current values into the plane. Call from the thread
    /// that owns the plugin, before the audio callback starts.
    pub fn seed_from(&self, plugin: &dyn Plugin) {
        for (index, param) in self.infos.iter().enumerate() {
            let value = plugin.param(param.id).unwrap_or(param.default);
            self.store(index, value);
        }
    }
}

/// Remembers the bits last applied to the plugin. The first [`Self::pull`]
/// applies every slot, because the seen bits start at `u32::MAX`.
pub struct ParamCursor {
    seen: Box<[u32]>,
}

impl ParamCursor {
    pub fn new(count: usize) -> Self {
        Self {
            seen: vec![u32::MAX; count].into_boxed_slice(),
        }
    }

    /// Forget every slot so the next pull calls `set_param` again.
    /// Use this after `prepare`, which may rebuild DSP from the parameter values.
    pub fn invalidate(&mut self) {
        self.seen.fill(u32::MAX);
    }

    /// Apply slots whose bits differ from the last pull.
    /// `set_param_index` runs on the caller, which must be the audio thread.
    pub fn pull(&mut self, plane: &ParamPlane, plugin: &mut dyn Plugin) -> usize {
        let count = self.seen.len().min(plane.len());
        let mut changed = 0;
        for index in 0..count {
            let Some(bits) = plane.bits(index) else {
                continue;
            };
            if self.seen[index] == bits {
                continue;
            }
            let _ = plugin.set_param_index(index, f32::from_bits(bits));
            self.seen[index] = bits;
            changed += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, Taper, Unit};

    static PARAMS: [ParamInfo; 2] = [
        ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
        ParamInfo::new("drive", "Drive", 1.0, 10.0, 1.0, Unit::None).with_taper(Taper::Log),
    ];

    static INFO: PluginInfo = PluginInfo {
        id: "com.example.plane",
        name: "Plane",
        vendor: "Test",
        url: "https://example.com",
        email: "a@b.c",
        version: "0",
        category: Category::Utility,
        params: &PARAMS,
        is_instrument: false,
        has_editor: false,
    };

    struct Gain {
        values: [f32; 2],
        sets: usize,
    }

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            let Some(index) = PARAMS.iter().position(|param| param.id == id) else {
                return false;
            };
            self.values[index] = PARAMS[index].clamp(value);
            self.sets += 1;
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            PARAMS
                .iter()
                .position(|param| param.id == id)
                .map(|index| self.values[index])
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
    }

    #[test]
    fn hostile_values_are_clamped_and_unchanged_slots_do_not_bump_revision() {
        let plane = ParamPlane::new(&INFO);
        let revision = plane.revision();
        assert_eq!(plane.load(0), Some(0.0));
        assert!(plane.store(0, f32::NAN));
        assert_eq!(plane.load(0), Some(0.0));
        assert_eq!(plane.revision(), revision);
        assert!(plane.store(0, f32::INFINITY));
        assert_eq!(plane.load(0), Some(24.0));
        assert!(plane.revision() > revision);
        assert!(!plane.store(9, 1.0));
        assert_eq!(plane.load_id("drive"), Some(1.0));
    }

    #[test]
    fn cursor_applies_each_slot_once_until_it_changes() {
        let plane = ParamPlane::new(&INFO);
        plane.store_id("gain", 6.0);
        let mut plugin = Gain {
            values: [0.0, 1.0],
            sets: 0,
        };
        let mut cursor = ParamCursor::new(plane.len());
        assert_eq!(cursor.pull(&plane, &mut plugin), 2);
        assert_eq!(plugin.values[0], 6.0);
        assert_eq!(cursor.pull(&plane, &mut plugin), 0);
        plane.store(0, -3.0);
        assert_eq!(cursor.pull(&plane, &mut plugin), 1);
        assert_eq!(plugin.values[0], -3.0);
        cursor.invalidate();
        assert_eq!(cursor.pull(&plane, &mut plugin), 2);
    }
}
