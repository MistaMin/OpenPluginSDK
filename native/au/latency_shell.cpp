// SPDX-License-Identifier: MIT
// A stand-in for a Rust plugin, for the Audio Unit proofs only. It implements
// the op_shell_* C ABI with a gain and a reported latency of 37 samples, which
// the SDK example (latency 0) cannot exercise. It is linked into a test bundle
// and never into a product.
#include "op_shell.h"
#include <cstring>
#include <new>

namespace {
constexpr uint32_t kLatency = 37;
constexpr double kGainDefault = 2.0;
struct Engine {
    float gain = static_cast<float>(kGainDefault);
};
void copy(char* dst, size_t size, const char* text) {
    std::strncpy(dst, text, size - 1);
    dst[size - 1] = 0;
}
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 1;
    copy(out->name, sizeof(out->name), "Latency Test");
    copy(out->vendor, sizeof(out->vendor), "OpenPlugin");
    copy(out->version, sizeof(out->version), "1");
    copy(out->id, sizeof(out->id), "test.latency");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index != 0 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 4.0;
    out->default_plain = kGainDefault;
    out->default_normalized = kGainDefault / 4.0;
    copy(out->name, sizeof(out->name), "Gain");
    copy(out->id, sizeof(out->id), "gain");
    return 1;
}
double op_shell_to_plain(uint32_t, double normalized) { return normalized * 4.0; }
double op_shell_to_normalized(uint32_t, double plain) { return plain / 4.0; }
void* op_shell_create(double, uint32_t, uint32_t channels) { return channels == 1 || channels == 2 ? new (std::nothrow) Engine() : nullptr; }
void op_shell_destroy(void* instance) { delete static_cast<Engine*>(instance); }
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return kLatency; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index != 0) return 0;
    static_cast<Engine*>(instance)->gain = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t) { return instance ? static_cast<Engine*>(instance)->gain : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized * 4.0f); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index) / 4.0f; }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    const float gain = static_cast<Engine*>(instance)->gain;
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * gain;
    if (in_right && out_right) {
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * gain;
    }
    return 1;
}
// State: the gain as four bytes, no chunk. The shell calls these for ClassInfo.
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(float);
    if (!out || capacity < sizeof(float)) return OP_STATE_TOO_SMALL;
    std::memcpy(out, &static_cast<Engine*>(instance)->gain, sizeof(float));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != sizeof(float)) return 0;
    float gain;
    std::memcpy(&gain, data, sizeof(float));
    if (!(gain >= 0.0f && gain <= 4.0f)) return 0;
    static_cast<Engine*>(instance)->gain = gain;
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}

// The ABI 2 calls, mapped onto the op_shell_process above.
#include "stub_abi2.h"
