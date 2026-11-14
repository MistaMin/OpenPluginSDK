// SPDX-License-Identifier: MIT
// AAX native process callback. Plain values arrive in the packet. Non-finite
// values silence the block. Bypass outputs the input delayed by the reported
// latency, crossfaded over 10 ms, and the engine keeps running while bypassed.
#include "OpenPluginAAX.h"
#include <algorithm>
#include <cmath>
#include <cstring>
namespace openplugin_aax {
ProcessState::~ProcessState() {
    if (engine) op_shell_destroy(engine);
}
bool ProcessState::prepare(double rate, uint32_t channelCount) {
    if (!std::isfinite(rate) || rate < 8000.0 || rate > 768000.0) return false;
    if (channelCount != 1 && channelCount != 2) return false;
    OpShellInfo info{};
    if (!op_shell_info(&info) || info.abi != OP_SHELL_ABI || info.param_count == 0 || info.param_count > kMaxParams) return false;
    void* next = op_shell_create(rate, kMaxBlock, channelCount);
    if (!next) return false;
    if (engine) op_shell_destroy(engine);
    engine = next;
    channels = channelCount;
    sampleRate = static_cast<uint32_t>(rate);
    paramCount = info.param_count;
    latency = op_shell_latency(engine);
    for (uint32_t c = 0; c < 2; ++c) {
        dry[c].assign(c < channelCount ? kMaxBlock : 0, 0.f);
        line[c].assign(c < channelCount ? latency : 0, 0.f);
    }
    linePos = 0;
    mixStep = static_cast<float>(1.0 / std::max(1.0, kBypassFadeSeconds * rate));
    mix = 0.f;
    mixPending = true;
    prepared.store(true, std::memory_order_release);
    return true;
}
void ProcessState::reset() {
    if (engine) op_shell_reset(engine);
    for (auto& l : line) std::fill(l.begin(), l.end(), 0.f);
    linePos = 0;
    mixPending = true;
}
namespace {
// Pushes the input through the latency delay into state.dry. The line always
// runs, so switching bypass on never starts from stale audio. Taken before the
// engine runs because the host may give the same buffer for input and output.
void captureDry(ProcessState& state, Context* context, unsigned offset, unsigned count) {
    for (uint32_t c = 0; c < state.channels; ++c) {
        const float* in = context->input[c] + offset;
        float* dry = state.dry[c].data();
        if (state.latency == 0) {
            std::memcpy(dry, in, count * sizeof(float));
            continue;
        }
        float* line = state.line[c].data();
        size_t pos = state.linePos;
        for (unsigned i = 0; i < count; ++i) {
            dry[i] = line[pos];
            line[pos] = in[i];
            if (++pos == state.latency) pos = 0;
        }
    }
    if (state.latency != 0) state.linePos = (state.linePos + count) % state.latency;
}
// Moves the engine output towards the dry signal (or back). Fully bypassed
// output is the dry signal exactly.
void applyBypass(ProcessState& state, Context* context, unsigned offset, unsigned count, float target) {
    if (state.mix == target) {
        if (target == 0.f) return;
        for (uint32_t c = 0; c < state.channels; ++c) std::memcpy(context->output[c] + offset, state.dry[c].data(), count * sizeof(float));
        return;
    }
    float mix = state.mix;
    for (unsigned i = 0; i < count; ++i) {
        if (mix < target) mix = std::min(target, mix + state.mixStep);
        else mix = std::max(target, mix - state.mixStep);
        for (uint32_t c = 0; c < state.channels; ++c) {
            float& out = context->output[c][offset + i];
            const float dry = state.dry[c][i];
            out = mix >= 1.f ? dry : out + (dry - out) * mix;
        }
    }
    state.mix = mix;
}
}
void AAX_CALLBACK process(Context* const begin[], const void* end) {
    for (auto current = begin; current < end; ++current) {
        Context* context = *current;
        if (!context || !context->frames || *context->frames <= 0 || !context->state || !*context->state || !context->output) continue;
        ProcessState& state = **context->state;
        const auto frames = static_cast<unsigned>(*context->frames);
        const auto silence = [&] {
            for (uint32_t channel = 0; channel < state.channels; ++channel) {
                if (context->output[channel]) std::fill_n(context->output[channel], frames, 0.f);
            }
        };
        if (!state.engine || !context->controls || !context->input || !context->input[0] || !context->output[0]) {
            silence();
            continue;
        }
        if (state.channels == 2 && (!context->input[1] || !context->output[1])) {
            silence();
            continue;
        }
        const Packet& packet = *context->controls;
        const float* plain = packet.plain();
        const bool bypass = packet.bypass != 0;
        // The port holds paramCount values (its size in Describe), so a larger count is refused.
        bool valid = packet.count <= state.paramCount;
        for (uint32_t i = 0; valid && i < packet.count; ++i) valid = std::isfinite(plain[i]);
        // A refused packet silences the block, unless the signal is bypassed:
        // that does not depend on the values.
        if (!valid && !bypass) {
            silence();
            continue;
        }
        if (valid) {
            bool applied = true;
            for (uint32_t i = 0; applied && i < packet.count; ++i) applied = op_shell_set_plain(state.engine, i, plain[i]) != 0;
            if (!applied && !bypass) {
                silence();
                continue;
            }
        }
        const float target = bypass ? 1.f : 0.f;
        if (state.mixPending) {
            state.mix = target;
            state.mixPending = false;
        }
        bool failed = false;
        for (unsigned offset = 0; offset < frames;) {
            const auto count = std::min(kMaxBlock, frames - offset);
            const float* inRight = state.channels > 1 ? context->input[1] + offset : nullptr;
            float* outRight = state.channels > 1 ? context->output[1] + offset : nullptr;
            captureDry(state, context, offset, count);
            if (!op_shell_process(state.engine, context->input[0] + offset, inRight, context->output[0] + offset, outRight, count)) {
                failed = true;
                // Output that is fully bypassed does not need the engine.
                if (!bypass || state.mix != 1.f) break;
            }
            applyBypass(state, context, offset, count, target);
            offset += count;
        }
        if (failed && !(bypass && state.mix == 1.f)) silence();
    }
}
}
