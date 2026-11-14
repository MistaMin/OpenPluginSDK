// SPDX-License-Identifier: MIT
// Thin Audio Unit adapter over op_shell_*. With OP_HAS_EDITOR it publishes a
// Cocoa view (OpenPluginAUView.mm); without it a host shows its generic view.
#include "op_shell.h"
#include "OpenPluginAUBridge.h"
#include <mutex>
#include <AudioUnitSDK/AUEffectBase.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <string>
#include <vector>

namespace {
#if OP_HAS_EDITOR
// Instances a view may still talk to. A view can outlive its Audio Unit.
std::mutex gLiveLock;
std::vector<void*> gLive;
#endif
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
#if OP_HAS_EDITOR
        std::lock_guard<std::mutex> lock(gLiveLock);
        gLive.push_back(this);
#endif
    }
    ~OpenPluginAU() override {
#if OP_HAS_EDITOR
        {
            std::lock_guard<std::mutex> lock(gLiveLock);
            gLive.erase(std::remove(gLive.begin(), gLive.end(), static_cast<void*>(this)), gLive.end());
        }
#endif
        destroy();
    }
#if OP_HAS_EDITOR
    OSStatus GetPropertyInfo(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, UInt32& size, bool& writable) override {
        if (scope == kAudioUnitScope_Global && id == kAudioUnitProperty_CocoaUI) {
            size = sizeof(AudioUnitCocoaViewInfo);
            writable = false;
            return noErr;
        }
        if (scope == kAudioUnitScope_Global && id == kOpenPluginAUProperty_Instance) {
            size = sizeof(void*);
            writable = false;
            return noErr;
        }
        return AUEffectBase::GetPropertyInfo(id, scope, element, size, writable);
    }
    OSStatus GetProperty(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, void* data) override {
        if (scope == kAudioUnitScope_Global && id == kAudioUnitProperty_CocoaUI) {
            CFBundleRef bundle = CFBundleGetBundleWithIdentifier(CFSTR(OP_AU_BUNDLE_ID));
            if (!bundle) return kAudioUnitErr_InvalidProperty;
            auto* info = static_cast<AudioUnitCocoaViewInfo*>(data);
            // The host releases both.
            info->mCocoaAUViewBundleLocation = CFBundleCopyBundleURL(bundle);
            info->mCocoaAUViewClass[0] = CFStringCreateWithCString(nullptr, OP_AU_VIEW_CLASS_NAME, kCFStringEncodingUTF8);
            return noErr;
        }
        if (scope == kAudioUnitScope_Global && id == kOpenPluginAUProperty_Instance) {
            *static_cast<void**>(data) = this;
            return noErr;
        }
        return AUEffectBase::GetProperty(id, scope, element, data);
    }
    void* engine() const { return engine_; }
    uint32_t paramCount() const { return static_cast<uint32_t>(params_.size()); }
    float paramValue(uint32_t index) { return GetParameter(index); }
#endif
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
        if (!engine_) {
            lastError_.store(kAudioUnitErr_FormatNotSupported, std::memory_order_relaxed);
            return kAudioUnitErr_FormatNotSupported;
        }
        op_shell_reset(engine_);
        resetPending_.store(false, std::memory_order_relaxed);
        return noErr;
    }
    void Cleanup() override {
        destroy();
        AUEffectBase::Cleanup();
    }
    OSStatus Reset(AudioUnitScope scope, AudioUnitElement element) override {
        // A host may call this from its main thread while the render thread is
        // inside the engine, and op_shell_reset is exclusive with process. The
        // render thread does the reset at the start of its next block.
        resetPending_.store(true, std::memory_order_release);
        return AUEffectBase::Reset(scope, element);
    }
    Float64 GetLatency() override {
        const double rate = GetSampleRate();
        if (!engine_ || rate <= 0.0) return 0.0;
        return static_cast<Float64>(op_shell_latency(engine_)) / rate;
    }
#if OP_HAS_TAIL
    bool SupportsTail() AUSDK_RTSAFE override { return true; }
    Float64 GetTailTime() AUSDK_RTSAFE override {
        if (!engine_) return 0.0;
        // An endless tail is reported as ten minutes; the property has no value for "forever".
        const double seconds = op_shell_tail_seconds(engine_);
        return seconds < 600.0 ? std::max(seconds, 0.0) : 600.0;
    }
#endif
    OSStatus GetParameterInfo(AudioUnitScope scope, AudioUnitParameterID id, AudioUnitParameterInfo& info) override {
        if (scope != kAudioUnitScope_Global || id >= params_.size()) return kAudioUnitErr_InvalidParameter;
        std::memset(&info, 0, sizeof(info));
        const OpParamDesc& desc = params_[id].desc;
        // The host releases the CFString.
        FillInParameterName(info, CFStringCreateWithCString(nullptr, desc.name, kCFStringEncodingUTF8), true);
        info.minValue = static_cast<float>(desc.min);
        info.maxValue = static_cast<float>(desc.max);
        info.defaultValue = static_cast<float>(desc.default_plain);
        info.flags |= kAudioUnitParameterFlag_IsReadable | kAudioUnitParameterFlag_IsWritable;
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
        const OSStatus status = render(flags, input, output, frames);
        // A host renders silence for a failed block and says nothing. Keep the
        // status where the view, or a debugger, can see it.
        if (status != noErr) lastError_.store(status, std::memory_order_relaxed);
        return status;
    }
#if OP_HAS_EDITOR
    OSStatus takeLastError() { return lastError_.exchange(noErr, std::memory_order_relaxed); }
#endif

private:
    OSStatus render(AudioUnitRenderActionFlags& flags, const AudioBufferList& input, AudioBufferList& output, UInt32 frames) {
        if (!engine_ || frames == 0) return kAudioUnitErr_Uninitialized;
        if (resetPending_.exchange(false, std::memory_order_acquire)) op_shell_reset(engine_);
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
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    void* engine_ = nullptr;
    std::atomic<bool> resetPending_{false};
    std::atomic<OSStatus> lastError_{noErr};
    UInt32 maxFrames_ = 512;
    std::vector<CachedParam> params_;
};
AUSDK_COMPONENT_ENTRY(ausdk::AUBaseFactory, OpenPluginAU)
}

#if OP_HAS_EDITOR
// The view's side of the instance. Each call checks the instance is still alive.
namespace {
OpenPluginAU* live(void* instance) {
    return std::find(gLive.begin(), gLive.end(), instance) != gLive.end() ? static_cast<OpenPluginAU*>(instance) : nullptr;
}
}
void* op_au_engine(void* instance) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    return unit ? unit->engine() : nullptr;
}
uint32_t op_au_param_count(void* instance) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    return unit ? unit->paramCount() : 0;
}
int32_t op_au_take_error(void* instance) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    return unit ? static_cast<int32_t>(unit->takeLastError()) : 0;
}
uint32_t op_au_param(void* instance, uint32_t index, float* value) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    if (!unit || !value || index >= unit->paramCount()) return 0;
    *value = unit->paramValue(index);
    return 1;
}
#endif
