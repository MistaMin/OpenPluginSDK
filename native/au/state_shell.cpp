// SPDX-License-Identifier: MIT
// A stand-in for a Rust plugin with a chunk of its own, for the Audio Unit
// state proof only. The SDK example has no chunk, so it cannot show that the
// shell carries one. This implements the op_shell_* C ABI with two parameters
// (Gain 0..4, Tilt 0..1) and a plugin-owned byte chunk, and saves both as one
// blob with the contract of op_shell_save_state / op_shell_load_state:
//   - size query on (null, 0), TOO_SMALL with *needed, a short buffer never written;
//   - load checks everything first and changes nothing on a refusal, including a
//     chunk the plugin itself refuses (any 0xFF byte).
// The op_test_* functions are for the proof to reach the chunk. It is linked
// into a test bundle and never into a product.
#include "op_shell.h"
#include <cmath>
#include <cstring>
#include <mutex>
#include <new>
#include <vector>

namespace {
constexpr uint32_t kMagic = 0x54535453;  // "STST"
constexpr size_t kHeader = 16;           // magic, gain, tilt, chunk length
struct Engine {
    float gain = 1.0f;
    float tilt = 0.5f;
    std::mutex lock;  // the proof saves from one thread while another renders
    std::vector<uint8_t> chunk;
    uint32_t growAfterQuery = 0;  // proof hook: the chunk grows once right after a size query
};
Engine* gLast = nullptr;
void copy(char* dst, size_t size, const char* text) {
    std::strncpy(dst, text, size - 1);
    dst[size - 1] = 0;
}
bool refusedChunk(const uint8_t* data, size_t length) {
    for (size_t i = 0; i < length; ++i) {
        if (data[i] == 0xFF) return true;
    }
    return false;
}
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 2;
    copy(out->name, sizeof(out->name), "State Test");
    copy(out->vendor, sizeof(out->vendor), "OpenPlugin");
    copy(out->version, sizeof(out->version), "1");
    copy(out->id, sizeof(out->id), "test.state");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index > 1 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = index == 0 ? 4.0 : 1.0;
    out->default_plain = index == 0 ? 1.0 : 0.5;
    out->default_normalized = out->default_plain / out->max;
    copy(out->name, sizeof(out->name), index == 0 ? "Gain" : "Tilt");
    copy(out->id, sizeof(out->id), index == 0 ? "gain" : "tilt");
    return 1;
}
double op_shell_to_plain(uint32_t index, double normalized) { return normalized * (index == 0 ? 4.0 : 1.0); }
double op_shell_to_normalized(uint32_t index, double plain) { return plain / (index == 0 ? 4.0 : 1.0); }
void* op_shell_create(double, uint32_t, uint32_t channels) {
    if (channels != 1 && channels != 2) return nullptr;
    gLast = new (std::nothrow) Engine();
    return gLast;
}
void op_shell_destroy(void* instance) {
    if (gLast == instance) gLast = nullptr;
    delete static_cast<Engine*>(instance);
}
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return 0; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index > 1) return 0;
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    (index == 0 ? engine->gain : engine->tilt) = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t index) {
    if (!instance || index > 1) return 0.0f;
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    return index == 0 ? engine->gain : engine->tilt;
}
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) {
    return op_shell_set_plain(instance, index, normalized * (index == 0 ? 4.0f : 1.0f));
}
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index) / (index == 0 ? 4.0f : 1.0f); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    float gain;
    {
        auto* engine = static_cast<Engine*>(instance);
        std::lock_guard<std::mutex> guard(engine->lock);
        gain = engine->gain;
    }
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * gain;
    if (in_right && out_right) {
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * gain;
    }
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif

uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    const uint32_t length = static_cast<uint32_t>(kHeader + engine->chunk.size());
    if (needed) *needed = length;
    if (!out || capacity < length) {
        // Proof hook: the state grows between the size query and the copy.
        if (!out && engine->growAfterQuery > 0) {
            engine->chunk.resize(engine->chunk.size() + engine->growAfterQuery, 0x11);
            engine->growAfterQuery = 0;
        }
        return OP_STATE_TOO_SMALL;
    }
    const uint32_t magic = kMagic;
    const uint32_t chunkLength = static_cast<uint32_t>(engine->chunk.size());
    std::memcpy(out, &magic, 4);
    std::memcpy(out + 4, &engine->gain, 4);
    std::memcpy(out + 8, &engine->tilt, 4);
    std::memcpy(out + 12, &chunkLength, 4);
    if (chunkLength) std::memcpy(out + kHeader, engine->chunk.data(), chunkLength);
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length < kHeader) return 0;
    uint32_t magic, chunkLength;
    float gain, tilt;
    std::memcpy(&magic, data, 4);
    std::memcpy(&gain, data + 4, 4);
    std::memcpy(&tilt, data + 8, 4);
    std::memcpy(&chunkLength, data + 12, 4);
    if (magic != kMagic || chunkLength != length - kHeader) return 0;
    if (!std::isfinite(gain) || gain < 0.0f || gain > 4.0f) return 0;
    if (!std::isfinite(tilt) || tilt < 0.0f || tilt > 1.0f) return 0;
    if (refusedChunk(data + kHeader, chunkLength)) return 0;
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    engine->gain = gain;
    engine->tilt = tilt;
    engine->chunk.assign(data + kHeader, data + kHeader + chunkLength);
    return 1;
}

// Proof hooks.
void* op_test_last_engine() { return gLast; }
void op_test_set_chunk(void* instance, const uint8_t* data, uint32_t length) {
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    engine->chunk.assign(data, data + length);
}
uint32_t op_test_get_chunk(void* instance, uint8_t* out, uint32_t capacity) {
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    const uint32_t length = static_cast<uint32_t>(engine->chunk.size());
    if (out && capacity >= length && length) std::memcpy(out, engine->chunk.data(), length);
    return length;
}
void op_test_grow_after_query(void* instance, uint32_t extra) {
    auto* engine = static_cast<Engine*>(instance);
    std::lock_guard<std::mutex> guard(engine->lock);
    engine->growAfterQuery = extra;
}
}
