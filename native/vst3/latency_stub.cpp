// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI with a non-zero latency, used only by
// the bypass proofs. The SDK examples report no latency, so they cannot show that
// a bypassed signal is delayed to match. This one halves the signal and reports
// kLatency samples. It is not a plugin and is never shipped.
#include "op_shell.h"
#include <cstring>
#include <new>

namespace {
constexpr uint32_t kLatency = 37;
struct Stub {
    float level = 0.5f;
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
        out->processor_cid[i] = static_cast<uint8_t>(0x10 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0x80 + i);
    }
    put(out->name, sizeof(out->name), "Latency Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), "Fx");
    put(out->id, sizeof(out->id), "proof.latency-stub");
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
uint32_t op_shell_latency(void*) { return kLatency; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index != 0) return 0;
    static_cast<Stub*>(instance)->level = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t) { return instance ? static_cast<Stub*>(instance)->level : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    const float level = static_cast<Stub*>(instance)->level;
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * level;
    if (in_right && out_right)
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * level;
    return 1;
}
// The whole state is the level, four bytes. Whatever else is refused.
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(float);
    if (!out || capacity < sizeof(float)) return capacity > 0 && !out ? OP_STATE_FAILED : OP_STATE_TOO_SMALL;
    std::memcpy(out, &static_cast<Stub*>(instance)->level, sizeof(float));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != sizeof(float)) return 0;
    float level = 0.0f;
    std::memcpy(&level, data, sizeof(float));
    if (!(level >= 0.0f && level <= 1.0f)) return 0;
    static_cast<Stub*>(instance)->level = level;
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
