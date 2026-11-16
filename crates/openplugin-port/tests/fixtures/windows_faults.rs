// A CLAP library that misbehaves inside `clap_entry.init`, built with rustc (`--crate-type cdylib`) by
// `tests/windows_probe.rs`. OPENPLUGIN_PORT_FIXTURE_FAULT picks the fault, raised in the plug-in's own
// code after the scanner has loaded it, the way a real plug-in fails:
//
//   access    write through a null pointer (EXCEPTION_ACCESS_VIOLATION, 0xC0000005)
//   illegal   execute ud2 (EXCEPTION_ILLEGAL_INSTRUCTION, 0xC000001D)
//   stack     unbounded recursion (EXCEPTION_STACK_OVERFLOW, 0xC00000FD)
//   panic     a Rust panic that cannot unwind out of `extern "C"`: abort (fast fail, 0xC0000409)
//   hang      sleep for ever
//   anything else, or unset: init returns false
#![allow(non_camel_case_types, unconditional_recursion, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ver {
    major: u32,
    minor: u32,
    revision: u32,
}

#[repr(C)]
pub struct Entry {
    v: Ver,
    init: unsafe extern "C" fn(*const c_char) -> bool,
    deinit: unsafe extern "C" fn(),
    get_factory: unsafe extern "C" fn(*const c_char) -> *const c_void,
}
unsafe impl Sync for Entry {}

#[inline(never)]
fn recurse(depth: u64) -> u64 {
    let pad = std::hint::black_box([depth; 512]);
    pad[(depth % 512) as usize] + recurse(std::hint::black_box(depth + 1))
}

unsafe extern "C" fn e_init(_path: *const c_char) -> bool {
    match std::env::var("OPENPLUGIN_PORT_FIXTURE_FAULT").as_deref() {
        Ok("access") => {
            let p: *mut u32 = std::hint::black_box(std::ptr::null_mut());
            unsafe { p.write_volatile(1) };
            true
        }
        Ok("illegal") => {
            unsafe { std::arch::asm!("ud2") };
            true
        }
        Ok("stack") => recurse(0) == 7,
        Ok("panic") => panic!("the fixture panics inside clap_entry.init"),
        Ok("hang") => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        _ => false,
    }
}
unsafe extern "C" fn e_deinit() {}
unsafe extern "C" fn e_factory(_id: *const c_char) -> *const c_void {
    std::ptr::null()
}

#[unsafe(no_mangle)]
pub static clap_entry: Entry = Entry { v: Ver { major: 1, minor: 2, revision: 0 }, init: e_init, deinit: e_deinit, get_factory: e_factory };
