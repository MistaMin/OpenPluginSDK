//! The parameter snapshot by id (`OPST` version 2), which a table with identity
//! pins writes: it must load into a table with another order, with more
//! parameters or with fewer, and the positional snapshot (version 1) of the
//! build before the pins must still load. Hostile bytes are refused whole.

use openplugin_core::identity::state_is_keyed;
use openplugin_core::{
    decode_params, decode_session, encode_params, encode_session, Category, ParamInfo, PluginInfo, Unit, SNAPSHOT_VERSION,
    SNAPSHOT_VERSION_KEYED,
};

const GAIN: ParamInfo = ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db);
const MIX: ParamInfo = ParamInfo::new("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent);
const MODE: ParamInfo = ParamInfo::new("mode", "Mode", 0.0, 3.0, 1.0, Unit::None).with_steps(4);
const TONE: ParamInfo = ParamInfo::new("tone", "Tone", 20.0, 20_000.0, 1_000.0, Unit::Hz);

const fn info(params: &'static [ParamInfo]) -> PluginInfo {
    PluginInfo {
        id: "com.example.keyed",
        name: "Keyed",
        vendor: "Test",
        url: "",
        email: "",
        version: "1",
        category: Category::Utility,
        params,
        is_instrument: false,
        has_editor: false,
    }
}

/// The release before pins: three parameters, ids are positions.
static OLD_PARAMS: [ParamInfo; 3] = [GAIN, MIX, MODE];
static OLD: PluginInfo = info(&OLD_PARAMS);

/// The same table with every parameter pinned at its old position.
static PINNED_PARAMS: [ParamInfo; 3] = [GAIN.with_host_id(0), MIX.with_host_id(1), MODE.with_host_id(2)];
static PINNED: PluginInfo = info(&PINNED_PARAMS);

/// A later release: reordered, and one more parameter.
static NEXT_PARAMS: [ParamInfo; 4] = [MODE.with_host_id(2), TONE.with_host_id(3), GAIN.with_host_id(0), MIX.with_host_id(1)];
static NEXT: PluginInfo = info(&NEXT_PARAMS);

/// A port: the ids are another SDK's, nothing like positions.
static PORT_PARAMS: [ParamInfo; 3] = [MIX.with_host_id(0x6D69_7800), GAIN.with_host_id(0x6761_696E), MODE.with_host_id(77)];
static PORT: PluginInfo = info(&PORT_PARAMS);

fn version(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[4], bytes[5]])
}

#[test]
fn a_table_without_pins_writes_the_positional_snapshot_byte_for_byte() {
    assert!(!state_is_keyed(OLD.params));
    let bytes = encode_params(&OLD, &[6.0, 50.0, 2.0]);
    let mut want = b"OPST".to_vec();
    want.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    want.extend_from_slice(&3u16.to_le_bytes());
    for value in [6.0f32, 50.0, 2.0] {
        want.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(bytes, want);
    assert_eq!(decode_params(&OLD, &bytes), Some(vec![6.0, 50.0, 2.0]));
    // A blob from an older build, whose table ended earlier: its values, then defaults.
    let older = encode_params(&info(&OLD_PARAMS[..2]), &[6.0, 50.0]);
    assert_eq!(decode_params(&OLD, &older), Some(vec![6.0, 50.0, MODE.default]));
    // A blob with more values than the table has parameters, or with none, is still refused.
    assert_eq!(decode_params(&info(&OLD_PARAMS[..2]), &bytes), None);
    assert_eq!(decode_params(&OLD, &encode_params(&info(&[]), &[])), None);
}

#[test]
fn a_pinned_table_writes_each_value_under_its_id() {
    assert!(state_is_keyed(PINNED.params));
    let bytes = encode_params(&PINNED, &[6.0, 50.0, 2.0]);
    assert_eq!(version(&bytes), SNAPSHOT_VERSION_KEYED);
    let mut want = b"OPST".to_vec();
    want.extend_from_slice(&2u16.to_le_bytes());
    want.extend_from_slice(&3u16.to_le_bytes());
    for (id, value) in [("gain", 6.0f32), ("mix", 50.0), ("mode", 2.0)] {
        want.extend_from_slice(&(id.len() as u16).to_le_bytes());
        want.extend_from_slice(id.as_bytes());
        want.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(bytes, want);
    assert_eq!(decode_params(&PINNED, &bytes), Some(vec![6.0, 50.0, 2.0]));
}

#[test]
fn a_keyed_snapshot_follows_its_parameters_through_a_reorder_and_a_new_parameter() {
    let saved = encode_params(&PINNED, &[6.0, 50.0, 2.0]);
    // NEXT is mode, tone (new), gain, mix.
    assert_eq!(decode_params(&NEXT, &saved), Some(vec![2.0, 1_000.0, 6.0, 50.0]));
    // And back: the newer table's snapshot in the older one; tone is dropped.
    let newer = encode_params(&NEXT, &[3.0, 440.0, -12.0, 25.0]);
    assert_eq!(decode_params(&PINNED, &newer), Some(vec![-12.0, 25.0, 3.0]));
    // The same through the session envelope, chunk included.
    let session = encode_session(&PINNED, &[6.0, 50.0, 2.0], b"chunk");
    let opened = decode_session(&NEXT, &session).unwrap();
    assert_eq!(opened.values, vec![2.0, 1_000.0, 6.0, 50.0]);
    assert_eq!(opened.chunk, b"chunk");
    // A table without pins reads a keyed snapshot too (the pins were taken out again).
    assert_eq!(decode_params(&OLD, &saved), Some(vec![6.0, 50.0, 2.0]));
}

#[test]
fn the_positional_snapshot_of_the_build_before_the_pins_lands_by_old_position() {
    // Saved by the unpinned release: position is the numeric id.
    let old = encode_params(&OLD, &[6.0, 50.0, 2.0]);
    assert_eq!(version(&old), SNAPSHOT_VERSION);
    // The pinned release in the same order.
    assert_eq!(decode_params(&PINNED, &old), Some(vec![6.0, 50.0, 2.0]));
    // The reordered release: value i goes to the parameter whose id is i; tone gets its default.
    assert_eq!(decode_params(&NEXT, &old), Some(vec![2.0, 1_000.0, 6.0, 50.0]));
    // A positional snapshot with more values than the table has ids for is another plugin's.
    static FOUR: [ParamInfo; 4] = [GAIN, MIX, MODE, TONE];
    let four = encode_params(&info(&FOUR), &[1.0, 2.0, 3.0, 400.0]);
    assert_eq!(decode_params(&PINNED, &four), None);
    // A port never had a positional build: its ids are not positions, so position 0 names nothing.
    assert_eq!(decode_params(&PORT, &old), None);
}

#[test]
fn values_are_clamped_and_snapped_to_the_table_that_reads_them() {
    let wild = encode_params(&PINNED, &[1.0e9, -5.0, 1.6]);
    assert_eq!(decode_params(&PINNED, &wild), Some(vec![24.0, 0.0, 2.0]));
    assert_eq!(decode_params(&NEXT, &wild), Some(vec![2.0, 1_000.0, 24.0, 0.0]));
}

/// A keyed blob from parts: `(id, value bits)` entries after a header that claims `count`.
fn blob(count: u16, entries: &[(&[u8], [u8; 4])]) -> Vec<u8> {
    let mut bytes = b"OPST".to_vec();
    bytes.extend_from_slice(&SNAPSHOT_VERSION_KEYED.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    for (id, value) in entries {
        bytes.extend_from_slice(&(id.len() as u16).to_le_bytes());
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(value);
    }
    bytes
}

#[test]
fn hostile_keyed_snapshots_are_refused_whole() {
    let one = 1.0f32.to_le_bytes();
    let good = blob(2, &[(b"gain", one), (b"mix", 50.0f32.to_le_bytes())]);
    assert_eq!(decode_params(&PINNED, &good), Some(vec![1.0, 50.0, 1.0]));
    // Cut anywhere, or longer by anything.
    for cut in 0..good.len() {
        assert_eq!(decode_params(&PINNED, &good[..cut]), None, "cut at {cut}");
        assert!(decode_session(&PINNED, &good[..cut]).is_none(), "cut at {cut}");
    }
    let mut longer = good.clone();
    longer.push(0);
    assert_eq!(decode_params(&PINNED, &longer), None);
    // The count disagrees with the entries.
    assert_eq!(decode_params(&PINNED, &blob(3, &[(b"gain", one), (b"mix", one)])), None);
    assert_eq!(decode_params(&PINNED, &blob(1, &[(b"gain", one), (b"mix", one)])), None);
    assert_eq!(decode_params(&PINNED, &blob(u16::MAX, &[(b"gain", one)])), None);
    // A value that is not a number.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(decode_params(&PINNED, &blob(2, &[(b"gain", one), (b"mix", bad.to_le_bytes())])), None);
    }
    // An empty id, and an id length that runs past the end.
    assert_eq!(decode_params(&PINNED, &blob(1, &[(b"", one)])), None);
    let mut overrun = blob(1, &[(b"gain", one)]);
    overrun[8] = 0xFF;
    overrun[9] = 0xFF;
    assert_eq!(decode_params(&PINNED, &overrun), None);
    // A snapshot that names none of the table's parameters is another plugin's.
    assert_eq!(decode_params(&PINNED, &blob(2, &[(b"size", one), (b"decay", one)])), None);
    assert_eq!(decode_params(&PINNED, &blob(0, &[])), None);
    // Ids that are not text are compared as bytes and match nothing.
    assert_eq!(decode_params(&PINNED, &blob(2, &[(&[0xFF, 0xFE], one), (b"gain", one)])), Some(vec![1.0, 100.0, 1.0]));
    // An unknown version is still refused.
    let mut future = good.clone();
    future[4] = 3;
    assert_eq!(decode_params(&PINNED, &future), None);
}

#[test]
fn every_single_byte_change_is_refused_or_gives_values_in_range() {
    let good = encode_params(&NEXT, &[3.0, 440.0, -12.0, 25.0]);
    for at in 0..good.len() {
        for flip in [0x01u8, 0x80, 0xFF] {
            let mut bytes = good.clone();
            bytes[at] ^= flip;
            for table in [&NEXT, &PINNED, &OLD, &PORT] {
                if let Some(values) = decode_params(table, &bytes) {
                    assert_eq!(values.len(), table.params.len());
                    for (param, value) in table.params.iter().zip(&values) {
                        assert!(value.is_finite() && *value >= param.min && *value <= param.max, "byte {at} ^ {flip:#x}: {} = {value}", param.id);
                    }
                }
            }
        }
    }
}

#[test]
fn a_repeated_id_in_the_blob_takes_the_first_value() {
    let twice = blob(2, &[(b"gain", 3.0f32.to_le_bytes()), (b"gain", 9.0f32.to_le_bytes())]);
    assert_eq!(decode_params(&PINNED, &twice), Some(vec![3.0, 100.0, 1.0]));
}
