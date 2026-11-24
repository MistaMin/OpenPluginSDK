//! Versioned parameter snapshot and A/B compare.
//!
//! Version 1 of the blob is the parameter vector in descriptor order. Version
//! 2 names every value by its parameter's string id, so it survives a
//! reordered or extended table; it is written for a table with identity pins
//! ([`crate::identity::state_is_keyed`]) and read for any table. Decode checks
//! the whole buffer before it returns: a corrupt document must not change a
//! live value.
//! [`AbCompare`] saves the side being left, matching the insert's A/B switch.

use crate::identity::{state_is_keyed, ParamIdMap, PinFormat};
use crate::{Plugin, PluginInfo};

/// The first four bytes of a parameter snapshot ([`encode_params`]).
pub const SNAPSHOT_MAGIC: &[u8; 4] = b"OPST";
/// The positional snapshot: one value per parameter in descriptor order.
pub const SNAPSHOT_VERSION: u16 = 1;
/// The snapshot by parameter id: `u16` id length, the id's bytes and the
/// value, once per parameter.
pub const SNAPSHOT_VERSION_KEYED: u16 = 2;

/// Encode one clamped value per parameter. `values` shorter than the descriptor
/// is padded with defaults. Extra entries are ignored. A table without
/// identity pins is written as version 1, as it always was; a pinned table as
/// version 2.
pub fn encode_params(info: &PluginInfo, values: &[f32]) -> Vec<u8> {
    let count = info.params.len();
    let keyed = state_is_keyed(info.params);
    let names: usize = if keyed { info.params.iter().map(|p| 2 + p.id.len()).sum() } else { 0 };
    let mut bytes = Vec::with_capacity(8 + count * 4 + names);
    bytes.extend_from_slice(SNAPSHOT_MAGIC);
    bytes.extend_from_slice(&(if keyed { SNAPSHOT_VERSION_KEYED } else { SNAPSHOT_VERSION }).to_le_bytes());
    bytes.extend_from_slice(&(count as u16).to_le_bytes());
    for (index, param) in info.params.iter().enumerate() {
        let raw = values.get(index).copied().unwrap_or(param.default);
        if keyed {
            bytes.extend_from_slice(&(param.id.len() as u16).to_le_bytes());
            bytes.extend_from_slice(param.id.as_bytes());
        }
        bytes.extend_from_slice(&param.clamp(raw).to_le_bytes());
    }
    bytes
}

/// Structural check, then clamped values in the order of `info.params`. A bad
/// magic, version, count, or truncated tail returns `None` and does not invent
/// a partial vector.
///
/// Version 1 into a table without pins: the values are taken by position. A
/// blob with fewer values than the table has parameters comes from an older
/// build, whose table ended earlier: its values go to the first parameters and
/// the rest get their defaults. A blob with more values, or none, is refused.
/// Version 1 into a pinned table: the blob comes from a
/// build made before the pins, in which a parameter's position was its
/// numeric id, so value `i` goes to the parameter whose CLAP id is `i`
/// (refused when there is none), and a parameter the old build did not have
/// gets its default. Version 2 into any table: each parameter takes the value
/// stored under its string id, or its default when the blob has none; values
/// under ids the table does not have are dropped; a blob that names none of
/// the table's parameters is refused as foreign.
pub fn decode_params(info: &PluginInfo, bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 8 || &bytes[0..4] != SNAPSHOT_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    match version {
        SNAPSHOT_VERSION if state_is_keyed(info.params) => decode_positional_into_keyed(info, count, &bytes[8..]),
        SNAPSHOT_VERSION => decode_positional(info, count, bytes),
        SNAPSHOT_VERSION_KEYED => decode_keyed(info, count, &bytes[8..]),
        _ => None,
    }
}

fn decode_positional(info: &PluginInfo, count: usize, bytes: &[u8]) -> Option<Vec<f32>> {
    if count > info.params.len() || (count == 0 && !info.params.is_empty()) {
        return None;
    }
    if bytes.len() != 8 + count * 4 {
        return None;
    }
    let mut values: Vec<f32> = info.params.iter().map(|param| param.default).collect();
    for (index, param) in info.params.iter().take(count).enumerate() {
        let start = 8 + index * 4;
        let raw = f32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
        if !raw.is_finite() {
            return None;
        }
        values[index] = param.clamp(raw);
    }
    Some(values)
}

fn decode_positional_into_keyed(info: &PluginInfo, count: usize, body: &[u8]) -> Option<Vec<f32>> {
    if count > info.params.len() || body.len() != count * 4 {
        return None;
    }
    let old_ids = ParamIdMap::new(info.params, PinFormat::Clap);
    let mut values: Vec<f32> = info.params.iter().map(|param| param.default).collect();
    for (position, chunk) in body.chunks_exact(4).enumerate() {
        let raw = f32::from_le_bytes(chunk.try_into().ok()?);
        let index = old_ids.index(position as u32)?;
        if !raw.is_finite() {
            return None;
        }
        values[index] = info.params[index].clamp(raw);
    }
    Some(values)
}

fn decode_keyed(info: &PluginInfo, count: usize, body: &[u8]) -> Option<Vec<f32>> {
    // Every entry is checked before any value is used. An entry is at least
    // six bytes, so the list cannot outgrow the input.
    let mut entries: Vec<(&[u8], f32)> = Vec::with_capacity(count.min(body.len() / 6));
    let mut rest = body;
    for _ in 0..count {
        if rest.len() < 2 {
            return None;
        }
        let length = u16::from_le_bytes([rest[0], rest[1]]) as usize;
        if length == 0 || rest.len() < 2 + length + 4 {
            return None;
        }
        let raw = f32::from_le_bytes(rest[2 + length..2 + length + 4].try_into().ok()?);
        if !raw.is_finite() {
            return None;
        }
        entries.push((&rest[2..2 + length], raw));
        rest = &rest[2 + length + 4..];
    }
    if !rest.is_empty() {
        return None;
    }
    let mut found = 0usize;
    let values: Vec<f32> = info
        .params
        .iter()
        .map(|param| match entries.iter().find(|entry| entry.0 == param.id.as_bytes()) {
            Some(entry) => {
                found += 1;
                param.clamp(entry.1)
            }
            None => param.default,
        })
        .collect();
    (found > 0 || (info.params.is_empty() && entries.is_empty())).then_some(values)
}

/// The first four bytes of a session ([`encode_session`]).
pub const SESSION_MAGIC: &[u8; 4] = b"OPCH";
/// The session layout version [`encode_session`] writes and [`decode_session`] reads.
pub const SESSION_VERSION: u16 = 1;

/// Parameter plane plus the plugin's own chunk.
///
/// An old OPST blob decodes as a session with an empty chunk. A plugin that
/// already returns its own document from [`Plugin::save_state`] keeps that
/// document as `chunk`. The plane is stored beside it.
///
/// Encoding and decoding allocate: main thread only.
///
/// ```
/// use openplugin_core::{decode_session, encode_session, Category, ParamInfo, PluginInfo, Unit};
/// static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// let bytes = encode_session(&INFO, &[-6.0], b"my chunk");
/// let session = decode_session(&INFO, &bytes).unwrap();
/// assert_eq!(session.values, [-6.0]);
/// assert_eq!(session.chunk, b"my chunk");
/// assert!(decode_session(&INFO, &bytes[..bytes.len() - 1]).is_none());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// One plain, clamped value per parameter, in descriptor order.
    pub values: Vec<f32>,
    /// The plugin's own chunk ([`Plugin::save_state`]); empty for none.
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

/// Apply a decoded vector only after every entry has been checked: false,
/// with nothing applied, when the length differs from the table or a value
/// is not finite. Values are clamped. A `set_param_index` that returns false
/// stops the loop there, leaving the earlier values applied. Main thread;
/// allocates.
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
///
/// It works through [`Plugin::save_state`] and [`Plugin::load_state`], so a
/// plugin whose state lives only in its parameters (`save_state` returns
/// `None`) cannot be toggled. Main thread; allocates.
///
/// ```
/// # use openplugin_core::{Category, ParamInfo, Plugin, PluginInfo, Unit};
/// # static PARAMS: [ParamInfo; 1] = [ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
/// # static INFO: PluginInfo = PluginInfo::new("com.example.gain", "Gain", "Example", "1.0.0", Category::Utility, &PARAMS);
/// # #[derive(Default)]
/// # struct Gain { db: f32 }
/// # impl Plugin for Gain {
/// #     fn info(&self) -> &'static PluginInfo { &INFO }
/// #     fn prepare(&mut self, _: f32, _: usize, _: usize) {}
/// #     fn reset(&mut self) {}
/// #     fn set_param(&mut self, id: &str, v: f32) -> bool { if id == "gain" { self.db = v; true } else { false } }
/// #     fn param(&self, id: &str) -> Option<f32> { (id == "gain").then_some(self.db) }
/// #     fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
/// #     fn save_state(&mut self) -> Option<Vec<u8>> { Some(self.db.to_le_bytes().to_vec()) }
/// #     fn load_state(&mut self, d: &[u8]) -> bool {
/// #         d.try_into().map(|b| self.db = f32::from_le_bytes(b)).is_ok()
/// #     }
/// # }
/// use openplugin_core::AbCompare;
/// let mut plugin = Gain { db: -3.0 };
/// let mut ab = AbCompare::new();
/// assert!(ab.toggle(&mut plugin)); // to B, a copy of A
/// plugin.set_param("gain", 6.0); // edit B
/// assert!(ab.toggle(&mut plugin)); // back to A
/// assert_eq!(plugin.param("gain"), Some(-3.0));
/// assert!(ab.toggle(&mut plugin) && ab.on_b());
/// assert_eq!(plugin.param("gain"), Some(6.0));
/// ```
#[derive(Clone, Debug, Default)]
pub struct AbCompare {
    a: Vec<u8>,
    b: Vec<u8>,
    on_b: bool,
}

impl AbCompare {
    /// On side A, both sides empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether side B is the live one.
    pub fn on_b(&self) -> bool {
        self.on_b
    }

    /// Save the live state to the current side and load the other one. True
    /// when the switch happened; false, staying on the same side, when
    /// `save_state` returns `None` or `load_state` refuses.
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

    static INFO: PluginInfo = PluginInfo::new(
        "com.example.state",
        "State",
        "Test",
        "0",
        Category::Utility,
        &PARAMS,
    )
        .with_url("https://example.com")
        .with_email("a@b.c");

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
        fn process(&mut self, _: &mut [&mut [f32]], _: usize) {}
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
