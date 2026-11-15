// SPDX-License-Identifier: MIT
// Negative control for the format parity test. TEST BUILDS ONLY: this file is
// compiled only into the *Fault targets of scripts/format_parity/parity.cmake,
// never into a shipped shell.
//
// Those targets compile the unmodified shell sources with
//   -Dop_shell_process=parity_faulty_process
// so the shell calls this function instead of the engine. It runs the real
// engine, then scales the output by (1 + PARITY_FAULT_SCALE), a gain error of
// 8.7e-4 dB at the default 1e-4. A parity test that cannot tell this from a
// correct shell is too loose.
#undef op_shell_process
#include "op_shell.h"

#ifndef PARITY_FAULT_SCALE
#define PARITY_FAULT_SCALE 1e-4f
#endif

extern "C" uint32_t parity_faulty_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    const uint32_t ok = op_shell_process(instance, in_left, in_right, out_left, out_right, frames);
    const float k = 1.0f + PARITY_FAULT_SCALE;
    for (uint32_t i = 0; i < frames; ++i) {
        out_left[i] *= k;
        if (out_right) out_right[i] *= k;
    }
    return ok;
}
