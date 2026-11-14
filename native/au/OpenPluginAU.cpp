// SPDX-License-Identifier: MIT
// Thin Audio Unit adapter over op_shell_*. With OP_HAS_EDITOR it publishes a
// Cocoa view (OpenPluginAUView.mm); without it a host shows its generic view.
#include "op_shell.h"
#include "OpenPluginAUBridge.h"
#include <memory>
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
// The shell's own key in the kAudioUnitProperty_ClassInfo dictionary. The value is the
// opaque blob of op_shell_save_state, as CFData. The base class's keys stay as they are.
#define OP_AU_STATE_KEY CFSTR("OpenPluginState")
struct EngineDeleter {
    void operator()(void* engine) const { op_shell_destroy(engine); }
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
        if (status != noErr) {
            // The base class refused the stream formats. The unit stays uninitialized and silent.
            lastError_.store(status, std::memory_order_relaxed);
            return status;
        }
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
        // A state restored while uninitialized (or kept over Uninitialize) goes in now. It was
        // checked when it arrived, so a refusal here is not expected. The parameters need no
        // sync: the unit's values are already the blob's, and the render thread pushes them.
        if (!pending_.empty()) {
            op_shell_load_state(engine_, pending_.data(), static_cast<uint32_t>(pending_.size()));
            pending_.clear();
        }
        // Bypass state is sized here, on the message thread, so the render thread never allocates.
        latency_ = op_shell_latency(engine_);
        delayPos_ = 0;
        delay_.assign(static_cast<size_t>(channels) * latency_, 0.0f);
        dry_.assign(static_cast<size_t>(channels) * frames, 0.0f);
        mix_ = bypass_.load(std::memory_order_relaxed) ? 0.0f : 1.0f;
        mixStep_ = static_cast<float>(1.0 / std::max(1.0, kCrossfadeSeconds * rate));
        return noErr;
    }
    void Cleanup() override {
        // Uninitialize drops the engine; keep its state for the next Initialize. Main thread,
        // the host has stopped rendering.
        if (engine_) {
            std::vector<uint8_t> blob;
            if (saveBlob(engine_, blob)) pending_ = std::move(blob);
        }
        destroy();
        AUEffectBase::Cleanup();
    }
    // Bypass is the shell's, not the base class's. AUEffectBase would copy the input to the
    // output with no delay, and reset the plugin when bypass ends. The shell instead keeps
    // running the plugin, delays the dry signal by the reported latency, and crossfades.
    bool ShouldBypassEffect() AUSDK_RTSAFE override { return false; }
    void SetBypassEffect(bool flag) override {
        AUEffectBase::SetBypassEffect(flag);
        bypass_.store(flag, std::memory_order_relaxed);
    }
    OSStatus SetProperty(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, const void* data, UInt32 size) override {
        if (scope == kAudioUnitScope_Global && id == kAudioUnitProperty_BypassEffect) {
            if (size < sizeof(UInt32) || !data) return kAudioUnitErr_InvalidPropertyValue;
            SetBypassEffect(*static_cast<const UInt32*>(data) != 0);
            return noErr;
        }
        return AUEffectBase::SetProperty(id, scope, element, data, size);
    }
    // kAudioUnitProperty_ClassInfo: the base dictionary plus the plugin's whole state under
    // OP_AU_STATE_KEY. Without an engine the state held since the last restore is written.
    // If the plugin cannot save, the key is left out and the parameters still save.
    OSStatus SaveState(CFPropertyListRef* out) override {
        const OSStatus status = AUEffectBase::SaveState(out);
        if (status != noErr) return status;
        std::vector<uint8_t> blob;
        bool have = false;
        if (engine_) have = saveBlob(engine_, blob);
        else if (!pending_.empty()) {
            blob = pending_;
            have = true;
        }
        if (have) {
            CFDataRef data = CFDataCreate(nullptr, blob.data(), static_cast<CFIndex>(blob.size()));
            if (data) {
                CFDictionarySetValue(reinterpret_cast<CFMutableDictionaryRef>(const_cast<void*>(*out)), OP_AU_STATE_KEY, data);
                CFRelease(data);
            }
        }
        return noErr;
    }
    // Key present: the blob goes through op_shell_load_state first, and a refused blob
    // refuses the whole dictionary (kAudioUnitErr_InvalidPropertyValue) and changes nothing,
    // the base dictionary's parameters included. Accepted, the base keys are restored and
    // then the parameters are read back from the engine, so the blob wins over the base
    // dictionary. If the base class then fails, the previous state is put back.
    // Key absent (an old preset): the base restore only; the plugin's chunk is left alone.
    // Without an engine the blob is checked on a scratch engine, held until Initialize.
    OSStatus RestoreState(CFPropertyListRef plist) override {
        if (!plist || CFGetTypeID(plist) != CFDictionaryGetTypeID()) return AUEffectBase::RestoreState(plist);
        const void* value = CFDictionaryGetValue(static_cast<CFDictionaryRef>(plist), OP_AU_STATE_KEY);
        if (!value) return AUEffectBase::RestoreState(plist);
        if (CFGetTypeID(value) != CFDataGetTypeID()) return kAudioUnitErr_InvalidPropertyValue;
        const auto data = static_cast<CFDataRef>(value);
        const CFIndex length = CFDataGetLength(data);
        if (length <= 0 || static_cast<unsigned long long>(length) > 0xFFFFFFFFull) return kAudioUnitErr_InvalidPropertyValue;
        const uint8_t* bytes = CFDataGetBytePtr(data);
        const auto size = static_cast<uint32_t>(length);
        if (engine_) {
            std::vector<uint8_t> before;
            const bool haveBefore = saveBlob(engine_, before);
            if (!op_shell_load_state(engine_, bytes, size)) return kAudioUnitErr_InvalidPropertyValue;
            const OSStatus status = AUEffectBase::RestoreState(plist);
            if (status != noErr && haveBefore) op_shell_load_state(engine_, before.data(), static_cast<uint32_t>(before.size()));
            syncParameters(engine_);
            return status;
        }
        const double rate = GetSampleRate();
        std::unique_ptr<void, EngineDeleter> scratch(op_shell_create(rate > 0.0 ? rate : 44100.0, 512, 2));
        if (!scratch || !op_shell_load_state(scratch.get(), bytes, size)) return kAudioUnitErr_InvalidPropertyValue;
        const OSStatus status = AUEffectBase::RestoreState(plist);
        if (status != noErr) return status;
        syncParameters(scratch.get());
        pending_.assign(bytes, bytes + size);
        return noErr;
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
    // Covers what happens before ProcessBufferLists: no input connection, a failed input pull.
    OSStatus Render(AudioUnitRenderActionFlags& flags, const AudioTimeStamp& time, UInt32 frames) AUSDK_RTSAFE override {
        const OSStatus status = AUEffectBase::Render(flags, time, frames);
        if (status != noErr) lastError_.store(status, std::memory_order_relaxed);
        return status;
    }
    OSStatus ProcessBufferLists(AudioUnitRenderActionFlags& flags, const AudioBufferList& input, AudioBufferList& output, UInt32 frames) override {
        const OSStatus status = render(flags, input, output, frames);
        // A host renders silence for a failed block and says nothing. Keep the
        // status where the view, or a debugger, can see it.
        if (status != noErr) lastError_.store(status, std::memory_order_relaxed);
        return status;
    }
#if OP_HAS_EDITOR
    OSStatus takeLastError() {
        // The base class keeps the first error of any render call, including the ones
        // it rejects before Render: an uninitialized unit, or more frames than the maximum.
        OSStatus base = noErr;
        if (DispatchGetProperty(kAudioUnitProperty_LastRenderError, kAudioUnitScope_Global, 0, &base) != noErr) base = noErr;
        const OSStatus own = lastError_.exchange(noErr, std::memory_order_relaxed);
        return own != noErr ? own : base;
    }
#endif

private:
    OSStatus render(AudioUnitRenderActionFlags& flags, const AudioBufferList& input, AudioBufferList& output, UInt32 frames) {
        if (!engine_ || frames == 0) return kAudioUnitErr_Uninitialized;
        if (resetPending_.exchange(false, std::memory_order_acquire)) {
            op_shell_reset(engine_);
            std::fill(delay_.begin(), delay_.end(), 0.0f);
            delayPos_ = 0;
        }
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
            const float target = bypass_.load(std::memory_order_relaxed) ? 0.0f : 1.0f;
            const bool settledWet = mix_ == 1.0f && target == 1.0f && latency_ == 0;
            // The dry copy is taken before the plugin runs: the host may pass the same buffer for both.
            if (!settledWet) delayDry(channels, inLeft, inRight, count);
            if (!op_shell_process(engine_, inLeft, inRight, outLeft, outRight, count)) return kAudioUnitErr_FailedInitialization;
            if (!settledWet) crossfade(channels, target, outLeft, outRight, count);
            offset += count;
        }
        flags &= ~kAudioUnitRenderAction_OutputIsSilence;
        return noErr;
    }
    // dry_[ch] gets the input delayed by the reported latency. The ring holds the last
    // latency_ samples of each channel and is written on every block, bypassed or not,
    // so it is already full when bypass starts.
    void delayDry(UInt32 channels, const float* inLeft, const float* inRight, uint32_t count) {
        for (UInt32 ch = 0; ch < channels; ++ch) {
            const float* in = ch == 0 ? inLeft : inRight;
            float* dry = dry_.data() + static_cast<size_t>(ch) * maxFrames_;
            if (latency_ == 0) {
                std::memcpy(dry, in, count * sizeof(float));
                continue;
            }
            float* ring = delay_.data() + static_cast<size_t>(ch) * latency_;
            size_t pos = delayPos_;
            for (uint32_t i = 0; i < count; ++i) {
                dry[i] = ring[pos];
                ring[pos] = in[i];
                if (++pos == latency_) pos = 0;
            }
        }
        if (latency_ > 0) delayPos_ = (delayPos_ + count) % latency_;
    }
    // Moves mix_ (0 dry, 1 plugin) toward the target over about 10 ms and writes the blend.
    // Settled bypass is the delayed input exactly; settled processing leaves the output alone.
    void crossfade(UInt32 channels, float target, float* outLeft, float* outRight, uint32_t count) {
        if (mix_ == target && target == 1.0f) return;
        for (UInt32 ch = 0; ch < channels; ++ch) {
            if (mix_ == target) std::memcpy(ch == 0 ? outLeft : outRight, dry_.data() + static_cast<size_t>(ch) * maxFrames_, count * sizeof(float));
        }
        if (mix_ == target) return;
        float mix = mix_;
        for (uint32_t i = 0; i < count; ++i) {
            if (mix < target) mix = std::min(target, mix + mixStep_);
            else mix = std::max(target, mix - mixStep_);
            for (UInt32 ch = 0; ch < channels; ++ch) {
                float* out = ch == 0 ? outLeft : outRight;
                const float dry = dry_[static_cast<size_t>(ch) * maxFrames_ + i];
                out[i] = mix == 0.0f ? dry : out[i] * mix + dry * (1.0f - mix);
            }
        }
        mix_ = mix;
    }
    // The engine's whole state, by the size-query protocol: ask, allocate, copy, and ask again
    // while the state grew in between. Main thread only.
    // The unit's parameter values are the truth, and the engine only sees them at the next
    // block, so they are stored first (set_plain is an atomic store, as the render thread's).
    bool saveBlob(void* engine, std::vector<uint8_t>& blob) {
        for (uint32_t i = 0; i < params_.size(); ++i) {
            const float plain = GetParameter(i);
            if (std::isfinite(plain)) op_shell_set_plain(engine, i, plain);
        }
        uint32_t needed = 0;
        uint32_t result = op_shell_save_state(engine, nullptr, 0, &needed);
        for (int attempt = 0; attempt < 8 && result == OP_STATE_TOO_SMALL; ++attempt) {
            blob.assign(needed, 0);
            result = op_shell_save_state(engine, blob.data(), needed, &needed);
        }
        if (result != OP_STATE_OK) return false;
        blob.resize(needed);
        return true;
    }
    // Makes the unit's parameter values the engine's. The render thread pushes the unit's
    // values into the engine every block, so after a restore they must agree.
    void syncParameters(void* engine) {
        for (uint32_t i = 0; i < params_.size(); ++i) Globals()->SetParameter(i, op_shell_get_plain(engine, i));
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
    static constexpr double kCrossfadeSeconds = 0.010;
    std::atomic<bool> bypass_{false};
    uint32_t latency_ = 0;
    size_t delayPos_ = 0;
    float mix_ = 1.0f;
    float mixStep_ = 1.0f;
    std::vector<float> delay_;
    std::vector<float> dry_;
    std::vector<CachedParam> params_;
    // A blob accepted while there was no engine, loaded at the next Initialize.
    std::vector<uint8_t> pending_;
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
