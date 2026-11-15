// SPDX-License-Identifier: MIT
// The shell ABI 2 calls for the stand-in plugins of the Audio Unit proofs that predate
// ABI 2 (latency_shell.cpp, state_shell.cpp). They are mono and stereo effects with no
// notes, no layouts and no sidechain; process_events maps onto their op_shell_process.
// Include after the stand-in's own op_shell_process. Test code, never linked into a product.
#pragma once
// Define OP_STUB_HAS_PARAM_TEXT before including to bring your own op_shell_param_format/parse.
#include "op_shell.h"
#include <cstdio>
#include <cstdlib>

extern "C" {
#ifndef OP_STUB_HAS_PARAM_TEXT
// ABI 3 parameter text: the plain number with "%g", read back with strtod.
size_t op_shell_param_format(uint32_t, double plain, char* buf, size_t cap) {
    char text[64];
    const int length = std::snprintf(text, sizeof(text), "%g", plain);
    if (buf && cap > 0) { std::snprintf(buf, cap, "%s", text); }
    return length < 0 ? 0 : static_cast<size_t>(length);
}
int op_shell_param_parse(uint32_t, const char* text, double* out) {
    if (!text || !out) return 0;
    char* end = nullptr;
    const double value = std::strtod(text, &end);
    if (end == text || *end) return 0;
    *out = value;
    return 1;
}
#endif
#ifndef OP_STUB_HAS_PARAM_GROUP
// ABI 5 parameter groups: none, every parameter is top level.
size_t op_shell_param_group(uint32_t, char* buf, size_t cap) {
    if (buf && cap > 0) buf[0] = '\0';
    return 0;
}
#endif
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
#ifndef OP_STUB_OWN_CHANGES
// ABI 6: these stand-ins never change their latency or parameter info.
uint32_t op_shell_take_changes(void*) { return 0; }
#endif
uint32_t op_shell_process_events(void* instance, const OpProcess* args) {
    if (!instance || !args || args->struct_size < sizeof(OpProcess) || !args->inputs || !args->outputs) return 0;
    if (args->input_channels < 1 || args->output_channels < 1) return 0;
    return op_shell_process(instance, args->inputs[0], args->input_channels > 1 ? args->inputs[1] : nullptr,
                            args->outputs[0], args->output_channels > 1 ? args->outputs[1] : nullptr, args->frames);
}
}
