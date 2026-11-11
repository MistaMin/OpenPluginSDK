// SPDX-License-Identifier: MIT
// AAX native process callback. Plain values arrive in the packet. Non-finite
// values silence the block. Bypass copies the dry buffer and does not delay it.
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
    prepared.store(true, std::memory_order_release);
    return true;
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
        if (packet.bypass) {
            for (uint32_t channel = 0; channel < state.channels; ++channel)
                std::copy_n(context->input[channel], frames, context->output[channel]);
            continue;
        }
        bool finite = packet.count <= kMaxParams;
        for (uint32_t i = 0; finite && i < packet.count && i < state.paramCount; ++i)
            finite = std::isfinite(packet.plain[i]);
        if (!finite) {
            silence();
            continue;
        }
        for (uint32_t i = 0; i < packet.count && i < state.paramCount; ++i) {
            if (!op_shell_set_plain(state.engine, i, packet.plain[i])) {
                silence();
                finite = false;
                break;
            }
        }
        if (!finite) continue;
        bool failed = false;
        for (unsigned offset = 0; offset < frames;) {
            const auto count = std::min(kMaxBlock, frames - offset);
            const float* inRight = state.channels > 1 ? context->input[1] + offset : nullptr;
            float* outRight = state.channels > 1 ? context->output[1] + offset : nullptr;
            if (!op_shell_process(state.engine, context->input[0] + offset, inRight, context->output[0] + offset, outRight, count)) {
                failed = true;
                break;
            }
            offset += count;
        }
        if (failed) silence();
    }
}
}
