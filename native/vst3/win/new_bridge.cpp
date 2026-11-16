// SPDX-License-Identifier: MIT
// For the VST3 events proof (native/vst3/events_host.cpp) on Windows.
//
// The proof counts allocations with an operator new of its own. On macOS that one
// operator new also serves the module loaded into the process, so the count covers
// the shell. On Windows every module links its own operator new, and the proof's
// would count nothing the shell does: "0 allocations in process()" would pass
// without looking, and the proof's self-check (the shell allocates in getState)
// would fail.
//
// This file closes the gap. native/win/alloc_hook.cpp reports every heap allocation in the
// process, whichever module asked, to malloc_logger; the logger here answers each
// one with a call of the proof's operator new, which does the counting. So the
// proof's count covers every allocation the shell and the plugin make, by any
// route, which is at least what it covers on macOS.
#include <cstddef>
#include <cstdint>
#include <new>

extern "C" {
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t num_hot_frames_to_skip);
extern malloc_logger_t* malloc_logger;
}

namespace {
constexpr uint32_t kLogAlloc = 2;
// The proof's operator new allocates too; that allocation is not reported again.
thread_local bool tInside = false;

void report(uint32_t type, uintptr_t, uintptr_t, uintptr_t, uintptr_t, uint32_t) {
    if (!(type & kLogAlloc) || tInside) return;
    tInside = true;
    ::operator delete(::operator new(1));
    tInside = false;
}

struct Install {
    Install() { malloc_logger = report; }
} gInstall;
}
