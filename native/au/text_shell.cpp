// SPDX-License-Identifier: MIT
// A stand-in for a Rust plugin, for the Audio Unit parameter text proof only. The SDK example has
// no choice parameter, so this implements the op_shell_* C ABI with four parameters whose text is
// the plugin's own and nothing like the plain number:
//   Level  -24..24 dB, continuous, "6.0 dB"; reads "6", "6 dB", "-inf"
//   Switch toggle, "Off" / "On"
//   Mode   choice of three, "Clean", "Soft", "Größe" (a label with a non-ASCII letter)
//   Steps  five whole steps 0..4, text "0".."4" (a stepped number, not a choice); its name ends
//          in half a UTF-8 sequence, so the unit must fall back to the id "steps"
// It passes audio through. Linked into a test bundle and never into a product.
#include "op_shell.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>

namespace {
constexpr uint32_t kParams = 4;
constexpr const char* kModes[] = {"Clean", "Soft", "Gr\xC3\xB6\xC3\x9F" "e"};
struct Engine {
    float values[kParams] = {0.0f, 0.0f, 0.0f, 0.0f};
};
void copy(char* dst, size_t size, const char* text) {
    std::strncpy(dst, text, size - 1);
    dst[size - 1] = 0;
}
double minOf(uint32_t index) { return index == 0 ? -24.0 : 0.0; }
double maxOf(uint32_t index) { return index == 0 ? 24.0 : index == 1 ? 1.0 : index == 2 ? 2.0 : 4.0; }
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = kParams;
    copy(out->name, sizeof(out->name), "Text Test");
    copy(out->vendor, sizeof(out->vendor), "OpenPlugin");
    copy(out->version, sizeof(out->version), "1");
    copy(out->id, sizeof(out->id), "test.text");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index >= kParams || !out) return 0;
    static const char* names[] = {"Level", "Switch", "Mode", "Steps\xC3"};
    static const char* ids[] = {"level", "switch", "mode", "steps"};
    std::memset(out, 0, sizeof(*out));
    out->min = minOf(index);
    out->max = maxOf(index);
    out->default_plain = 0.0;
    out->default_normalized = (0.0 - out->min) / (out->max - out->min);
    out->steps = index == 0 ? 0 : static_cast<uint32_t>(out->max) + 1;
    out->flags = (index == 0 ? 0 : OP_PARAM_DISCRETE) | OP_PARAM_AUTOMATABLE;
    if (index == 0) copy(out->unit, sizeof(out->unit), "dB");
    copy(out->name, sizeof(out->name), names[index]);
    copy(out->id, sizeof(out->id), ids[index]);
    return 1;
}
double op_shell_to_plain(uint32_t index, double normalized) { return minOf(index) + normalized * (maxOf(index) - minOf(index)); }
double op_shell_to_normalized(uint32_t index, double plain) { return (plain - minOf(index)) / (maxOf(index) - minOf(index)); }
size_t op_shell_param_format(uint32_t index, double plain, char* buf, size_t cap) {
    if (index >= kParams) {
        if (buf && cap > 0) buf[0] = 0;
        return 0;
    }
    if (!std::isfinite(plain)) plain = 0.0;
    char text[64];
    const long step = std::lround(plain);
    if (index == 0) std::snprintf(text, sizeof(text), "%.1f dB", plain);
    else if (index == 1) std::snprintf(text, sizeof(text), "%s", plain >= 0.5 ? "On" : "Off");
    else if (index == 2) std::snprintf(text, sizeof(text), "%s", kModes[step < 0 ? 0 : step > 2 ? 2 : step]);
    else std::snprintf(text, sizeof(text), "%ld", step);
    const size_t length = std::strlen(text);
    if (buf && cap > 0) {
        const size_t n = length < cap ? length : cap - 1;
        std::memcpy(buf, text, n);
        buf[n] = 0;
    }
    return length;
}
int op_shell_param_parse(uint32_t index, const char* text, double* out) {
    if (index >= kParams || !text || !out) return 0;
    if (index == 1 || index == 2) {
        const char* const* labels = index == 1 ? nullptr : kModes;
        if (index == 1) {
            if (!std::strcmp(text, "Off") || !std::strcmp(text, "off")) { *out = 0.0; return 1; }
            if (!std::strcmp(text, "On") || !std::strcmp(text, "on")) { *out = 1.0; return 1; }
            return 0;
        }
        for (int i = 0; i < 3; ++i) {
            if (!std::strcmp(text, labels[i])) { *out = i; return 1; }
        }
        return 0;
    }
    char* end = nullptr;
    const double value = std::strtod(text, &end);
    if (end == text) return 0;
    while (*end == ' ') ++end;
    if (index == 0 && !std::strcmp(end, "dB")) end += 2;
    if (*end) return 0;
    *out = std::fmin(std::fmax(value, minOf(index)), maxOf(index));
    return 1;
}
void* op_shell_create(double, uint32_t, uint32_t channels) { return channels == 1 || channels == 2 ? new (std::nothrow) Engine() : nullptr; }
void op_shell_destroy(void* instance) { delete static_cast<Engine*>(instance); }
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return 0; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index >= kParams) return 0;
    static_cast<Engine*>(instance)->values[index] = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t index) { return instance && index < kParams ? static_cast<Engine*>(instance)->values[index] : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) {
    return op_shell_set_plain(instance, index, static_cast<float>(op_shell_to_plain(index, normalized)));
}
float op_shell_get_normalized(void* instance, uint32_t index) { return static_cast<float>(op_shell_to_normalized(index, op_shell_get_plain(instance, index))); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    std::memcpy(out_left, in_left, frames * sizeof(float));
    if (in_right && out_right) std::memcpy(out_right, in_right, frames * sizeof(float));
    return 1;
}
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(Engine::values);
    if (!out || capacity < sizeof(Engine::values)) return OP_STATE_TOO_SMALL;
    std::memcpy(out, static_cast<Engine*>(instance)->values, sizeof(Engine::values));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != sizeof(Engine::values)) return 0;
    std::memcpy(static_cast<Engine*>(instance)->values, data, sizeof(Engine::values));
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}

// The ABI 2 calls, mapped onto the op_shell_process above. This file has its own text calls.
#define OP_STUB_HAS_PARAM_TEXT 1
#include "stub_abi2.h"
