//! Hostile-input fuzzing for the state decoders and the plugin state load path.
//!
//! The corpus is deterministic: [`SEED`] is fixed and every generator is a small
//! 32-bit LCG, so a failing input reproduces on every run. The mutator starts
//! from three valid documents (a bare OPST plane, the test plugin's own chunk,
//! and an OPCH session that carries that chunk) and flips, replaces, inserts,
//! deletes and truncates bytes.
//!
//! Every decodable result must be a descriptor-sized vector of finite values
//! clamped to the parameter range. Every rejection must leave the live plugin
//! untouched, and a header that declares a multi-gigabyte length must not make
//! the decoder allocate memory from that number.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use openplugin_core::{
    decode_params, decode_session, encode_params, encode_session, restore_params, Category,
    ParamInfo, Plugin, PluginInfo, Unit, SESSION_MAGIC, SESSION_VERSION, SNAPSHOT_MAGIC,
    SNAPSHOT_VERSION,
};

/// Deterministic corpus seed, `0x4F50_5344` ("OPSD"). Every generator in this
/// file is a 32-bit LCG, so the same bytes are tried on every run.
const SEED: u32 = 0x4F50_5344;
/// Mutations over the three valid base documents.
const MUTATION_ITERATIONS: usize = 4_096;
/// Random buffers fed straight to the plugin's state load path.
const RANDOM_LOAD_ITERATIONS: usize = 2_048;
/// A hostile header may declare a 4 GiB length. The decoder may allocate its
/// result vector, but never memory proportional to such a field.
const ALLOCATION_LIMIT: usize = 64 * 1024;

struct CountingAllocator;

thread_local! {
    /// Bytes requested on the current test thread. libtest gives each test its
    /// own thread, so one test's allocation does not disturb another's count.
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATED.try_with(|bytes| bytes.set(bytes.get() + layout.size()));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATED.try_with(|bytes| bytes.set(bytes.get() + new_size));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Run `f` and report how many bytes the global allocator handed out meanwhile.
fn allocated_bytes<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let _ = ALLOCATED.try_with(|bytes| bytes.set(0));
    let value = f();
    let allocated = ALLOCATED.try_with(Cell::get).unwrap_or(0);
    (value, allocated)
}

/// Small 32-bit LCG (Numerical Recipes constants). Low entropy is fine here:
/// the fuzzer needs reproducible junk, not cryptographic randomness.
struct Lcg(u32);

impl Lcg {
    const fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    /// Uniform-enough value in `0..bound`; `bound` must be non-zero.
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u32() as usize) % bound
    }
}

const PARAM_COUNT: usize = 4;

static MODES: [&str; 3] = ["Room", "Hall", "Plate"];

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    ParamInfo::new("freq", "Freq", 20.0, 20_000.0, 1_000.0, Unit::Hz),
    ParamInfo::new("mix", "Mix", 0.0, 100.0, 50.0, Unit::Percent),
    ParamInfo::new("mode", "Mode", 0.0, 2.0, 0.0, Unit::Choice).with_choices(&MODES),
];

const DEFAULT_VALUES: [f32; PARAM_COUNT] = [0.0, 1_000.0, 50.0, 0.0];

static INFO: PluginInfo = PluginInfo {
    id: "com.example.state_hostile_input",
    name: "State Hostile Input",
    vendor: "Test",
    url: "https://example.com",
    email: "test@example.com",
    version: "1.0.0",
    category: Category::Utility,
    params: &PARAMS,
    is_instrument: false,
    has_editor: false,
};

fn valid_values() -> [f32; PARAM_COUNT] {
    [-6.5, 440.0, 25.0, 2.0]
}

/// Clamp a raw vector the way the decoders do, in descriptor order.
fn clamped(values: &[f32]) -> [f32; PARAM_COUNT] {
    let mut out = DEFAULT_VALUES;
    for (index, value) in values.iter().take(PARAM_COUNT).enumerate() {
        out[index] = PARAMS[index].clamp(*value);
    }
    out
}

// The test plugin's own document: a small opaque chunk of magic, version,
// count, then one f32 per parameter. A realistic plugin decodes its own chunk
// strictly and only then touches its parameters.
const CHUNK_MAGIC: &[u8; 4] = b"TSST";
const CHUNK_VERSION: u16 = 2;

fn encode_chunk(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + values.len() * 4);
    bytes.extend_from_slice(CHUNK_MAGIC);
    bytes.extend_from_slice(&CHUNK_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(values.len() as u16).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn decode_chunk(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 8 || &bytes[0..4] != CHUNK_MAGIC {
        return None;
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    if version != CHUNK_VERSION || count != PARAM_COUNT {
        return None;
    }
    if bytes.len() != 8 + count * 4 {
        return None;
    }
    let mut values = Vec::with_capacity(count);
    for (index, param) in PARAMS.iter().enumerate() {
        let start = 8 + index * 4;
        let raw = f32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
        if !raw.is_finite() {
            return None;
        }
        values.push(param.clamp(raw));
    }
    Some(values)
}

/// A minimal plugin whose state load path is the one under test: decode the
/// plugin document, then apply it through `restore_params`.
struct TestPlugin {
    values: [f32; PARAM_COUNT],
    set_calls: usize,
    index_calls: usize,
}

type Snapshot = ([f32; PARAM_COUNT], usize, usize);

impl TestPlugin {
    fn new() -> Self {
        Self {
            values: DEFAULT_VALUES,
            set_calls: 0,
            index_calls: 0,
        }
    }

    fn with_values(values: [f32; PARAM_COUNT]) -> Self {
        Self {
            values,
            set_calls: 0,
            index_calls: 0,
        }
    }

    fn values(&self) -> [f32; PARAM_COUNT] {
        self.values
    }

    fn snapshot(&self) -> Snapshot {
        (self.values, self.set_calls, self.index_calls)
    }
}

impl Plugin for TestPlugin {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.set_calls += 1;
        let Some(index) = PARAMS.iter().position(|param| param.id == id) else {
            return false;
        };
        self.values[index] = PARAMS[index].clamp(value);
        true
    }

    fn set_param_index(&mut self, index: usize, value: f32) -> bool {
        self.index_calls += 1;
        let Some(param) = PARAMS.get(index) else {
            return false;
        };
        self.values[index] = param.clamp(value);
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        PARAMS
            .iter()
            .position(|param| param.id == id)
            .map(|index| self.values[index])
    }

    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}

    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(encode_chunk(&self.values))
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        match decode_chunk(data) {
            Some(values) => restore_params(self, &values),
            None => false,
        }
    }
}

/// A structurally valid OPST document, with a caller-chosen header.
fn plane_bytes(magic: &[u8; 4], version: u16, count: u16, values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + values.len() * 4);
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// The first ten bytes of an OPCH envelope.
fn session_header(plane_len: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(10);
    bytes.extend_from_slice(SESSION_MAGIC);
    bytes.extend_from_slice(&SESSION_VERSION.to_le_bytes());
    bytes.extend_from_slice(&plane_len.to_le_bytes());
    bytes
}

/// A well-formed OPCH envelope around `plane` and `chunk`, built by hand so the
/// test can then break one field at a time.
fn session_bytes(plane: &[u8], chunk: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(10 + plane.len() + 4 + chunk.len());
    bytes.extend_from_slice(SESSION_MAGIC);
    bytes.extend_from_slice(&SESSION_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(plane.len() as u32).to_le_bytes());
    bytes.extend_from_slice(plane);
    bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(chunk);
    bytes
}

fn random_bytes(rng: &mut Lcg, len: usize) -> Vec<u8> {
    (0..len).map(|_| rng.next_u32() as u8).collect()
}

/// One random edit against a copy of `base`: bit flip, byte replace, insert,
/// delete or truncate. Insert and delete change the length; flips and replaces
/// keep it, which is what lets some mutations still decode.
fn mutate(rng: &mut Lcg, base: &[u8]) -> Vec<u8> {
    let mut bytes = base.to_vec();
    match rng.below(5) {
        0 if !bytes.is_empty() => {
            let index = rng.below(bytes.len());
            bytes[index] ^= 1 << rng.below(8);
        }
        1 if !bytes.is_empty() => {
            let index = rng.below(bytes.len());
            bytes[index] = rng.next_u32() as u8;
        }
        2 => {
            let index = rng.below(bytes.len() + 1);
            bytes.insert(index, rng.next_u32() as u8);
        }
        3 if !bytes.is_empty() => {
            let index = rng.below(bytes.len());
            bytes.remove(index);
        }
        _ => {
            let len = rng.below(bytes.len() + 1);
            bytes.truncate(len);
        }
    }
    bytes
}

/// Panic unless every decoded value is finite, inside its descriptor range and
/// already clamped, and the vector has exactly one value per parameter.
fn assert_decoded_values(values: &[f32], context: &str) {
    assert_eq!(
        values.len(),
        PARAM_COUNT,
        "{context}: decoded length must match the descriptor"
    );
    for (index, value) in values.iter().enumerate() {
        let param = &PARAMS[index];
        assert!(
            value.is_finite(),
            "{context}: {} decoded the non-finite {value}",
            param.id
        );
        assert!(
            (param.min..=param.max).contains(value),
            "{context}: {} decoded {value}, outside {}..={}",
            param.id,
            param.min,
            param.max
        );
        assert_eq!(
            *value,
            param.clamp(*value),
            "{context}: {} decoded an unclamped {value}",
            param.id
        );
    }
}

/// A failed document must not change a value or call a setter.
fn assert_untouched(plugin: &TestPlugin, before: Snapshot, context: &str) {
    assert_eq!(
        plugin.snapshot(),
        before,
        "{context}: a rejected document changed the live plugin"
    );
    assert_eq!(
        plugin.set_calls + plugin.index_calls,
        0,
        "{context}: a rejected document called a parameter setter"
    );
}

/// Headers and short inputs that must all be rejected by both decoders.
///
/// The u32 length fields are the largest representable declarations. On a
/// 64-bit target they fit `usize`; on a 32-bit target the same inputs exercise
/// the `checked_add` overflow path in `decode_session`.
fn hostile_documents() -> Vec<(Vec<u8>, &'static str)> {
    let mut rng = Lcg::new(SEED ^ 0x0000_0001);
    let mut cases = Vec::new();
    let values = valid_values();
    let valid_plane = plane_bytes(
        SNAPSHOT_MAGIC,
        SNAPSHOT_VERSION,
        PARAM_COUNT as u16,
        &values,
    );

    // Empty and 1..=8 byte inputs: too short for either magic plus header.
    for len in 0..=8 {
        cases.push((random_bytes(&mut rng, len), "short input"));
    }

    // Wrong magic with the rest of a valid plane after it.
    for magic in [b"OPCX", b"opst", b"\0\0\0\0", b"TSST"] {
        let mut bytes = valid_plane.clone();
        bytes[0..4].copy_from_slice(magic);
        cases.push((bytes, "wrong OPST magic"));
    }

    // Wrong OPST version.
    for version in [0, SNAPSHOT_VERSION + 1, u16::MAX] {
        cases.push((
            plane_bytes(SNAPSHOT_MAGIC, version, PARAM_COUNT as u16, &values),
            "wrong OPST version",
        ));
    }

    // Wrong parameter count, including the largest count a u16 can declare.
    for count in [0u16, 3, 5] {
        let short = vec![0.0; count as usize];
        cases.push((
            plane_bytes(SNAPSHOT_MAGIC, SNAPSHOT_VERSION, count, &short),
            "wrong OPST parameter count",
        ));
    }
    cases.push((
        plane_bytes(SNAPSHOT_MAGIC, SNAPSHOT_VERSION, u16::MAX, &[]),
        "declared parameter count",
    ));

    // Truncated value arrays: one byte, one value and half a value short.
    for cut in [1, 4, 8] {
        let bytes = valid_plane[..valid_plane.len() - cut].to_vec();
        cases.push((bytes, "truncated value array"));
    }

    // Extra bytes after an otherwise valid plane.
    let mut extra_one = valid_plane.clone();
    extra_one.push(0);
    cases.push((extra_one, "extra trailing byte"));
    let mut extra_four = valid_plane.clone();
    extra_four.extend_from_slice(&[0, 1, 2, 3]);
    cases.push((extra_four, "extra trailing bytes"));

    // A valid session to corrupt field by field.
    let chunk = encode_chunk(&values);
    let valid_session = session_bytes(&valid_plane, &chunk);

    // OPCH prefix cut before the ten-byte header is complete.
    for len in 4..10 {
        cases.push((valid_session[..len].to_vec(), "truncated OPCH header"));
    }

    // Wrong OPCH version.
    for version in [0, SESSION_VERSION + 1, u16::MAX] {
        let mut bytes = valid_session.clone();
        bytes[4..6].copy_from_slice(&version.to_le_bytes());
        cases.push((bytes, "wrong OPCH version"));
    }

    // Header-only OPCH: the declared plane length has no plane bytes behind it.
    let plane_lengths = [
        0u32,
        1,
        7,
        8,
        0x8000_0000,
        u32::MAX - 13,
        u32::MAX - 9,
        u32::MAX,
    ];
    for plane_len in plane_lengths {
        cases.push((session_header(plane_len), "OPCH header only"));
    }

    // Valid plane, then a chunk length with no chunk bytes behind it.
    for chunk_len in [1u32, 3, 4, 8, 0x8000_0000, u32::MAX] {
        let mut bytes = session_header(valid_plane.len() as u32);
        bytes.extend_from_slice(&valid_plane);
        bytes.extend_from_slice(&chunk_len.to_le_bytes());
        cases.push((bytes, "OPCH missing chunk"));
    }

    // Valid plane but no chunk length at all.
    let mut no_chunk_length = session_header(valid_plane.len() as u32);
    no_chunk_length.extend_from_slice(&valid_plane);
    cases.push((no_chunk_length, "OPCH without chunk length"));

    // Declared plane length off by one in each direction.
    for delta in [-1i32, 1] {
        let mut bytes = valid_session.clone();
        let declared = (valid_plane.len() as i32 + delta) as u32;
        bytes[6..10].copy_from_slice(&declared.to_le_bytes());
        cases.push((bytes, "OPCH plane length mismatch"));
    }

    // Chunk length disagreeing with the bytes present.
    for chunk_len in [1u32, 3, u32::MAX] {
        let mut bytes = valid_session.clone();
        let offset = 10 + valid_plane.len();
        bytes[offset..offset + 4].copy_from_slice(&chunk_len.to_le_bytes());
        cases.push((bytes, "OPCH chunk length mismatch"));
    }

    // Truncated and overlong session tails.
    let mut truncated_session = valid_session.clone();
    truncated_session.pop();
    cases.push((truncated_session, "truncated OPCH chunk"));
    let mut overlong_session = valid_session.clone();
    overlong_session.push(0);
    cases.push((overlong_session, "extra byte after OPCH chunk"));

    // A correct envelope around a plane carrying NaN.
    let mut nan_values = values;
    nan_values[1] = f32::NAN;
    let nan_plane = plane_bytes(
        SNAPSHOT_MAGIC,
        SNAPSHOT_VERSION,
        PARAM_COUNT as u16,
        &nan_values,
    );
    cases.push((session_bytes(&nan_plane, &chunk), "OPCH plane with NaN"));

    // A correct envelope around a plane with the wrong parameter count.
    let wrong_count_plane = plane_bytes(SNAPSHOT_MAGIC, SNAPSHOT_VERSION, 3, &[0.0; 3]);
    cases.push((
        session_bytes(&wrong_count_plane, &chunk),
        "OPCH plane with wrong count",
    ));

    // OPCH magic mutated after the first byte.
    let mut bad_magic = valid_session.clone();
    bad_magic[0] = b'X';
    cases.push((bad_magic, "wrong OPCH magic"));

    // A plane that is itself one byte short inside a correct envelope.
    cases.push((
        session_bytes(&valid_plane[..valid_plane.len() - 1], &chunk),
        "truncated OPCH plane",
    ));

    cases
}

#[test]
fn hostile_headers_are_rejected_without_unbounded_allocation() {
    // Prove the instrument is live before trusting its zero readings below.
    let (probe, probed) = allocated_bytes(|| vec![0u8; 4_096]);
    assert_eq!(probe.len(), 4_096);
    assert!(
        probed >= 4_096,
        "the allocation counter missed a 4096-byte allocation"
    );

    let cases = hostile_documents();
    assert!(
        cases.len() >= 50,
        "the hostile corpus shrank to {} inputs",
        cases.len()
    );

    let mut plugin = TestPlugin::new();
    let untouched = plugin.snapshot();

    for (bytes, context) in &cases {
        let (params, allocated) = allocated_bytes(|| decode_params(&INFO, bytes));
        assert!(
            params.is_none(),
            "{context}: decode_params accepted {} bytes",
            bytes.len()
        );
        assert!(
            allocated < ALLOCATION_LIMIT,
            "{context}: decode_params allocated {allocated} bytes from a {} byte input",
            bytes.len()
        );

        let (session, allocated) = allocated_bytes(|| decode_session(&INFO, bytes));
        assert!(
            session.is_none(),
            "{context}: decode_session accepted {} bytes",
            bytes.len()
        );
        assert!(
            allocated < ALLOCATION_LIMIT,
            "{context}: decode_session allocated {allocated} bytes from a {} byte input",
            bytes.len()
        );

        let before = plugin.snapshot();
        assert!(
            !plugin.load_state(bytes),
            "{context}: the plugin load path accepted {} bytes",
            bytes.len()
        );
        assert_untouched(&plugin, before, context);
    }

    assert_eq!(
        plugin.snapshot(),
        untouched,
        "hostile documents changed the long-lived plugin"
    );
}

#[test]
fn nan_inf_and_out_of_range_payloads_are_rejected_or_clamped() {
    // Non-finite payloads are rejected, never clamped to anything.
    let mut plugin = TestPlugin::new();
    let untouched = plugin.snapshot();
    let non_finite = [
        f32::NAN,
        -f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::from_bits(0x7F80_0001),
        f32::from_bits(0xFFC0_1234),
    ];
    let mut rejected = 0;
    for bad in non_finite {
        for index in 0..PARAM_COUNT {
            let mut raw = valid_values();
            raw[index] = bad;
            let blob = plane_bytes(SNAPSHOT_MAGIC, SNAPSHOT_VERSION, PARAM_COUNT as u16, &raw);
            assert!(
                decode_params(&INFO, &blob).is_none(),
                "decode_params accepted {} at index {index}",
                bad
            );
            assert!(
                decode_session(&INFO, &blob).is_none(),
                "decode_session accepted {} at index {index}",
                bad
            );
            let chunk = encode_chunk(&raw);
            assert!(
                !plugin.load_state(&chunk),
                "the plugin load path accepted {} at index {index}",
                bad
            );
            assert_untouched(&plugin, untouched, "non-finite payload");
            rejected += 1;
        }
    }
    assert_eq!(rejected, non_finite.len() * PARAM_COUNT);

    // Encoding sanitises hostile values instead of writing them.
    let encoded = encode_params(
        &INFO,
        &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX],
    );
    let decoded = decode_params(&INFO, &encoded).expect("encoded hostiles decode");
    assert_eq!(
        decoded,
        vec![
            PARAMS[0].default,
            PARAMS[1].max,
            PARAMS[2].min,
            PARAMS[3].max
        ]
    );

    // Finite extremes and values far outside every range decode clamped.
    let extremes = [
        f32::MAX,
        f32::MIN,
        f32::from_bits(1),
        f32::from_bits(0x007F_FFFF),
        f32::from_bits(0x8000_0001),
        1e30,
        -1e30,
        123_456.0,
        -123_456.0,
        f32::EPSILON,
    ];
    let mut accepted = 0;
    for raw_value in extremes {
        for index in 0..PARAM_COUNT {
            let mut raw = valid_values();
            raw[index] = raw_value;
            let blob = plane_bytes(SNAPSHOT_MAGIC, SNAPSHOT_VERSION, PARAM_COUNT as u16, &raw);
            let expected = clamped(&raw);

            let decoded = decode_params(&INFO, &blob).expect("finite extreme must decode");
            assert_decoded_values(&decoded, "extreme plane");
            assert_eq!(decoded, expected.to_vec());

            let session = decode_session(&INFO, &blob).expect("finite extreme session must decode");
            assert_eq!(session.values, expected.to_vec());

            let mut plugin = TestPlugin::new();
            let chunk = encode_chunk(&raw);
            assert!(plugin.load_state(&chunk));
            assert_eq!(&plugin.values()[..], &expected[..]);
            assert_eq!(plugin.set_calls, 0, "load applies through the index path");
            assert_eq!(plugin.index_calls, PARAM_COUNT);
            accepted += 1;
        }
    }
    assert_eq!(accepted, extremes.len() * PARAM_COUNT);

    // The plugin's own chunk decodes the same extremes clamped.
    let chunk = encode_chunk(&[f32::MAX, f32::MIN, f32::from_bits(1), 1e30]);
    let mut plugin = TestPlugin::new();
    assert!(plugin.load_state(&chunk));
    assert_decoded_values(&plugin.values(), "extreme chunk");
    assert_eq!(
        plugin.values(),
        clamped(&[f32::MAX, f32::MIN, f32::from_bits(1), 1e30])
    );
}

#[test]
fn byte_mutations_never_panic_and_keep_values_valid() {
    let base_plane = encode_params(&INFO, &valid_values());
    let base_chunk = encode_chunk(&valid_values());
    let base_session = encode_session(&INFO, &valid_values(), &base_chunk);
    let bases: [&[u8]; 3] = [&base_plane, &base_chunk, &base_session];

    let mut rng = Lcg::new(SEED ^ 0x00C0_FFEE);
    let mut decoded_planes = 0usize;
    let mut decoded_sessions = 0usize;
    let mut loaded = 0usize;
    let mut rejected_loads = 0usize;

    for iteration in 0..MUTATION_ITERATIONS {
        let base = bases[rng.below(bases.len())];
        let mutated = mutate(&mut rng, base);

        if let Some(values) = decode_params(&INFO, &mutated) {
            assert_decoded_values(&values, &format!("plane mutation {iteration}"));
            decoded_planes += 1;
        }

        if let Some(session) = decode_session(&INFO, &mutated) {
            assert_decoded_values(&session.values, &format!("session mutation {iteration}"));
            assert!(
                session.chunk.len() <= mutated.len(),
                "session mutation {iteration}: chunk grew past its document"
            );
            decoded_sessions += 1;
        }

        let mut plugin = TestPlugin::new();
        let before = plugin.snapshot();
        if plugin.load_state(&mutated) {
            assert_decoded_values(&plugin.values(), &format!("load mutation {iteration}"));
            assert_eq!(
                plugin.index_calls, PARAM_COUNT,
                "load mutation {iteration}: a successful load applies every parameter"
            );
            assert_eq!(
                plugin.set_calls, 0,
                "load mutation {iteration}: the index path applies a document"
            );
            loaded += 1;
        } else {
            assert_untouched(&plugin, before, &format!("load mutation {iteration}"));
            rejected_loads += 1;
        }
    }

    assert!(
        decoded_planes > 0,
        "the corpus never produced a decodable plane mutation"
    );
    assert!(
        decoded_sessions > 0,
        "the corpus never produced a decodable session mutation"
    );
    assert!(
        loaded > 0,
        "the corpus never produced a loadable document mutation"
    );
    assert!(rejected_loads > 0, "the corpus never rejected a mutation");
}

#[test]
fn restore_params_is_all_or_nothing() {
    let mut plugin = TestPlugin::new();
    let before = plugin.snapshot();

    // Wrong length, short and long.
    assert!(!restore_params(&mut plugin, &[]));
    assert_untouched(&plugin, before, "empty restore");
    assert!(!restore_params(&mut plugin, &[0.0; PARAM_COUNT - 1]));
    assert_untouched(&plugin, before, "short restore");
    assert!(!restore_params(&mut plugin, &[0.0; PARAM_COUNT + 1]));
    assert_untouched(&plugin, before, "long restore");

    // One NaN, one infinity, one negative infinity in an otherwise valid vector.
    let mut bad = valid_values();
    bad[2] = f32::NAN;
    assert!(!restore_params(&mut plugin, &bad));
    assert_untouched(&plugin, before, "NaN restore");
    let mut bad = valid_values();
    bad[0] = f32::INFINITY;
    assert!(!restore_params(&mut plugin, &bad));
    assert_untouched(&plugin, before, "infinity restore");
    let mut bad = valid_values();
    bad[3] = f32::NEG_INFINITY;
    assert!(!restore_params(&mut plugin, &bad));
    assert_untouched(&plugin, before, "negative infinity restore");
    assert_eq!(
        plugin.set_calls + plugin.index_calls,
        0,
        "a rejected vector must not call any setter"
    );

    // A valid vector applies every value through the index path, clamped.
    let out_of_range = [-1_000.0, 1_000_000.0, f32::MAX, -1.0];
    let expected = clamped(&out_of_range);
    assert!(restore_params(&mut plugin, &out_of_range));
    assert_eq!(plugin.values(), expected);
    assert_eq!(plugin.index_calls, PARAM_COUNT);
    assert_eq!(plugin.set_calls, 0);

    // Applying an already clamped vector again changes nothing.
    let current = plugin.values();
    assert!(restore_params(&mut plugin, &current));
    assert_eq!(plugin.values(), expected);
}

#[test]
fn plugin_load_state_rejects_random_and_corrupt_chunks() {
    let mut rng = Lcg::new(SEED ^ 0x00F0_0D00);
    let mut random_accepted = 0usize;

    for _ in 0..RANDOM_LOAD_ITERATIONS {
        let len = rng.below(65);
        let junk = random_bytes(&mut rng, len);
        // The free decoders must not panic on the same junk either; a rare
        // accidental header is still held to the value contract.
        if let Some(values) = decode_params(&INFO, &junk) {
            assert_decoded_values(&values, "random plane");
        }
        if let Some(session) = decode_session(&INFO, &junk) {
            assert_decoded_values(&session.values, "random session");
        }

        let mut plugin = TestPlugin::new();
        let before = plugin.snapshot();
        if plugin.load_state(&junk) {
            assert_decoded_values(&plugin.values(), "random load");
            random_accepted += 1;
        } else {
            assert_untouched(&plugin, before, "random load");
        }
    }
    assert_eq!(
        random_accepted, 0,
        "random buffers must not decode as a plugin document"
    );

    // Targeted corruptions of the plugin's own chunk.
    let valid_chunk = encode_chunk(&valid_values());
    let mut corrupt = Vec::new();

    corrupt.push(("empty chunk", Vec::new()));
    let mut wrong_magic = valid_chunk.clone();
    wrong_magic[0] ^= 0xFF;
    corrupt.push(("wrong chunk magic", wrong_magic));
    let mut wrong_version = valid_chunk.clone();
    wrong_version[4..6].copy_from_slice(&(CHUNK_VERSION + 1).to_le_bytes());
    corrupt.push(("wrong chunk version", wrong_version));
    let mut wrong_count = valid_chunk.clone();
    wrong_count[6..8].copy_from_slice(&3u16.to_le_bytes());
    corrupt.push(("wrong chunk count", wrong_count));
    corrupt.push((
        "truncated chunk",
        valid_chunk[..valid_chunk.len() - 1].to_vec(),
    ));
    let mut extra = valid_chunk.clone();
    extra.push(0);
    corrupt.push(("extra chunk byte", extra));
    corrupt.push(("NaN in chunk", encode_chunk(&[f32::NAN, 440.0, 25.0, 1.0])));
    corrupt.push((
        "infinity in chunk",
        encode_chunk(&[f32::INFINITY, 440.0, 25.0, 1.0]),
    ));

    let mut plugin = TestPlugin::new();
    let before = plugin.snapshot();
    for (context, bytes) in &corrupt {
        assert!(
            !plugin.load_state(bytes),
            "{context}: accepted by load_state"
        );
        assert_untouched(&plugin, before, context);
    }

    // The untouched valid document still restores exactly afterwards.
    assert!(plugin.load_state(&valid_chunk));
    assert_eq!(plugin.values(), valid_values());
    assert_eq!(plugin.index_calls, PARAM_COUNT);
}

#[test]
fn corrupt_session_envelopes_do_not_touch_the_plugin() {
    let values = valid_values();
    let chunk = encode_chunk(&values);
    let good = encode_session(&INFO, &values, &chunk);

    // A corrupt plane means the host never reaches the plugin at all.
    let mut bad_plane = good.clone();
    bad_plane[18..22].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(decode_session(&INFO, &bad_plane).is_none());

    // A corrupt plugin chunk still decodes as a session envelope; the chunk is
    // then rejected and the plane must not be applied either.
    let mut bad_chunk = good.clone();
    bad_chunk[38] ^= 0xFF;
    let session = decode_session(&INFO, &bad_chunk).expect("the envelope still decodes");
    assert_decoded_values(&session.values, "corrupt chunk session");
    let mut plugin = TestPlugin::new();
    let before = plugin.snapshot();
    assert!(!plugin.load_state(&session.chunk));
    assert_untouched(&plugin, before, "corrupt session chunk");

    // A truncated chunk from that envelope behaves the same way.
    let truncated = session.chunk[..session.chunk.len() - 1].to_vec();
    assert!(!plugin.load_state(&truncated));
    assert_untouched(&plugin, before, "truncated session chunk");

    // The valid session restores its chunk exactly.
    let session = decode_session(&INFO, &good).expect("valid session decodes");
    assert!(plugin.load_state(&session.chunk));
    assert_eq!(plugin.values(), values);
}

#[test]
fn valid_state_round_trips_exactly() {
    let values = valid_values();
    let plane = encode_params(&INFO, &values);
    assert_eq!(plane.len(), 8 + PARAM_COUNT * 4);
    assert_eq!(decode_params(&INFO, &plane), Some(values.to_vec()));

    // An opaque chunk survives the OPCH envelope byte for byte.
    let opaque = b"opaque plugin settings".to_vec();
    let session_bytes = encode_session(&INFO, &values, &opaque);
    assert_eq!(session_bytes.len(), 10 + plane.len() + 4 + opaque.len());
    let session = decode_session(&INFO, &session_bytes).expect("session decodes");
    assert_eq!(session.values, values.to_vec());
    assert_eq!(session.chunk, opaque);

    // A bare OPST plane still parses as a session with an empty chunk.
    let bare = decode_session(&INFO, &plane).expect("bare plane decodes");
    assert_eq!(bare.values, values.to_vec());
    assert!(bare.chunk.is_empty());

    // An OPCH envelope with an empty chunk is valid too.
    let empty = decode_session(&INFO, &encode_session(&INFO, &values, &[]))
        .expect("empty chunk session decodes");
    assert_eq!(empty.values, values.to_vec());
    assert!(empty.chunk.is_empty());

    // save_state into a fresh plugin restores every value exactly.
    let mut source = TestPlugin::with_values(values);
    let saved = source.save_state().expect("plugin saves state");
    assert_eq!(saved.len(), 8 + PARAM_COUNT * 4);
    let mut target = TestPlugin::new();
    assert_ne!(target.snapshot(), source.snapshot());
    assert!(target.load_state(&saved));
    assert_eq!(target.values(), values);
    assert_eq!(target.index_calls, PARAM_COUNT);
    assert_eq!(target.set_calls, 0);

    // The plugin chunk inside an OPCH envelope loads through the same path.
    let session = decode_session(&INFO, &encode_session(&INFO, &values, &saved))
        .expect("plugin chunk session decodes");
    let mut plugin = TestPlugin::new();
    assert!(plugin.load_state(&session.chunk));
    assert_eq!(plugin.values(), values);
}
