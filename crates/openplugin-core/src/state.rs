//! Versioned parameter snapshot and A/B compare.
//!
//! The blob is the parameter vector in descriptor order. Decode checks the
//! whole buffer before it returns, which is the rule OpenGridder uses for a
//! DAW state chunk: a corrupt document must not change a live value.
//! [`AbCompare`] saves the side being left, matching the insert's A/B switch.

use crate::{Plugin, PluginInfo};

pub const SNAPSHOT_MAGIC: &[u8; 4] = b"OPST";
pub const SNAPSHOT_VERSION: u16 = 1;

/// Encode one clamped value per parameter. `values` shorter than the descriptor
/// is padded with defaults. Extra entries are ignored.
pub fn encode_params(info: &PluginInfo, values: &[f32]) -> Vec<u8> {
    let count = info.params.len();
    let mut bytes = Vec::with_capacity(8 + count * 4);
    bytes.extend_from_slice(SNAPSHOT_MAGIC);
    bytes.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(count as u16).to_le_bytes());
    for (index, param) in info.params.iter().enumerate() {
        let raw = values.get(index).copied().unwrap_or(param.default);
        bytes.extend_from_slice(&param.clamp(raw).to_le_bytes());
    }
    bytes
}

/// Structural check, then clamped values. A bad magic, version, count, or
/// truncated tail returns `None` and does not invent a partial vector.
pub fn decode_params(info: &PluginInfo, bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 8 || &bytes[0..4] != SNAPSHOT_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    if version != SNAPSHOT_VERSION || count != info.params.len() {
        return None;
    }
    if bytes.len() != 8 + count * 4 {
        return None;
    }
    let mut values = Vec::with_capacity(count);
    for (index, param) in info.params.iter().enumerate() {
        let start = 8 + index * 4;
        let raw = f32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
        if !raw.is_finite() {
            return None;
        }
        values.push(param.clamp(raw));
    }
    Some(values)
}

pub const SESSION_MAGIC: &[u8; 4] = b"OPCH";
pub const SESSION_VERSION: u16 = 1;

/// Parameter plane plus the plugin's own chunk.
///
/// An old OPST blob decodes as a session with an empty chunk. A plugin that
/// already returns its own document from [`Plugin::save_state`] keeps that
/// document as `chunk`. The plane is stored beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub values: Vec<f32>,
    pub chunk: Vec<u8>,
}

/// OPCH envelope: magic, version, the OPST plane, then the opaque chunk.
pub fn encode_session(info: &PluginInfo, values: &[f32], chunk: &[u8]) -> Vec<u8> {
    let plane = encode_params(info, values);
    let mut bytes = Vec::with_capacity(10 + plane.len() + 4 + chunk.len());
    bytes.extend_from_slice(SESSION_MAGIC);
    bytes.extend_from_slice(&SESSION_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(plane.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&plane);
    bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(chunk);
    bytes
}

/// Accepts an OPCH session or a bare OPST plane. A bad length returns `None`.
pub fn decode_session(info: &PluginInfo, bytes: &[u8]) -> Option<Session> {
    if bytes.len() >= 4 && &bytes[0..4] == SNAPSHOT_MAGIC {
        return decode_params(info, bytes).map(|values| Session {
            values,
            chunk: Vec::new(),
        });
    }
    if bytes.len() < 10 || &bytes[0..4] != SESSION_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != SESSION_VERSION {
        return None;
    }
    let plane_len = u32::from_le_bytes(bytes[6..10].try_into().ok()?) as usize;
    let plane_end = 10usize.checked_add(plane_len)?;
    if bytes.len() < plane_end.checked_add(4)? {
        return None;
    }
    let values = decode_params(info, &bytes[10..plane_end])?;
    let rest = &bytes[plane_end..];
    let chunk_len = u32::from_le_bytes(rest[0..4].try_into().ok()?) as usize;
    if rest.len() != 4 + chunk_len {
        return None;
    }
    Some(Session {
        values,
        chunk: rest[4..].to_vec(),
    })
}

/// Apply a decoded vector only after every entry has been checked.
pub fn restore_params(plugin: &mut dyn Plugin, values: &[f32]) -> bool {
    let info = plugin.info();
    if values.len() != info.params.len() {
        return false;
    }
    let mut pending = Vec::with_capacity(values.len());
    for (param, value) in info.params.iter().zip(values) {
        if !value.is_finite() {
            return false;
        }
        pending.push(param.clamp(*value));
    }
    for (index, value) in pending.into_iter().enumerate() {
        if !plugin.set_param_index(index, value) {
            return false;
        }
    }
    true
}

/// Two full state blobs. The first toggle copies the live state onto the empty
/// side so it is a snapshot, and later toggles restore the side being entered.
#[derive(Clone, Debug, Default)]
pub struct AbCompare {
    a: Vec<u8>,
    b: Vec<u8>,
    on_b: bool,
}

impl AbCompare {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_b(&self) -> bool {
        self.on_b
    }

    pub fn toggle(&mut self, plugin: &mut dyn Plugin) -> bool {
        let Some(live) = plugin.save_state() else {
            return false;
        };
        if self.on_b {
            self.b = live;
            let next = if self.a.is_empty() {
                self.b.clone()
            } else {
                self.a.clone()
            };
            if plugin.load_state(&next) {
                self.on_b = false;
                true
            } else {
                false
            }
        } else {
            self.a = live;
            let next = if self.b.is_empty() {
                self.a.clone()
            } else {
                self.b.clone()
            };
            if plugin.load_state(&next) {
                self.on_b = true;
                true
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, ParamInfo, Unit};

    static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];

    static INFO: PluginInfo = PluginInfo {
        id: "com.example.state",
        name: "State",
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
        db: f32,
    }

    impl Plugin for Gain {
        fn info(&self) -> &'static PluginInfo {
            &INFO
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, id: &str, value: f32) -> bool {
            if id != "gain" {
                return false;
            }
            self.db = PARAMS[0].clamp(value);
            true
        }
        fn param(&self, id: &str) -> Option<f32> {
            (id == "gain").then_some(self.db)
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(encode_params(&INFO, &[self.db]))
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            match decode_params(&INFO, data) {
                Some(values) => restore_params(self, &values),
                None => false,
            }
        }
    }

    #[test]
    fn corrupt_snapshots_do_not_decode() {
        let bytes = encode_params(&INFO, &[6.0]);
        let values = decode_params(&INFO, &bytes).unwrap();
        assert_eq!(values, vec![6.0]);
        assert!(decode_params(&INFO, b"NOPE").is_none());
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(decode_params(&INFO, &truncated).is_none());
        let mut bad = bytes.clone();
        bad[8..12].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode_params(&INFO, &bad).is_none());
    }

    #[test]
    fn ab_keeps_both_sides() {
        let mut plugin = Gain { db: -6.0 };
        let mut ab = AbCompare::new();
        assert!(ab.toggle(&mut plugin));
        assert!(ab.on_b());
        assert_eq!(plugin.db, -6.0);
        plugin.set_param("gain", 3.0);
        assert!(ab.toggle(&mut plugin));
        assert!(!ab.on_b());
        assert_eq!(plugin.db, -6.0);
        assert!(ab.toggle(&mut plugin));
        assert_eq!(plugin.db, 3.0);
    }

    #[test]
    fn session_keeps_the_plane_and_the_chunk() {
        let chunk = b"{\"program\":2}";
        let bytes = encode_session(&INFO, &[3.5], chunk);
        let session = decode_session(&INFO, &bytes).unwrap();
        assert_eq!(session.values, vec![3.5]);
        assert_eq!(session.chunk, chunk);
        let plane = encode_params(&INFO, &[1.0]);
        let old = decode_session(&INFO, &plane).unwrap();
        assert_eq!(old.values, vec![1.0]);
        assert!(old.chunk.is_empty());
        let mut broken = bytes.clone();
        broken.pop();
        assert!(decode_session(&INFO, &broken).is_none());
    }
}
