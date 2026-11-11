// SPDX-License-Identifier: MIT
// Thin Audio Unit adapter over op_shell_*. No editor view.
#include "op_shell.h"
#include <AudioUnitSDK/AUEffectBase.h>
#include <algorithm>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>

namespace {
struct CachedParam {
    OpParamDesc desc;
};
class OpenPluginAU final : public ausdk::AUEffectBase {
public:
    explicit OpenPluginAU(AudioComponentInstance instance) : AUEffectBase(instance, false) {
        CreateElements();
        OpShellInfo info{};
        if (op_shell_info(&info) && info.abi == OP_SHELL_ABI) {
            params_.resize(info.param_count);
            Globals()->UseIndexedParameters(info.param_count);
            for (uint32_t i = 0; i < info.param_count; ++i) {
                if (!op_shell_param(i, &params_[i].desc)) continue;
                Globals()->SetParameter(i, static_cast<float>(params_[i].desc.default_plain));
            }
        }
    }
    ~OpenPluginAU() override { destroy(); }
    bool CanScheduleParameters() const noexcept override { return false; }
    UInt32 SupportedNumChannels(const AUChannelInfo** info) override {
        static const AUChannelInfo channels[] = {{1, 1}, {2, 2}};
        if (info) *info = channels;
        return 2;
    }
    OSStatus Initialize() override {
        const OSStatus status = AUEffectBase::Initialize();
        if (status != noErr) return status;
        destroy();
        const double rate = GetSampleRate();
        const UInt32 channels = GetNumberOfChannels();
        UInt32 frames = GetMaxFramesPerSlice();
        if (frames < 1) frames = 512;
        if (frames > 65536) frames = 65536;
        maxFrames_ = frames;
        engine_ = op_shell_create(rate, frames, channels);
        if (!engine_) return kAudioUnitErr_FormatNotSupported;
        op_shell_reset(engine_);
        return noErr;
    }
    void Cleanup() override {
        destroy();
        AUEffectBase::Cleanup();
    }
    OSStatus Reset(AudioUnitScope scope, AudioUnitElement element) override {
        if (engine_) op_shell_reset(engine_);
        return AUEffectBase::Reset(scope, element);
    }
    Float64 GetLatency() override {
        const double rate = GetSampleRate();
        if (!engine_ || rate <= 0.0) return 0.0;
        return static_cast<Float64>(op_shell_latency(engine_)) / rate;
    }
    OSStatus GetParameterInfo(AudioUnitScope scope, AudioUnitParameterID id, AudioUnitParameterInfo& info) override {
        if (scope != kAudioUnitScope_Global || id >= params_.size()) return kAudioUnitErr_InvalidParameter;
        std::memset(&info, 0, sizeof(info));
        const OpParamDesc& desc = params_[id].desc;
        strlcpy(info.name, desc.name, sizeof(info.name));
        info.minValue = static_cast<float>(desc.min);
        info.maxValue = static_cast<float>(desc.max);
        info.defaultValue = static_cast<float>(desc.default_plain);
        info.flags = kAudioUnitParameterFlag_IsReadable | kAudioUnitParameterFlag_IsWritable;
        if (desc.flags & OP_PARAM_LOG) info.flags |= kAudioUnitParameterFlag_DisplayLogarithmic;
        if (std::strcmp(desc.unit, "dB") == 0) info.unit = kAudioUnitParameterUnit_Decibels;
        else if (std::strcmp(desc.unit, "Hz") == 0) info.unit = kAudioUnitParameterUnit_Hertz;
        else if (std::strcmp(desc.unit, "ms") == 0) info.unit = kAudioUnitParameterUnit_Milliseconds;
        else if (std::strcmp(desc.unit, "%") == 0) info.unit = kAudioUnitParameterUnit_Percent;
        else if (std::strcmp(desc.unit, ":1") == 0) info.unit = kAudioUnitParameterUnit_Ratio;
        else if ((desc.flags & OP_PARAM_DISCRETE) && desc.max == 1.0 && desc.steps == 2) info.unit = kAudioUnitParameterUnit_Boolean;
        else info.unit = kAudioUnitParameterUnit_Generic;
        return noErr;
    }
    OSStatus SetParameter(AudioUnitParameterID id, AudioUnitScope scope, AudioUnitElement element, AudioUnitParameterValue value, UInt32 offset) override {
        if (scope == kAudioUnitScope_Global && element == 0) {
            if (id >= params_.size() || !std::isfinite(value)) return kAudioUnitErr_InvalidParameter;
            const OpParamDesc& desc = params_[id].desc;
            if (value < desc.min - 1e-4 || value > desc.max + 1e-4) return kAudioUnitErr_InvalidParameter;
        }
        return AUEffectBase::SetParameter(id, scope, element, value, offset);
    }
    OSStatus ProcessBufferLists(AudioUnitRenderActionFlags& flags, const AudioBufferList& input, AudioBufferList& output, UInt32 frames) override {
        if (!engine_ || frames == 0) return kAudioUnitErr_Uninitialized;
        const UInt32 channels = input.mNumberBuffers;
        if ((channels != 1 && channels != 2) || output.mNumberBuffers != channels) return kAudioUnitErr_FormatNotSupported;
        for (UInt32 i = 0; i < channels; ++i) {
            if (!input.mBuffers[i].mData || !output.mBuffers[i].mData) return kAudio_ParamError;
        }
        for (uint32_t i = 0; i < params_.size(); ++i) {
            const float plain = GetParameter(i);
            if (!std::isfinite(plain) || !op_shell_set_plain(engine_, i, plain)) return kAudioUnitErr_InvalidParameter;
        }
        const UInt32 limit = maxFrames_ > 0 ? maxFrames_ : frames;
        for (UInt32 offset = 0; offset < frames;) {
            const uint32_t count = static_cast<uint32_t>(std::min(limit, frames - offset));
            auto* inLeft = static_cast<const float*>(input.mBuffers[0].mData) + offset;
            auto* outLeft = static_cast<float*>(output.mBuffers[0].mData) + offset;
            const float* inRight = channels > 1 ? static_cast<const float*>(input.mBuffers[1].mData) + offset : nullptr;
            float* outRight = channels > 1 ? static_cast<float*>(output.mBuffers[1].mData) + offset : nullptr;
            if (!op_shell_process(engine_, inLeft, inRight, outLeft, outRight, count)) return kAudioUnitErr_FailedInitialization;
            offset += count;
        }
        flags &= ~kAudioUnitRenderAction_OutputIsSilence;
        return noErr;
    }

private:
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    void* engine_ = nullptr;
    UInt32 maxFrames_ = 512;
    std::vector<CachedParam> params_;
};
AUSDK_COMPONENT_ENTRY(ausdk::AUBaseFactory, OpenPluginAU)
}
