// SPDX-License-Identifier: MIT
// The ABI 2 half of the stand-in plugins (latency_stub.cpp, state_stub.cpp). Test
// builds only. A stub that defines the ABI 1 calls (op_shell_process,
// op_shell_set_normalized, ...) includes this once to answer the ABI 2 calls as a
// plain stereo effect: one stereo layout, no notes, parameter events applied at
// their offsets, then the ABI 1 process on the first two channels.
#pragma once
#include "op_shell.h"
#include "stub_text.h"
#include <cstring>
#include <initializer_list>

extern "C" {
uint32_t op_shell_abi_version(void) { return OP_SHELL_ABI; }
uint32_t op_shell_kind(void) { return OP_KIND_EFFECT; }
uint32_t op_shell_note_ports(void) { return 0; }
uint32_t op_shell_layout_count(void) { return 1; }
uint32_t op_shell_layout(uint32_t index, OpLayout* out) {
    if (index != 0 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    for (OpBus* bus : {&out->input, &out->output}) {
        bus->kind = OP_BUS_STEREO;
        bus->channels = 2;
        std::memset(bus->roles, OP_ROLE_NONE, sizeof(bus->roles));
        bus->roles[0] = 0;
        bus->roles[1] = 1;
    }
    std::memset(out->sidechain.roles, OP_ROLE_NONE, sizeof(out->sidechain.roles));
    std::strcpy(out->name, "Stereo");
    return 1;
}
uint32_t op_shell_set_layout(void*, uint32_t index) { return index == 0; }
uint32_t op_shell_layout_selected(void*) { return 0; }
void op_shell_set_offline(void*, uint32_t) {}
uint32_t op_shell_midi_out(void*, OpMidiEvent*, uint32_t) { return 0; }
uint32_t op_shell_midi_dropped(const void*) { return 0; }
uint32_t op_shell_process_events(void* instance, const OpProcess* args) {
    if (!instance || !args || args->struct_size < sizeof(OpProcess) || !args->inputs || !args->outputs || args->input_channels < 1 || args->output_channels < 1)
        return 0;
    uint32_t pos = 0;
    while (pos < args->frames) {
        uint32_t next = args->frames;
        for (uint32_t i = 0; i < args->event_count; ++i) {
            const OpEvent& e = args->events[i];
            if (e.kind != OP_EVENT_PARAM_NORMALIZED || e.offset >= args->frames) continue;
            if (e.offset <= pos) op_shell_set_normalized(instance, e.index, e.value);
            else if (e.offset < next) next = e.offset;
        }
        const bool stereo = args->input_channels > 1 && args->output_channels > 1;
        if (!op_shell_process(instance, args->inputs[0] + pos, stereo ? args->inputs[1] + pos : nullptr, args->outputs[0] + pos,
                              stereo ? args->outputs[1] + pos : nullptr, next - pos))
            return 0;
        pos = next;
    }
    return 1;
}
}
