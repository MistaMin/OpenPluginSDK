//! `op_shell_save_state` / `op_shell_load_state` on one thread: the round
//! trip of a plugin with its own chunk, the size protocol, and refusal of
//! hostile blobs. A refused blob changes nothing: not the parameters, not the
//! plugin's chunk. Deterministic (fixed seed).

#[path = "common/state_plugin.rs"]
mod common;

use common::*;
use openplugin_core::encode_params;
use openplugin_ffi::shell::{self, STATE_OK, STATE_TOO_SMALL};

fn make() -> *mut shell::ShellInstance {
    let instance = shell::create::<Chunky>(48_000.0, 256, 2);
    assert!(!instance.is_null());
    instance
}

fn render(instance: *mut shell::ShellInstance) -> f32 {
    let input = [1.0f32; 64];
    let mut out = [0.0f32; 64];
    let ran = unsafe {
        shell::process(instance, input.as_ptr(), input.as_ptr(), out.as_mut_ptr(), out.as_mut_ptr(), 64)
    };
    assert_eq!(ran, 1);
    out[63]
}

#[test]
fn a_plugin_with_its_own_chunk_round_trips_through_a_fresh_instance() {
    let a = make();
    unsafe {
        assert_eq!(shell::set_plain(a, 0, 6.0), 1);
        assert_eq!(shell::set_plain(a, 1, 0.25), 1);
    }
    // Load a chunk so the plugin holds a program the plane does not know.
    let blob = session(&[6.0, 0.25], &chunk(5, b"lead"));
    assert!(load(a, &blob));
    let saved = save(a);
    assert!(saved.starts_with(b"OPCH"));
    let decoded = openplugin_core::decode_session(&INFO, &saved).unwrap();
    assert_eq!(decoded.values, vec![6.0, 0.25]);
    assert_eq!(decoded.chunk, chunk(5, b"lead"));

    let b = make();
    assert_eq!(render(b), 1.0, "a fresh instance runs at program 1, 0 dB");
    assert!(load(b, &saved));
    unsafe {
        assert!((shell::get_plain(b, 0) - 6.0).abs() < 1e-6);
        assert!((shell::get_plain(b, 1) - 0.25).abs() < 1e-6);
    }
    let expected = 5.0 * 10f32.powf(6.0 / 20.0);
    assert!((render(b) - expected).abs() < 1e-3, "program 5 at +6 dB");
    assert!((render(a) - expected).abs() < 1e-3);
    assert_eq!(save(b), saved, "restoring and saving again is the identity");
    unsafe {
        shell::destroy(a);
        shell::destroy(b);
    }
}

#[test]
fn the_chunk_sees_the_plane_values_not_yet_applied() {
    // A parameter written just before the save must be in the saved plane.
    let a = make();
    unsafe { assert_eq!(shell::set_plain(a, 0, -12.0), 1) };
    let saved = save(a);
    let decoded = openplugin_core::decode_session(&INFO, &saved).unwrap();
    assert_eq!(decoded.values[0], -12.0);
    unsafe { shell::destroy(a) };
}

#[test]
fn the_size_protocol_is_exact() {
    let a = make();
    let mut needed = 0u32;
    let code = unsafe { shell::save_state(a, std::ptr::null_mut(), 0, &mut needed) };
    assert_eq!(code, STATE_TOO_SMALL);
    assert!(needed > 0);
    // A short buffer: nothing written, size reported.
    let mut small = vec![0xAAu8; needed as usize - 1];
    let mut again = 0u32;
    let code = unsafe { shell::save_state(a, small.as_mut_ptr(), small.len() as u32, &mut again) };
    assert_eq!(code, STATE_TOO_SMALL);
    assert_eq!(again, needed);
    assert!(small.iter().all(|byte| *byte == 0xAA), "a short buffer is not written");
    // An exact buffer, and one with room to spare (only `needed` bytes written).
    let mut exact = vec![0u8; needed as usize];
    assert_eq!(unsafe { shell::save_state(a, exact.as_mut_ptr(), needed, &mut again) }, STATE_OK);
    assert_eq!(again, needed);
    let mut roomy = vec![0xEEu8; needed as usize + 8];
    assert_eq!(unsafe { shell::save_state(a, roomy.as_mut_ptr(), roomy.len() as u32, &mut again) }, STATE_OK);
    assert_eq!(&roomy[..needed as usize], &exact[..]);
    assert!(roomy[needed as usize..].iter().all(|byte| *byte == 0xEE));
    // `needed` may be null.
    assert_eq!(
        unsafe { shell::save_state(a, exact.as_mut_ptr(), needed, std::ptr::null_mut()) },
        STATE_OK
    );
    // Misuse fails cleanly.
    assert_eq!(unsafe { shell::save_state(std::ptr::null_mut(), exact.as_mut_ptr(), needed, &mut again) }, 0);
    assert_eq!(unsafe { shell::save_state(a, std::ptr::null_mut(), 16, &mut again) }, 0);
    unsafe { shell::destroy(a) };
}

#[test]
fn null_and_empty_loads_are_refused() {
    let a = make();
    let before = save(a);
    assert_eq!(unsafe { shell::load_state(std::ptr::null_mut(), before.as_ptr(), before.len() as u32) }, 0);
    assert_eq!(unsafe { shell::load_state(a, std::ptr::null(), 10) }, 0);
    assert_eq!(unsafe { shell::load_state(a, before.as_ptr(), 0) }, 0);
    assert_eq!(save(a), before);
    unsafe { shell::destroy(a) };
}

#[test]
fn a_bare_parameter_plane_still_loads_and_keeps_the_chunk() {
    let a = make();
    assert!(load(a, &session(&[0.0, 0.5], &chunk(4, b"keep"))));
    let opst = encode_params(&INFO, &[9.0, 0.75]);
    assert!(opst.starts_with(b"OPST"));
    assert!(load(a, &opst));
    unsafe {
        assert!((shell::get_plain(a, 0) - 9.0).abs() < 1e-6);
        assert!((shell::get_plain(a, 1) - 0.75).abs() < 1e-6);
    }
    let decoded = openplugin_core::decode_session(&INFO, &save(a)).unwrap();
    assert_eq!(decoded.chunk, chunk(4, b"keep"), "an OPST plane does not touch the plugin's chunk");
    unsafe { shell::destroy(a) };
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// A blob the loader must refuse, and nothing it does may show afterwards.
fn assert_refused(instance: *mut shell::ShellInstance, blob: &[u8], what: &str) {
    let before = save(instance);
    let rendered = render(instance);
    assert!(!load(instance, blob), "{what}: the blob was accepted");
    assert_eq!(save(instance), before, "{what}: a refused blob changed the state");
    assert_eq!(render(instance), rendered, "{what}: a refused blob changed the audio");
}

#[test]
fn hostile_blobs_are_refused_and_change_nothing() {
    let a = make();
    // A live state that differs from every blob below.
    assert!(load(a, &session(&[-3.0, 0.9], &chunk(6, b"live"))));
    let good = session(&[7.0, 0.1], &chunk(2, b"other"));

    // Every truncation, and every extension.
    for cut in 0..good.len() {
        assert_refused(a, &good[..cut], &format!("truncated to {cut}"));
    }
    let mut longer = good.clone();
    longer.push(0);
    assert_refused(a, &longer, "one trailing byte");

    // Wrong magic, unknown version.
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_refused(a, &bad, "magic");
    let mut bad = good.clone();
    bad[4] = 9;
    assert_refused(a, &bad, "version");

    // Lengths that lie: plane length and chunk length, including huge ones.
    for value in [0u32, 1, 7, 12, 0x7FFF_FFFF, u32::MAX] {
        let mut bad = good.clone();
        bad[6..10].copy_from_slice(&value.to_le_bytes());
        assert_refused(a, &bad, &format!("plane length {value}"));
    }
    let chunk_at = good.len() - chunk(2, b"other").len() - 4;
    for value in [0u32, 1, 100, 0x7FFF_FFFF, u32::MAX] {
        let mut bad = good.clone();
        bad[chunk_at..chunk_at + 4].copy_from_slice(&value.to_le_bytes());
        assert_refused(a, &bad, &format!("chunk length {value}"));
    }

    // Non-finite parameter values (the plane starts at byte 18 in an OPCH blob).
    for nan in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bad = good.clone();
        bad[18..22].copy_from_slice(&nan.to_le_bytes());
        assert_refused(a, &bad, &format!("value {nan}"));
    }

    // A foreign plane: wrong parameter count, as OPST and inside OPCH.
    let mut foreign = b"OPST".to_vec();
    foreign.extend_from_slice(&1u16.to_le_bytes());
    foreign.extend_from_slice(&3u16.to_le_bytes());
    foreign.extend_from_slice(&[0u8; 12]);
    assert_refused(a, &foreign, "OPST with three parameters");

    // A valid envelope and plane around a chunk the plugin refuses. The plane
    // differs from the live one, so a half-applied load would show.
    for bad_chunk in [
        Vec::from(&b"JUNK"[..]),
        chunk(0, b"x"),
        chunk(PROGRAM_MAX + 1, b"x"),
        chunk(3, b"x")[..6].to_vec(),
        b"PRG1".to_vec(),
    ] {
        assert_refused(a, &session(&[5.0, 0.2], &bad_chunk), "plugin-refused chunk");
    }

    // Garbage: empty, short, random, and the good blob with random byte flips
    // that the loader may only accept when the result is a consistent state.
    assert_refused(a, &[], "empty");
    assert_refused(a, b"OPCH", "bare magic");
    let mut rng = Lcg(0x5EED_57A7_E000_0001);
    for round in 0..400 {
        let len = (rng.next() % 96) as usize;
        let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        assert_refused(a, &junk, &format!("random junk {round}"));
    }
    for round in 0..600 {
        let mut mutated = good.clone();
        for _ in 0..1 + rng.next() % 3 {
            let at = (rng.next() as usize) % mutated.len();
            mutated[at] ^= 1 << (rng.next() % 8);
        }
        let before = save(a);
        if load(a, &mutated) {
            // A flip inside a value or the program can still be a state; it
            // must then be exactly the state the blob describes.
            let decoded = openplugin_core::decode_session(&INFO, &mutated).expect("accepted blobs decode");
            let now = openplugin_core::decode_session(&INFO, &save(a)).unwrap();
            assert_eq!(now.values, decoded.values, "mutation {round}");
            assert_eq!(now.chunk, decoded.chunk, "mutation {round}");
            assert!(load(a, &session(&[-3.0, 0.9], &chunk(6, b"live"))));
        } else {
            assert_eq!(save(a), before, "mutation {round}: a refused blob changed the state");
        }
    }
    unsafe { shell::destroy(a) };
}
