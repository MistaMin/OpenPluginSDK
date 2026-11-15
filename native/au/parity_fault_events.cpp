// SPDX-License-Identifier: MIT
// Negative control for the format parity test, Audio Unit shell. TEST BUILDS ONLY: compiled
// only into the OpenPluginAUFault target of scripts/format_parity/parity.cmake.
//
// The Audio Unit shell renders through op_shell_process_events, so that target compiles the
// unmodified shell source with -Dop_shell_process_events=parity_faulty_process_events. This
// runs the real engine, then scales every output channel by (1 + PARITY_FAULT_SCALE), the same
// gain error scripts/format_parity/native/parity_fault.cpp gives the other shells.
#undef op_shell_process_events
#include "op_shell.h"

#ifndef PARITY_FAULT_SCALE
#define PARITY_FAULT_SCALE 1e-4f
#endif

extern "C" uint32_t parity_faulty_process_events(void* instance, const OpProcess* args) {
    const uint32_t ok = op_shell_process_events(instance, args);
    const float k = 1.0f + PARITY_FAULT_SCALE;
    for (uint32_t ch = 0; ok && ch < args->output_channels; ++ch) {
        for (uint32_t i = 0; i < args->frames; ++i) args->outputs[ch][i] *= k;
    }
    return ok;
}
