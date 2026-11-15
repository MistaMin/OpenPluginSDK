// SPDX-License-Identifier: MIT
// Negative control for the format parity test, for shells that process through
// op_shell_process_events (the VST3 shell). TEST BUILDS ONLY: compiled only into
// the VST3 *Fault target of scripts/format_parity/parity.cmake.
//
// That target compiles the unmodified shell sources with
//   -Dop_shell_process_events=parity_faulty_process_events
// so the shell calls this function instead of the engine. It runs the real
// engine, then scales the main output by (1 + PARITY_FAULT_SCALE), like
// parity_fault.cpp does for op_shell_process.
#undef op_shell_process_events
#include "op_shell.h"

#ifndef PARITY_FAULT_SCALE
#define PARITY_FAULT_SCALE 1e-4f
#endif

extern "C" uint32_t parity_faulty_process_events(void* instance, const OpProcess* args) {
    const uint32_t ok = op_shell_process_events(instance, args);
    if (!ok || !args || !args->outputs) return ok;
    const float k = 1.0f + PARITY_FAULT_SCALE;
    for (uint32_t c = 0; c < args->output_channels; ++c) {
        if (!args->outputs[c]) continue;
        for (uint32_t i = 0; i < args->frames; ++i) args->outputs[c][i] *= k;
    }
    return ok;
}
