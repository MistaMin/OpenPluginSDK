// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI that has a chunk of its own, used
// only by the VST3 state proof. The SDK example has no chunk, so it cannot show
// that the plugin's state travels through the shell's component state. This one
// has one parameter (level) and a chunk (gain); the audio is input * level *
// gain, so a restored chunk is audible. The blob layout is in state_stub.h; the
// stub validates all of it before it changes anything, and it can report a state
// that grows between size queries. It is not a plugin and is never shipped.
#include "op_shell.h"
#include "state_stub.h"
#include <cstring>
#include <new>
#include <vector>

namespace {
struct Stub {
    state_stub::Blob state;
};
void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 1;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x30 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0xA0 + i);
    }
    put(out->name, sizeof(out->name), "State Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), "Fx");
    put(out->id, sizeof(out->id), "proof.state-stub");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index != 0 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 1.0;
    out->default_plain = 0.5;
    out->default_normalized = 0.5;
    out->flags = OP_PARAM_AUTOMATABLE;
    put(out->name, sizeof(out->name), "Level");
    put(out->id, sizeof(out->id), "level");
    return 1;
}
double op_shell_to_plain(uint32_t, double normalized) { return normalized; }
double op_shell_to_normalized(uint32_t, double plain) { return plain; }
void* op_shell_create(double, uint32_t, uint32_t) { return new (std::nothrow) Stub; }
void op_shell_destroy(void* instance) { delete static_cast<Stub*>(instance); }
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return 0; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index != 0) return 0;
    static_cast<Stub*>(instance)->state.level = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t) { return instance ? static_cast<Stub*>(instance)->state.level : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    const auto& state = static_cast<Stub*>(instance)->state;
    const float scale = state.level * state.gain;
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * scale;
    if (in_right && out_right)
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * scale;
    return 1;
}
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    auto& state = static_cast<Stub*>(instance)->state;
    const uint32_t size = state_stub::kHeader + state.pad;
    if (needed) *needed = size;
    if (capacity < size) {
        // The state grows after it has been reported, as a plugin whose chunk
        // changes between the size query and the copy would.
        if (state.grow > 0) {
            --state.grow;
            state.pad = state.pad + 16 <= state_stub::kMaxPad ? state.pad + 16 : state.pad;
        }
        return OP_STATE_TOO_SMALL;
    }
    const std::vector<uint8_t> bytes = state_stub::encode(state);
    std::memcpy(out, bytes.data(), bytes.size());
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    state_stub::Blob blob;
    if (!instance || !state_stub::decode(data, length, blob)) return 0;
    static_cast<Stub*>(instance)->state = blob;
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
#include "stub_abi2.h"
