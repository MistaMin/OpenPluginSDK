// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI that records what the shell tells it
// about the host's track (op_shell_set_track_info), used only by the VST3 track
// proof. The SDK example's plugin keeps no track, so it cannot show what the
// shell passed. One parameter (level). The recording lives in this module and
// the proof reads it through the op_stub_track_* functions. It is not a plugin
// and is never shipped.
#include "op_shell.h"
#include <cstring>
#include <new>
#include <pthread.h>

#define OP_STUB_OWN_TRACK 1

namespace {
constexpr uint32_t kMaxCalls = 64;
struct Call {
    char name[300];
    bool has_name;
    uint32_t rgba;
    uint32_t flags;
    void* instance;
    bool main_thread;
};
Call gCalls[kMaxCalls];
uint32_t gCount = 0;
struct Stub {
    float level = 0.5f;
};
void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
}

#define OP_STUB_EXPORT __attribute__((visibility("default")))

extern "C" {
// How many times the shell told the plugin about the track, and the n-th call
// (0 is the first). Returns 0 for a call that does not exist.
OP_STUB_EXPORT uint32_t op_stub_track_count(void) { return gCount; }
OP_STUB_EXPORT uint32_t op_stub_track_call(uint32_t n, char* name, uint32_t capacity, uint32_t* has_name, uint32_t* rgba, uint32_t* flags,
                                           uint32_t* main_thread) {
    if (n >= gCount) return 0;
    const Call& c = gCalls[n];
    if (name && capacity) {
        std::strncpy(name, c.name, capacity - 1);
        name[capacity - 1] = 0;
    }
    if (has_name) *has_name = c.has_name;
    if (rgba) *rgba = c.rgba;
    if (flags) *flags = c.flags;
    if (main_thread) *main_thread = c.main_thread;
    return 1;
}
OP_STUB_EXPORT void op_stub_track_reset(void) { gCount = 0; }

uint32_t op_shell_set_track_info(void* instance, const char* name, uint32_t rgba, uint32_t flags) {
    if (!instance || gCount >= kMaxCalls) return 0;
    Call& c = gCalls[gCount++];
    std::memset(&c, 0, sizeof(c));
    c.has_name = name != nullptr;
    if (name) std::strncpy(c.name, name, sizeof(c.name) - 1);
    c.rgba = rgba;
    c.flags = flags;
    c.instance = instance;
    c.main_thread = pthread_main_np() != 0;
    return 1;
}

uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 1;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x50 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0xC0 + i);
    }
    put(out->name, sizeof(out->name), "Track Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), "Fx");
    put(out->id, sizeof(out->id), "proof.track-stub");
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
// ABI 9: this stub reads no other plug-in's state.
uint32_t op_shell_foreign_kinds(void) { return 0; }
uint32_t op_shell_load_foreign_state(void*, uint32_t, const uint8_t*, uint32_t) { return 0; }
uint32_t op_shell_latency(void*) { return 0; }
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
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(float);
    if (!out || capacity < sizeof(float)) return capacity > 0 && !out ? OP_STATE_FAILED : OP_STATE_TOO_SMALL;
    std::memcpy(out, &static_cast<Stub*>(instance)->level, sizeof(float));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != sizeof(float)) return 0;
    std::memcpy(&static_cast<Stub*>(instance)->level, data, sizeof(float));
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
#include "stub_abi2.h"
