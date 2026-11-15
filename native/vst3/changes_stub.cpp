// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI whose latency follows a parameter,
// for the VST3 and Audio Unit change proofs (shell ABI 6). Test builds only;
// never shipped. It behaves as the Rust library does:
//   - parameter 0, "Lookahead", is normalized 0..1; the latency is
//     round(256 * value) samples. A stored value applies at the next block or
//     reset, as the parameter plane does;
//   - a block or reset that moves the latency raises OP_CHANGE_LATENCY;
//     op_shell_latency remembers what it returned, and op_shell_take_changes
//     drops a latency the host already read;
//   - op_stub_changes_rename() raises OP_CHANGE_PARAM_INFO, as a plugin's
//     HostNotifier::param_info_changed does.
// op_stub_changes_* are test hooks: the proof reads which threads called
// op_shell_take_changes and op_shell_latency.
#include "op_shell.h"
#include <atomic>
#include <cmath>
#include <cstring>
#include <new>
#include <pthread.h>

#define OP_STUB_OWN_CHANGES 1

namespace {
constexpr double kMaxLookahead = 256.0;
struct Stub {
    std::atomic<float> value{0.0f};   // stored (normalized), applied at the next block
    std::atomic<uint32_t> latency{0};  // applied
    std::atomic<uint32_t> told{0};     // what op_shell_latency last returned
    std::atomic<uint32_t> flags{0};
    float level = 1.0f;
    void apply() {
        const float v = value.load();
        const uint32_t next = static_cast<uint32_t>(std::lround(std::fmin(std::fmax(v, 0.0f), 1.0f) * kMaxLookahead));
        if (latency.exchange(next) != next) flags.fetch_or(OP_CHANGE_LATENCY);
    }
};
std::atomic<Stub*> gLast{nullptr};
std::atomic<uint32_t> gTakesMain{0}, gTakesOther{0}, gCreated{0};
void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
}

#define OP_STUB_EXPORT __attribute__((visibility("default")))

extern "C" {
// The newest engine raises OP_CHANGE_PARAM_INFO. Returns 0 when there is none.
OP_STUB_EXPORT uint32_t op_stub_changes_rename(void) {
    Stub* stub = gLast.load();
    if (!stub) return 0;
    stub->flags.fetch_or(OP_CHANGE_PARAM_INFO);
    return 1;
}
// How many op_shell_take_changes calls came from the main thread and from others.
OP_STUB_EXPORT void op_stub_changes_takes(uint32_t* main_thread, uint32_t* other) {
    if (main_thread) *main_thread = gTakesMain.load();
    if (other) *other = gTakesOther.load();
}
// Engines made so far (a VST3 restart makes a new one).
OP_STUB_EXPORT uint32_t op_stub_changes_engines(void) { return gCreated.load(); }

uint32_t op_shell_take_changes(void* instance) {
    if (!instance) return 0;
    (pthread_main_np() ? gTakesMain : gTakesOther).fetch_add(1);
    Stub& stub = *static_cast<Stub*>(instance);
    uint32_t flags = stub.flags.exchange(0);
    if ((flags & OP_CHANGE_LATENCY) && stub.latency.load() == stub.told.load()) flags &= ~static_cast<uint32_t>(OP_CHANGE_LATENCY);
    return flags;
}

uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 1;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x60 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0xD0 + i);
    }
    put(out->name, sizeof(out->name), "Changes Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), "Fx|Delay");
    put(out->id, sizeof(out->id), "proof.changes-stub");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index != 0 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 1.0;
    out->flags = OP_PARAM_AUTOMATABLE;
    put(out->name, sizeof(out->name), "Lookahead");
    put(out->id, sizeof(out->id), "lookahead");
    return 1;
}
double op_shell_to_plain(uint32_t, double normalized) { return normalized; }
double op_shell_to_normalized(uint32_t, double plain) { return plain; }
void* op_shell_create(double, uint32_t, uint32_t) {
    Stub* stub = new (std::nothrow) Stub;
    if (stub) {
        gLast.store(stub);
        gCreated.fetch_add(1);
    }
    return stub;
}
void op_shell_destroy(void* instance) {
    Stub* stub = static_cast<Stub*>(instance);
    Stub* expected = stub;
    gLast.compare_exchange_strong(expected, nullptr);
    delete stub;
}
void op_shell_reset(void* instance) {
    if (instance) static_cast<Stub*>(instance)->apply();
}
uint32_t op_shell_latency(void* instance) {
    if (!instance) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    const uint32_t now = stub.latency.load();
    stub.told.store(now);
    return now;
}
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index != 0 || !std::isfinite(plain)) return 0;
    static_cast<Stub*>(instance)->value.store(plain);
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t) { return instance ? static_cast<Stub*>(instance)->value.load() : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    stub.apply();
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * stub.level;
    if (in_right && out_right)
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * stub.level;
    return 1;
}
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(float);
    if (!out || capacity < sizeof(float)) return capacity > 0 && !out ? OP_STATE_FAILED : OP_STATE_TOO_SMALL;
    const float value = static_cast<Stub*>(instance)->value.load();
    std::memcpy(out, &value, sizeof(float));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != sizeof(float)) return 0;
    float value = 0.0f;
    std::memcpy(&value, data, sizeof(float));
    if (!(value >= 0.0f && value <= 1.0f)) return 0;
    static_cast<Stub*>(instance)->value.store(value);
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
#include "stub_abi2.h"
