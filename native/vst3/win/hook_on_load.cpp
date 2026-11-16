// SPDX-License-Identifier: MIT
// For the VST3 proofs that count allocations (native/win/alloc_hook.cpp): a VST3
// proof loads its plug-in module after main has started, so the module's heap imports
// are pointed at the counting functions when the load is done. cf_compat.cpp calls
// op_win_module_loaded after each module a proof loads.
#include "CoreFoundation/CoreFoundation.h"

extern "C" void op_win_alloc_hook_patch(void);

namespace {
struct Install {
    Install() {
        op_win_module_loaded = [](void*) { op_win_alloc_hook_patch(); };
    }
} gInstall;
}
