// SPDX-License-Identifier: MIT
// The shell ABI 2 calls for the stand-in plugins of the Audio Unit proofs that predate
// ABI 2 (latency_shell.cpp, state_shell.cpp). They are mono and stereo effects with no
// notes, no layouts and no sidechain; process_events maps onto their op_shell_process.
// Include after the stand-in's own op_shell_process. Test code, never linked into a product.
#pragma once
#include "op_shell.h"

extern "C" {
uint32_t op_shell_abi_version(void) { return OP_SHELL_ABI; }
uint32_t op_shell_kind(void) { return OP_KIND_EFFECT; }
uint32_t op_shell_note_ports(void) { return 0; }
uint32_t op_shell_layout_count(void) { return 0; }
uint32_t op_shell_layout(uint32_t, OpLayout*) { return 0; }
uint32_t op_shell_set_layout(void*, uint32_t) { return 0; }
uint32_t op_shell_layout_selected(void*) { return 0; }
void op_shell_set_offline(void*, uint32_t) {}
uint32_t op_shell_midi_out(void*, OpMidiEvent*, uint32_t) { return 0; }
uint32_t op_shell_midi_dropped(const void*) { return 0; }
uint32_t op_shell_process_events(void* instance, const OpProcess* args) {
    if (!instance || !args || args->struct_size < sizeof(OpProcess) || !args->inputs || !args->outputs) return 0;
    if (args->input_channels < 1 || args->output_channels < 1) return 0;
    return op_shell_process(instance, args->inputs[0], args->input_channels > 1 ? args->inputs[1] : nullptr,
                            args->outputs[0], args->output_channels > 1 ? args->outputs[1] : nullptr, args->frames);
}
}
