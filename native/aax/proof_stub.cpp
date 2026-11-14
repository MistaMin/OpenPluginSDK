// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI, used only by the AAX bypass,
// parameter-count and allocation proofs. The SDK example reports no latency and
// has two parameters, so it cannot show a delayed bypass or a large packet.
// This one reports OP_STUB_LATENCY samples, declares OP_STUB_PARAMS parameters,
// and outputs input * (p0 + 0.5 * p[last]) (p0 alone with one parameter). It is
// not a plugin and is never shipped.
//
// It also has a plugin-owned chunk, saved and loaded through op_shell_save_state
// and op_shell_load_state, to prove the AAX state chunk carries it. A non-empty
// chunk scales the output by 1 + chunk[0] / 512, so the chunk is audible. The
// blob is: u32 'STUB', u32 parameter count, f32 x count, u32 chunk length,
// chunk bytes. A load checks all of it first; a chunk starting with 0xFF is
// refused, as a plugin's load_state may refuse one. op_stub_* are test hooks.
#include "op_shell.h"
#include <cstdio>
#include <cstring>
#include <new>
#include <vector>

#ifndef OP_STUB_PARAMS
#define OP_STUB_PARAMS 1
#endif
#ifndef OP_STUB_LATENCY
#define OP_STUB_LATENCY 37
#endif

namespace {
constexpr uint32_t kParams = OP_STUB_PARAMS;
constexpr uint32_t kLatency = OP_STUB_LATENCY;
struct Stub {
    std::vector<float> plain;
    std::vector<uint8_t> chunk;
    uint32_t growOnQuery = 0;  // bytes the chunk grows by right after the next size query
    uint32_t loads = 0;
    Stub() : plain(kParams) {}
};
void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
constexpr uint32_t kBlobMagic = 0x42555453u;  // 'S','T','U','B'
size_t blobSize(const Stub& stub) { return 8 + 4 * static_cast<size_t>(kParams) + 4 + stub.chunk.size(); }
void put32(uint8_t* out, uint32_t v) { std::memcpy(out, &v, 4); }  // host byte order: the blob never leaves the process
uint32_t get32(const uint8_t* in) { uint32_t v; std::memcpy(&v, in, 4); return v; }
double defaultFor(uint32_t index) { return index == 0 ? 0.5 : 0.25 + 0.001 * index; }
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = kParams;
    out->manufacturer_id = 0x6f706d31;
    out->product_id = 0x6f707031;
    out->plugin_id_mono = 0x6f706d6f;
    out->plugin_id_stereo = 0x6f707374;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x10 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0x80 + i);
    }
    put(out->name, sizeof(out->name), "AAX Proof Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), "Fx");
    put(out->id, sizeof(out->id), "proof.aax-stub");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index >= kParams || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 1.0;
    out->default_plain = defaultFor(index);
    out->default_normalized = defaultFor(index);
    out->flags = OP_PARAM_AUTOMATABLE;
    std::snprintf(out->name, sizeof(out->name), "Param %u", index);
    std::snprintf(out->id, sizeof(out->id), "p%u", index);
    return 1;
}
double op_shell_to_plain(uint32_t, double normalized) { return normalized; }
double op_shell_to_normalized(uint32_t, double plain) { return plain; }
void* op_shell_create(double, uint32_t, uint32_t) {
    auto* stub = new (std::nothrow) Stub;
    if (stub)
        for (uint32_t i = 0; i < kParams; ++i) stub->plain[i] = static_cast<float>(defaultFor(i));
    return stub;
}
void op_shell_destroy(void* instance) { delete static_cast<Stub*>(instance); }
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return kLatency; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index >= kParams) return 0;
    static_cast<Stub*>(instance)->plain[index] = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t index) {
    return instance && index < kParams ? static_cast<Stub*>(instance)->plain[index] : 0.0f;
}
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    const Stub& stub = *static_cast<Stub*>(instance);
    float gain = kParams > 1 ? stub.plain[0] + 0.5f * stub.plain[kParams - 1] : stub.plain[0];
    if (!stub.chunk.empty()) gain *= 1.f + static_cast<float>(stub.chunk[0]) / 512.f;
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * gain;
    if (in_right && out_right)
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * gain;
    return 1;
}
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    Stub& stub = *static_cast<Stub*>(instance);
    const size_t size = blobSize(stub);
    if (needed) *needed = static_cast<uint32_t>(size);
    if (!out || capacity < size) {
        if (!out && stub.growOnQuery) {
            stub.chunk.insert(stub.chunk.end(), stub.growOnQuery, 0x11);
            stub.growOnQuery = 0;
        }
        return OP_STATE_TOO_SMALL;
    }
    put32(out, kBlobMagic);
    put32(out + 4, kParams);
    for (uint32_t i = 0; i < kParams; ++i) std::memcpy(out + 8 + 4 * i, &stub.plain[i], 4);
    put32(out + 8 + 4 * kParams, static_cast<uint32_t>(stub.chunk.size()));
    if (!stub.chunk.empty()) std::memcpy(out + 12 + 4 * kParams, stub.chunk.data(), stub.chunk.size());
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length < 12 + 4 * kParams) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    if (get32(data) != kBlobMagic || get32(data + 4) != kParams) return 0;
    const uint32_t chunkLength = get32(data + 8 + 4 * kParams);
    if (length - (12 + 4 * kParams) != chunkLength) return 0;
    std::vector<float> plain(kParams);
    for (uint32_t i = 0; i < kParams; ++i) {
        std::memcpy(&plain[i], data + 8 + 4 * i, 4);
        if (!(plain[i] >= 0.f && plain[i] <= 1.f)) return 0;
    }
    const uint8_t* chunk = data + 12 + 4 * kParams;
    if (chunkLength > 0 && chunk[0] == 0xFF) return 0;
    stub.plain = plain;
    stub.chunk.assign(chunk, chunk + chunkLength);
    ++stub.loads;
    return 1;
}
// Test hooks, not part of the shell ABI.
uint32_t op_stub_set_chunk(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || (!data && length)) return 0;
    static_cast<Stub*>(instance)->chunk.assign(data, data + length);
    return 1;
}
uint32_t op_stub_chunk(void* instance, uint8_t* out, uint32_t capacity) {
    if (!instance) return 0;
    const Stub& stub = *static_cast<Stub*>(instance);
    if (out && capacity >= stub.chunk.size() && !stub.chunk.empty()) std::memcpy(out, stub.chunk.data(), stub.chunk.size());
    return static_cast<uint32_t>(stub.chunk.size());
}
uint32_t op_stub_loads(void* instance) { return instance ? static_cast<Stub*>(instance)->loads : 0; }
void op_stub_grow_on_query(void* instance, uint32_t bytes) {
    if (instance) static_cast<Stub*>(instance)->growOnQuery = bytes;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
