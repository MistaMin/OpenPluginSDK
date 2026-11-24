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
//   - op_stub_changes_rename() renames the newest engine's table and raises
//     OP_CHANGE_PARAM_INFO, as a plugin's HostNotifier::param_info_changed
//     does. The renamed table (ABI 9, op_shell_instance_param*) calls the
//     parameter "Lookahead (renamed)" in unit "smp" and shows its value as the
//     latency it gives ("128 smp (renamed)"); the calls without an instance
//     keep the first table ("Lookahead", the stand-ins' text);
//   - op_shell_max_latency is 256, the largest latency the parameter gives, as
//     a plugin's Plugin::max_latency declares it. op_stub_changes_declare_max(0)
//     makes engines made after it declare none (the largest is then the
//     latency), as a plugin without the declaration: the negative control of
//     the bypass that follows the latency.
// op_stub_changes_* are test hooks: the proof reads which threads called
// op_shell_take_changes and op_shell_latency.
#include "op_shell.h"
#include <atomic>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <new>
#include <pthread.h>
#include <vector>

#define OP_STUB_OWN_CHANGES 1
#define OP_STUB_OWN_ABI9 1

namespace {
constexpr double kMaxLookahead = 256.0;
struct Stub {
    std::atomic<float> value{0.0f};   // stored (normalized), applied at the next block
    std::atomic<uint32_t> latency{0};  // applied
    std::atomic<uint32_t> told{0};     // what op_shell_latency last returned
    std::atomic<uint32_t> flags{0};
    std::atomic<bool> renamed{false};
    bool declaresMax = true;
    float level = 1.0f;
    void apply() {
        const float v = value.load();
        const uint32_t next = static_cast<uint32_t>(std::lround(std::fmin(std::fmax(v, 0.0f), 1.0f) * kMaxLookahead));
        if (latency.exchange(next) != next) flags.fetch_or(OP_CHANGE_LATENCY);
    }
};
std::atomic<Stub*> gLast{nullptr};
std::atomic<uint32_t> gTakesMain{0}, gTakesOther{0}, gCreated{0};
std::atomic<bool> gDeclareMax{true};
// Engines alive now: an ABI 9 call reads an instance's table only when it is one of them.
std::mutex gLiveLock;
std::vector<const void*> gLive;
bool renamed(const void* instance) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    if (std::find(gLive.begin(), gLive.end(), instance) == gLive.end()) return false;
    return static_cast<const Stub*>(instance)->renamed.load();
}
size_t put_text(const char* text, char* buf, size_t cap) {
    const size_t length = std::strlen(text);
    if (buf && cap > 0) {
        const size_t count = length < cap - 1 ? length : cap - 1;
        std::memcpy(buf, text, count);
        buf[count] = 0;
    }
    return length;
}
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
    stub->renamed.store(true);
    stub->flags.fetch_or(OP_CHANGE_PARAM_INFO);
    return 1;
}
// Whether engines made from now on declare their largest latency (default 1).
OP_STUB_EXPORT void op_stub_changes_declare_max(uint32_t on) { gDeclareMax.store(on != 0); }
// The name of parameter 0 through the call without an instance (op_shell_param): the
// first table, whatever an engine was renamed to. Writes at most `cap` bytes with the NUL.
OP_STUB_EXPORT uint32_t op_stub_changes_type_name(char* buf, uint32_t cap) {
    OpParamDesc desc{};
    if (!buf || cap == 0 || !op_shell_param(0, &desc)) return 0;
    put(buf, cap, desc.name);
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
        stub->declaresMax = gDeclareMax.load();
        {
            std::lock_guard<std::mutex> lock(gLiveLock);
            gLive.push_back(stub);
        }
        gLast.store(stub);
        gCreated.fetch_add(1);
    }
    return stub;
}
void op_shell_destroy(void* instance) {
    Stub* stub = static_cast<Stub*>(instance);
    Stub* expected = stub;
    gLast.compare_exchange_strong(expected, nullptr);
    {
        std::lock_guard<std::mutex> lock(gLiveLock);
        gLive.erase(std::remove(gLive.begin(), gLive.end(), static_cast<const void*>(stub)), gLive.end());
    }
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
// ---- ABI 9 ----
uint32_t op_shell_instance_param(const void* instance, uint32_t index, OpParamDesc* out) {
    if (!op_shell_param(index, out)) return 0;
    if (renamed(instance)) {
        put(out->name, sizeof(out->name), "Lookahead (renamed)");
        put(out->unit, sizeof(out->unit), "smp");
    }
    return 1;
}
size_t op_shell_instance_param_format(const void* instance, uint32_t index, double plain, char* buf, size_t cap) {
    if (!renamed(instance)) return op_shell_param_format(index, plain, buf, cap);
    if (index != 0) return put_text("", buf, cap);
    char text[64];
    std::snprintf(text, sizeof(text), "%.0f smp (renamed)", std::fmin(std::fmax(plain, 0.0), 1.0) * kMaxLookahead);
    return put_text(text, buf, cap);
}
int op_shell_instance_param_parse(const void* instance, uint32_t index, const char* text, double* out_plain) {
    if (!renamed(instance)) return op_shell_param_parse(index, text, out_plain);
    if (index != 0 || !text || !out_plain) return 0;
    char* end = nullptr;
    const double samples = std::strtod(text, &end);
    if (end == text || !std::isfinite(samples)) return 0;
    *out_plain = std::fmin(std::fmax(samples / kMaxLookahead, 0.0), 1.0);
    return 1;
}
uint32_t op_shell_max_latency(const void* instance) {
    if (!instance) return 0;
    const Stub& stub = *static_cast<const Stub*>(instance);
    return stub.declaresMax ? static_cast<uint32_t>(kMaxLookahead) : stub.latency.load();
}
uint32_t op_shell_latency_now(const void* instance) { return instance ? static_cast<const Stub*>(instance)->latency.load() : 0; }
}
#include "stub_abi2.h"
