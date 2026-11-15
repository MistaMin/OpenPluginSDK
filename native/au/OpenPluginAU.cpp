// SPDX-License-Identifier: MIT
// Thin Audio Unit adapter over op_shell_* (shell ABI 2). One source builds the three
// component types, chosen at build time by OP_AU_TYPE_CODE (0 aufx effect, 1 aumu
// instrument, 2 aumf music effect). With OP_HAS_EDITOR it publishes a Cocoa view
// (OpenPluginAUView.mm); without it a host shows its generic view.
//
// What the host sends is turned into one op_shell_process_events call per block:
//   - audio: planar pointer arrays, reordered from the host's channel layout to the
//     SDK's canonical order (au_layout.h);
//   - MIDI and scheduled parameters: sample-offset events;
//   - transport: from HostCallbackInfo;
//   - sidechain: the input element after the main one.
// and the plugin's MIDI output goes to kAudioUnitProperty_MIDIOutputCallback.
#include "op_shell.h"
#include "OpenPluginAUBridge.h"
#include "au_layout.h"
#include <memory>
#include <mutex>
#include <AudioUnitSDK/AUEffectBase.h>
#include <AudioUnitSDK/AUMIDIEffectBase.h>
#include <AudioUnitSDK/AUPlugInDispatch.h>
#include <AudioUnitSDK/MusicDeviceBase.h>
#include <CoreMIDI/CoreMIDI.h>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <string>
#include <type_traits>
#include <vector>

#ifndef OP_AU_TYPE_CODE
#define OP_AU_TYPE_CODE 0
#endif

namespace {
#if OP_HAS_EDITOR
// Instances a view may still talk to. A view can outlive its Audio Unit.
std::mutex gLiveLock;
std::vector<void*> gLive;
#endif
struct CachedParam {
    OpParamDesc desc;
    // The plugin's labels at each step, for a choice parameter (empty for every other one).
    std::vector<std::string> labels;
};
// The plugin's text for a plain value (op_shell_param_format). Main thread only; it allocates.
std::string shellText(uint32_t index, double plain) {
    char small[128];
    const size_t length = op_shell_param_format(index, plain, small, sizeof(small));
    if (length < sizeof(small)) return std::string(small, length);
    std::string text(length + 1, '\0');
    const size_t again = op_shell_param_format(index, plain, text.data(), text.size());
    text.resize(std::min(again, length));
    return text;
}
bool isNumberText(const std::string& text) {
    char* end = nullptr;
    std::strtod(text.c_str(), &end);
    return end != text.c_str() && *end == '\0';
}
// A choice parameter has whole steps from min to max, no unit, and labels that are words. The ABI
// has no "choice" flag, so a stepped number (labels like "3") stays an ordinary stepped parameter.
std::vector<std::string> choiceLabels(uint32_t index, const OpParamDesc& desc) {
    std::vector<std::string> labels;
    if (!(desc.flags & OP_PARAM_DISCRETE) || desc.unit[0] != '\0' || desc.steps < 3 || desc.steps > 256) return labels;
    if (desc.max - desc.min != static_cast<double>(desc.steps - 1) || desc.min != std::floor(desc.min)) return labels;
    bool allNumbers = true;
    for (uint32_t step = 0; step < desc.steps; ++step) {
        labels.push_back(shellText(index, desc.min + step));
        if (!isNumberText(labels.back())) allNumbers = false;
    }
    if (allNumbers) labels.clear();
    return labels;
}
// The shell's own key in the kAudioUnitProperty_ClassInfo dictionary. The value is the
// opaque blob of op_shell_save_state, as CFData. The base class's keys stay as they are.
#define OP_AU_STATE_KEY CFSTR("OpenPluginState")
struct EngineDeleter {
    void operator()(void* engine) const { op_shell_destroy(engine); }
};

// The three component types differ in their SDK base class only.
struct FxBase : ausdk::AUEffectBase {
    explicit FxBase(AudioComponentInstance instance) : AUEffectBase(instance, false) {}
};
struct MfBase : ausdk::AUMIDIEffectBase {
    explicit MfBase(AudioComponentInstance instance) : AUMIDIEffectBase(instance, false) {}
};
struct MuBase : ausdk::MusicDeviceBase {
    explicit MuBase(AudioComponentInstance instance) : MusicDeviceBase(instance, 0, 1) {}
};

// Events that arrive from any thread (MusicDeviceMIDIEvent, scheduled parameters) wait
// here for the next render. A spin lock held for one copy, no allocation.
class EventQueue {
public:
    bool push(const OpEvent& event) {
        Hold hold(flag_);
        if (count_ >= OP_MAX_EVENTS) return false;
        events_[count_++] = event;
        return true;
    }
    uint32_t drain(OpEvent* out) {
        Hold hold(flag_);
        const uint32_t count = count_;
        std::memcpy(out, events_, count * sizeof(OpEvent));
        count_ = 0;
        return count;
    }
    void clear() {
        Hold hold(flag_);
        count_ = 0;
    }

private:
    struct Hold {
        explicit Hold(std::atomic_flag& flag) : flag_(flag) {
            while (flag_.test_and_set(std::memory_order_acquire)) {
            }
        }
        ~Hold() { flag_.clear(std::memory_order_release); }
        std::atomic_flag& flag_;
    };
    std::atomic_flag flag_;
    uint32_t count_ = 0;
    OpEvent events_[OP_MAX_EVENTS];
};

struct MidiOutTarget {
    AUMIDIOutputCallback proc;
    void* userData;
};

// How a block's channels are laid out, decided at Initialize.
struct Plan {
    uint32_t layout = 0;
    uint32_t inChannels = 0;
    uint32_t outChannels = 0;
    uint32_t scChannels = 0;
    // perm[j]: the canonical index of the host's channel j.
    uint8_t inPerm[OP_MAX_CHANNELS] = {};
    uint8_t outPerm[OP_MAX_CHANNELS] = {};
    uint8_t scPerm[OP_MAX_CHANNELS] = {};
};

// Ramps are cut into steps this long (frames). A step is a parameter event at its offset.
constexpr uint32_t kRampStep = 16;

template <class Base, int kType>
class OpenPluginAUImpl final : public Base {
public:
    explicit OpenPluginAUImpl(AudioComponentInstance instance) : Base(instance) {
        this->CreateElements();
        OpShellInfo info{};
        // The library must be at least the ABI this shell was written for; a newer one only adds.
        abiOk_ = op_shell_abi_version() >= OP_SHELL_ABI && op_shell_info(&info) && info.abi >= OP_SHELL_ABI;
        if (abiOk_) {
            params_.resize(info.param_count);
            this->Globals()->UseIndexedParameters(info.param_count);
            for (uint32_t i = 0; i < info.param_count; ++i) {
                if (!op_shell_param(i, &params_[i].desc)) continue;
                this->Globals()->SetParameter(i, static_cast<float>(params_[i].desc.default_plain));
                params_[i].labels = choiceLabels(i, params_[i].desc);
            }
            const uint32_t kind = op_shell_kind();
            // aumu is the instrument; aufx and aumf are both audio effects (an aumf also takes MIDI).
            kindOk_ = (kType == 1) == (kind == OP_KIND_INSTRUMENT);
            hasMainIn_ = kind != OP_KIND_INSTRUMENT;
            hasMidiOut_ = (op_shell_note_ports() & OP_NOTE_PORT_OUTPUT) != 0;
            const uint32_t count = op_shell_layout_count();
            for (uint32_t i = 0; i < count; ++i) {
                OpLayout layout{};
                if (op_shell_layout(i, &layout)) layouts_.push_back(layout);
            }
            for (const OpLayout& layout : layouts_) {
                if (layout.sidechain.kind != OP_BUS_ABSENT && layout.sidechain.channels > 0) hasSidechain_ = true;
                const AUChannelInfo pair{static_cast<SInt16>(hasMainIn_ ? layout.input.channels : 0), static_cast<SInt16>(layout.output.channels)};
                bool known = false;
                for (const AUChannelInfo& have : channelInfo_) known = known || (have.inChannels == pair.inChannels && have.outChannels == pair.outChannels);
                if (!known) channelInfo_.push_back(pair);
            }
            if (layouts_.empty()) {
                // A library with no layouts is a mono and stereo plugin with equal buses.
                if (hasMainIn_) channelInfo_ = {{1, 1}, {2, 2}};
                else channelInfo_ = {{0, 1}, {0, 2}};
            } else {
                // Mono has always worked: one channel in and out, which the engine widens to its bus.
                // A plugin that lists only wider layouts keeps it (see makePlan).
                const AUChannelInfo mono{static_cast<SInt16>(hasMainIn_ ? 1 : 0), 1};
                bool known = false;
                for (const AUChannelInfo& have : channelInfo_) known = known || (have.inChannels == mono.inChannels && have.outChannels == mono.outChannels);
                if (!known) channelInfo_.push_back(mono);
            }
        }
        scIndex_ = hasMainIn_ ? 1 : 0;
        this->Inputs().SetNumberOfElements(hasMainIn_ ? 1 : 0);
        if (abiOk_) startPolling();
#if OP_HAS_EDITOR
        std::lock_guard<std::mutex> lock(gLiveLock);
        gLive.push_back(this);
#endif
    }
    ~OpenPluginAUImpl() override {
        stopPolling();
#if OP_HAS_EDITOR
        {
            std::lock_guard<std::mutex> lock(gLiveLock);
            gLive.erase(std::remove(gLive.begin(), gLive.end(), static_cast<void*>(this)), gLive.end());
        }
#endif
        destroy();
    }

    // ---- properties --------------------------------------------------------------------
    OSStatus GetPropertyInfo(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, UInt32& size, bool& writable) override {
#if OP_HAS_EDITOR
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
#endif
        // The plugin's own parameter text; the parameter id travels in the struct.
        if (id == kAudioUnitProperty_ParameterStringFromValue) {
            size = sizeof(AudioUnitParameterStringFromValue);
            writable = false;
            return noErr;
        }
        if (id == kAudioUnitProperty_ParameterValueFromString) {
            size = sizeof(AudioUnitParameterValueFromString);
            writable = false;
            return noErr;
        }
        if (scope == kAudioUnitScope_Global) {
            if (id == kAudioUnitProperty_OfflineRender) {
                size = sizeof(UInt32);
                writable = true;
                return noErr;
            }
            if (hasMidiOut_ && id == kAudioUnitProperty_MIDIOutputCallbackInfo) {
                size = sizeof(CFArrayRef);
                writable = false;
                return noErr;
            }
            if (hasMidiOut_ && id == kAudioUnitProperty_MIDIOutputCallback) {
                size = sizeof(AUMIDIOutputCallbackStruct);
                writable = true;
                return noErr;
            }
        }
        return Base::GetPropertyInfo(id, scope, element, size, writable);
    }
    OSStatus GetProperty(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, void* data) override {
#if OP_HAS_EDITOR
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
#endif
        if (id == kAudioUnitProperty_ParameterStringFromValue) {
            auto* request = static_cast<AudioUnitParameterStringFromValue*>(data);
            if (request->inParamID >= params_.size()) return kAudioUnitErr_InvalidParameter;
            const double plain = request->inValue ? static_cast<double>(*request->inValue) : static_cast<double>(this->Globals()->GetParameter(request->inParamID));
            if (!std::isfinite(plain)) return kAudioUnitErr_InvalidParameterValue;
            const std::string text = shellText(request->inParamID, plain);
            // The host releases the string.
            request->outString = CFStringCreateWithBytes(nullptr, reinterpret_cast<const UInt8*>(text.data()), static_cast<CFIndex>(text.size()), kCFStringEncodingUTF8, false);
            return request->outString ? OSStatus(noErr) : OSStatus(kAudioUnitErr_InvalidPropertyValue);
        }
        if (id == kAudioUnitProperty_ParameterValueFromString) {
            auto* request = static_cast<AudioUnitParameterValueFromString*>(data);
            if (request->inParamID >= params_.size() || !request->inString) return kAudioUnitErr_InvalidParameter;
            const CFIndex room = CFStringGetMaximumSizeForEncoding(CFStringGetLength(request->inString), kCFStringEncodingUTF8) + 1;
            if (room > 4096) return kAudioUnitErr_InvalidPropertyValue;
            std::string text(static_cast<size_t>(room), '\0');
            if (!CFStringGetCString(request->inString, text.data(), room, kCFStringEncodingUTF8)) return kAudioUnitErr_InvalidPropertyValue;
            double plain = 0.0;
            if (!op_shell_param_parse(request->inParamID, text.c_str(), &plain) || !std::isfinite(plain)) return kAudioUnitErr_InvalidPropertyValue;
            request->outValue = static_cast<AudioUnitParameterValue>(plain);
            return noErr;
        }
        if (scope == kAudioUnitScope_Global) {
            if (id == kAudioUnitProperty_OfflineRender) {
                *static_cast<UInt32*>(data) = offline_.load(std::memory_order_relaxed) ? 1 : 0;
                return noErr;
            }
            if (hasMidiOut_ && id == kAudioUnitProperty_MIDIOutputCallbackInfo) {
                // One MIDI output stream. The host releases the array and its strings.
                CFStringRef name = CFSTR("MIDI Out");
                *static_cast<CFArrayRef*>(data) = CFArrayCreate(nullptr, reinterpret_cast<const void**>(&name), 1, &kCFTypeArrayCallBacks);
                return noErr;
            }
        }
        return Base::GetProperty(id, scope, element, data);
    }
    OSStatus SetProperty(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element, const void* data, UInt32 size) override {
        if (scope == kAudioUnitScope_Global) {
            if (id == kAudioUnitProperty_BypassEffect && hasMainIn_) {
                if (size < sizeof(UInt32) || !data) return kAudioUnitErr_InvalidPropertyValue;
                setBypass(*static_cast<const UInt32*>(data) != 0);
                return noErr;
            }
            if (id == kAudioUnitProperty_OfflineRender) {
                if (size < sizeof(UInt32) || !data) return kAudioUnitErr_InvalidPropertyValue;
                const bool on = *static_cast<const UInt32*>(data) != 0;
                offline_.store(on, std::memory_order_relaxed);
                if (engine_) op_shell_set_offline(engine_, on ? 1 : 0);
                return noErr;
            }
            if (hasMidiOut_ && id == kAudioUnitProperty_MIDIOutputCallback) {
                if (size < sizeof(AUMIDIOutputCallbackStruct) || !data) return kAudioUnitErr_InvalidPropertyValue;
                const auto* callback = static_cast<const AUMIDIOutputCallbackStruct*>(data);
                // Published as a whole, so the render thread never sees a proc with another unit's data.
                // Replaced targets are kept until the unit is destroyed: a set is rare and a render may hold one.
                auto target = std::make_unique<MidiOutTarget>(MidiOutTarget{callback->midiOutputCallback, callback->userData});
                midiOut_.store(callback->midiOutputCallback ? target.get() : nullptr, std::memory_order_release);
                midiTargets_.push_back(std::move(target));
                return noErr;
            }
        }
        return Base::SetProperty(id, scope, element, data, size);
    }
#if OP_HAS_EDITOR
    void* engine() const { return engine_; }
    uint32_t paramCount() const { return static_cast<uint32_t>(params_.size()); }
    float paramValue(uint32_t index) { return this->Globals()->GetParameter(index); }
#endif

    // ---- buses and channels ------------------------------------------------------------
    // Formats change only while uninitialized (AUEffectBase says the same; MusicDeviceBase leaves it to us).
    bool StreamFormatWritable(AudioUnitScope, AudioUnitElement) override { return !this->IsInitialized(); }
    // A sidechain is one more input element: element 1 of an effect, element 0 of an instrument.
    bool BusCountWritable(AudioUnitScope scope) override { return scope == kAudioUnitScope_Input && hasSidechain_; }
    OSStatus SetBusCount(AudioUnitScope scope, UInt32 count) override {
        if (scope != kAudioUnitScope_Input || !hasSidechain_ || count > scIndex_ + 1) return kAudioUnitErr_InvalidPropertyValue;
        if (count < scIndex_) return kAudioUnitErr_InvalidPropertyValue;
        return Base::SetBusCount(scope, count);
    }
    UInt32 SupportedNumChannels(const AUChannelInfo** info) override {
        if (info) *info = channelInfo_.data();
        return static_cast<UInt32>(channelInfo_.size());
    }
    // The plugin buses an element can be: those of its layouts with as many channels as the element has.
    std::vector<const OpBus*> busesFor(AudioUnitScope scope, AudioUnitElement element) {
        std::vector<const OpBus*> buses;
        uint32_t channels = 0;
        enum { kIn, kOut, kSc } which;
        if (scope == kAudioUnitScope_Output && element == 0) {
            channels = this->Output(0).GetStreamFormat().mChannelsPerFrame;
            which = kOut;
        } else if (scope == kAudioUnitScope_Input && element < this->Inputs().GetNumberOfElements()) {
            channels = this->Input(element).GetStreamFormat().mChannelsPerFrame;
            which = hasSidechain_ && element == scIndex_ ? kSc : kIn;
            if (!hasMainIn_ && which == kIn) return buses;
        } else {
            return buses;
        }
        for (const OpLayout& layout : layouts_) {
            const OpBus& bus = which == kOut ? layout.output : which == kSc ? layout.sidechain : layout.input;
            if (bus.channels == channels) buses.push_back(&bus);
        }
        return buses;
    }
    // The tags a bus offers.
    std::vector<AudioChannelLayoutTag> GetChannelLayoutTags(AudioUnitScope scope, AudioUnitElement element) override {
        std::vector<AudioChannelLayoutTag> tags;
        for (const OpBus* bus : busesFor(scope, element)) opau::appendTags(*bus, tags);
        return tags;
    }
    // The SDK accepts any layout once the tag list holds UseChannelDescriptions, so the check is ours:
    // a listed tag, or channel descriptions whose labels the plugin's bus has.
    OSStatus SetAudioChannelLayout(AudioUnitScope scope, AudioUnitElement element, const AudioChannelLayout* layout) override {
        const auto buses = busesFor(scope, element);
        if (buses.empty()) return kAudioUnitErr_InvalidProperty;
        bool ok = false;
        if (layout->mChannelLayoutTag == opau::kTagUseDescriptions) {
            uint8_t perm[OP_MAX_CHANNELS];
            for (const OpBus* bus : buses) ok = ok || opau::planBus(*bus, layout, bus->channels, perm);
        } else {
            for (AudioChannelLayoutTag listed : GetChannelLayoutTags(scope, element)) ok = ok || (listed == layout->mChannelLayoutTag && listed != opau::kTagUseDescriptions);
        }
        return ok ? Base::SetAudioChannelLayout(scope, element, layout) : OSStatus(kAudioUnitErr_InvalidPropertyValue);
    }
    // A host that never set a layout gets the bus's preferred tag back.
    UInt32 GetAudioChannelLayout(AudioUnitScope scope, AudioUnitElement element, AudioChannelLayout* out, bool& writable) override {
        const UInt32 size = Base::GetAudioChannelLayout(scope, element, out, writable);
        if (size != 0) return size;
        const auto tags = GetChannelLayoutTags(scope, element);
        if (tags.empty()) return 0;
        writable = true;
        if (out) {
            std::memset(out, 0, sizeof(AudioChannelLayout));
            out->mChannelLayoutTag = tags.front();
        }
        return sizeof(AudioChannelLayout);
    }

    // ---- lifecycle ---------------------------------------------------------------------
    bool CanScheduleParameters() const noexcept override { return true; }
    OSStatus Initialize() override {
        if (!abiOk_ || !kindOk_) {
            // The library is older than this shell needs, or is not the kind of plugin this component type is.
            lastError_.store(kAudioUnitErr_FailedInitialization, std::memory_order_relaxed);
            return kAudioUnitErr_FailedInitialization;
        }
        const OSStatus status = Base::Initialize();
        if (status != noErr) {
            // The base class refused the stream formats. The unit stays uninitialized and silent.
            lastError_.store(status, std::memory_order_relaxed);
            return status;
        }
        Plan plan;
        if (!makePlan(plan)) {
            lastError_.store(kAudioUnitErr_FormatNotSupported, std::memory_order_relaxed);
            return kAudioUnitErr_FormatNotSupported;
        }
        destroy();
        const double rate = this->Output(0).GetStreamFormat().mSampleRate;
        UInt32 frames = this->GetMaxFramesPerSlice();
        if (frames < 1) frames = 512;
        if (frames > 65536) frames = 65536;
        maxFrames_ = frames;
        engine_ = op_shell_create(rate, frames, std::max(plan.inChannels, plan.outChannels) > 1 ? 2 : 1);
        if (!engine_) {
            lastError_.store(kAudioUnitErr_FormatNotSupported, std::memory_order_relaxed);
            return kAudioUnitErr_FormatNotSupported;
        }
        if (!layouts_.empty() && op_shell_layout_selected(engine_) != plan.layout && !op_shell_set_layout(engine_, plan.layout)) {
            destroy();
            lastError_.store(kAudioUnitErr_FormatNotSupported, std::memory_order_relaxed);
            return kAudioUnitErr_FormatNotSupported;
        }
        plan_ = plan;
        op_shell_reset(engine_);
        op_shell_set_offline(engine_, offline_.load(std::memory_order_relaxed) ? 1 : 0);
        resetPending_.store(false, std::memory_order_relaxed);
        events_.clear();
        // A state restored while uninitialized (or kept over Uninitialize) goes in now. It was
        // checked when it arrived, so a refusal here is not expected. The parameters need no
        // sync: the unit's values are already the blob's, and the render thread pushes them.
        if (!pending_.empty()) {
            op_shell_load_state(engine_, pending_.data(), static_cast<uint32_t>(pending_.size()));
            pending_.clear();
        }
        // Everything the render thread uses is sized here, on the message thread, so it never allocates.
        latency_ = op_shell_latency(engine_);
        delayPos_ = 0;
        delay_.clear();
        dry_.clear();
        if (hasMainIn_) {
            delay_.assign(static_cast<size_t>(plan.outChannels) * latency_, 0.0f);
            dry_.assign(static_cast<size_t>(plan.outChannels) * frames, 0.0f);
        }
        mix_ = bypass_.load(std::memory_order_relaxed) ? 0.0f : 1.0f;
        mixStep_ = static_cast<float>(1.0 / std::max(1.0, kCrossfadeSeconds * rate));
        rate_ = rate;
        touched_.assign(params_.size(), 0);
        lastOffset_.assign(params_.size(), 0);
        lastValue_.assign(params_.size(), 0.0f);
        pendingEvents_.resize(OP_MAX_EVENTS);
        chunkEvents_.resize(OP_MAX_EVENTS);
        midiPacketBuffer_.assign(kMidiPacketBytes, 0);
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
        Base::Cleanup();
    }
    // Bypass is the shell's, not the base class's. AUEffectBase would copy the input to the
    // output with no delay, and reset the plugin when bypass ends. The shell instead keeps
    // running the plugin, delays the dry signal by the reported latency, and crossfades.
    // The base class's flag is kept in step so a host reading the property sees it.
    void setBypass(bool flag) {
        if constexpr (std::is_base_of_v<ausdk::AUEffectBase, Base>) Base::SetBypassEffect(flag);
        bypass_.store(flag, std::memory_order_relaxed);
    }
    // kAudioUnitProperty_ClassInfo: the base dictionary plus the plugin's whole state under
    // OP_AU_STATE_KEY. Without an engine the state held since the last restore is written.
    // If the plugin cannot save, the key is left out and the parameters still save.
    OSStatus SaveState(CFPropertyListRef* out) override {
        const OSStatus status = Base::SaveState(out);
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
        if (!plist || CFGetTypeID(plist) != CFDictionaryGetTypeID()) return Base::RestoreState(plist);
        const void* value = CFDictionaryGetValue(static_cast<CFDictionaryRef>(plist), OP_AU_STATE_KEY);
        if (!value) return Base::RestoreState(plist);
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
            const OSStatus status = Base::RestoreState(plist);
            if (status != noErr && haveBefore) op_shell_load_state(engine_, before.data(), static_cast<uint32_t>(before.size()));
            syncParameters(engine_);
            return status;
        }
        const double rate = this->Output(0).GetStreamFormat().mSampleRate;
        std::unique_ptr<void, EngineDeleter> scratch(op_shell_create(rate > 0.0 ? rate : 44100.0, 512, 2));
        if (!scratch || !op_shell_load_state(scratch.get(), bytes, size)) return kAudioUnitErr_InvalidPropertyValue;
        const OSStatus status = Base::RestoreState(plist);
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
        return Base::Reset(scope, element);
    }
    Float64 GetLatency() override {
        const double rate = this->Output(0).GetStreamFormat().mSampleRate;
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

    // ---- parameters --------------------------------------------------------------------
    OSStatus GetParameterInfo(AudioUnitScope scope, AudioUnitParameterID id, AudioUnitParameterInfo& info) override {
        if (scope != kAudioUnitScope_Global || id >= params_.size()) return kAudioUnitErr_InvalidParameter;
        std::memset(&info, 0, sizeof(info));
        const OpParamDesc& desc = params_[id].desc;
        // The host releases the CFString.
        this->FillInParameterName(info, CFStringCreateWithCString(nullptr, desc.name, kCFStringEncodingUTF8), true);
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
        // Hosts ask the plugin for the text of every value (kAudioUnitProperty_ParameterStringFromValue).
        info.flags |= kAudioUnitParameterFlag_ValuesHaveStrings;
        if (!params_[id].labels.empty()) info.unit = kAudioUnitParameterUnit_Indexed;
        return noErr;
    }
    // The labels of a choice parameter, one per step from min (kAudioUnitParameterUnit_Indexed).
    OSStatus GetParameterValueStrings(AudioUnitScope scope, AudioUnitParameterID id, CFArrayRef* strings) override {
        if (scope != kAudioUnitScope_Global || id >= params_.size() || params_[id].labels.empty()) return kAudioUnitErr_InvalidProperty;
        if (!strings) return noErr;
        const auto& labels = params_[id].labels;
        CFMutableArrayRef array = CFArrayCreateMutable(nullptr, static_cast<CFIndex>(labels.size()), &kCFTypeArrayCallBacks);
        if (!array) return kAudio_ParamError;
        for (const std::string& label : labels) {
            CFStringRef text = CFStringCreateWithBytes(nullptr, reinterpret_cast<const UInt8*>(label.data()), static_cast<CFIndex>(label.size()), kCFStringEncodingUTF8, false);
            if (!text) { CFRelease(array); return kAudio_ParamError; }
            CFArrayAppendValue(array, text);
            CFRelease(text);
        }
        *strings = array;  // the host releases it
        return noErr;
    }
    bool validValue(AudioUnitParameterID id, AudioUnitParameterValue value) const {
        if (id >= params_.size() || !std::isfinite(value)) return false;
        const OpParamDesc& desc = params_[id].desc;
        return value >= desc.min - 1e-4 && value <= desc.max + 1e-4;
    }
    // A value with no offset is stored at once and reaches the plugin at the start of the next
    // render. A value with an offset (AudioUnitSetParameter's buffer offset, a scheduled
    // immediate event) waits as a sample-offset event and reaches the plugin at that sample.
    OSStatus SetParameter(AudioUnitParameterID id, AudioUnitScope scope, AudioUnitElement element, AudioUnitParameterValue value, UInt32 offset) override {
        if (scope == kAudioUnitScope_Global && element == 0) {
            if (!validValue(id, value)) return kAudioUnitErr_InvalidParameter;
            if (offset > 0) return queueParameter(id, value, offset) ? OSStatus(noErr) : OSStatus(kAudioUnitErr_InvalidParameter);
        }
        return Base::SetParameter(id, scope, element, value, offset);
    }
    // Immediate events are SetParameter. A ramp becomes a staircase of events kRampStep frames
    // apart that ends on the end value: the plugin sees values, not slopes.
    OSStatus ScheduleParameter(const AudioUnitParameterEvent* events, UInt32 count) override {
        for (UInt32 n = 0; n < count; ++n) {
            const AudioUnitParameterEvent& event = events[n];
            if (event.scope != kAudioUnitScope_Global || event.element != 0) return kAudioUnitErr_InvalidScope;
            if (event.eventType == kParameterEvent_Immediate) {
                const OSStatus status = SetParameter(event.parameter, event.scope, event.element, event.eventValues.immediate.value, event.eventValues.immediate.bufferOffset);
                if (status != noErr) return status;
                continue;
            }
            const auto& ramp = event.eventValues.ramp;
            if (!validValue(event.parameter, ramp.startValue) || !validValue(event.parameter, ramp.endValue)) return kAudioUnitErr_InvalidParameter;
            const int64_t start = ramp.startBufferOffset;
            const int64_t duration = std::max<int64_t>(ramp.durationInFrames, 1);
            const int64_t end = start + duration;
            if (end <= 0) continue;  // over before this render
            int64_t at = std::max<int64_t>(start, 0);
            for (; at < end && at < 65536; at += kRampStep) {
                const double t = static_cast<double>(at - start) / static_cast<double>(duration);
                const float value = static_cast<float>(ramp.startValue + (ramp.endValue - ramp.startValue) * t);
                if (!queueParameter(event.parameter, value, static_cast<uint32_t>(at))) break;
            }
            if (end < 65536) queueParameter(event.parameter, ramp.endValue, static_cast<uint32_t>(end));
        }
        return noErr;
    }

    // ---- MIDI in -----------------------------------------------------------------------
    // MusicDeviceMIDIEvent reaches here with the sample offset into the next render. Without
    // `override`: an effect has no AUMIDIBase and never calls it.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Winconsistent-missing-override"
    OSStatus HandleMIDIEvent(UInt8 status, UInt8 channel, UInt8 data1, UInt8 data2, UInt32 offset) {
        OpEvent event{};
        event.offset = offset;
        event.channel = channel;
        if (status == 0x90 && data2 > 0) {
            event.kind = OP_EVENT_NOTE_ON;
            event.note = data1 & 0x7F;
            event.velocity = data2 & 0x7F;
        } else if (status == 0x80 || status == 0x90) {
            event.kind = OP_EVENT_NOTE_OFF;
            event.note = data1 & 0x7F;
            event.velocity = status == 0x90 ? 0 : (data2 & 0x7F);
        } else {
            event.kind = OP_EVENT_MIDI;
            event.midi[0] = static_cast<uint8_t>(status | channel);
            event.midi[1] = data1;
            event.midi[2] = data2;
        }
        return events_.push(event) ? OSStatus(noErr) : OSStatus(kAudio_ParamError);
    }
#pragma clang diagnostic pop

    // ---- render ------------------------------------------------------------------------
    OSStatus Render(AudioUnitRenderActionFlags& flags, const AudioTimeStamp& time, UInt32 frames) AUSDK_RTSAFE override {
        const OSStatus status = render(flags, time, frames);
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
        if (this->DispatchGetProperty(kAudioUnitProperty_LastRenderError, kAudioUnitScope_Global, 0, &base) != noErr) base = noErr;
        const OSStatus own = lastError_.exchange(noErr, std::memory_order_relaxed);
        return own != noErr ? own : base;
    }
#endif

private:
    static constexpr size_t kMidiPacketBytes = 32768;

    // Picks the plugin layout for the stream formats and layouts the host set, and the
    // channel permutations that go with it. The first layout whose buses fit wins.
    bool makePlan(Plan& plan) {
        const AudioStreamBasicDescription out = this->Output(0).GetStreamFormat();
        plan.outChannels = out.mChannelsPerFrame;
        plan.inChannels = 0;
        if (hasMainIn_) {
            const AudioStreamBasicDescription in = this->Input(0).GetStreamFormat();
            plan.inChannels = in.mChannelsPerFrame;
            if (in.mSampleRate != out.mSampleRate) return false;
        }
        plan.scChannels = 0;
        if (hasSidechain_ && this->Inputs().GetNumberOfElements() > scIndex_ && this->Input(scIndex_).IsActive()) {
            const AudioStreamBasicDescription sc = this->Input(scIndex_).GetStreamFormat();
            if (sc.mSampleRate != out.mSampleRate) return false;
            plan.scChannels = sc.mChannelsPerFrame;
        }
        if (plan.outChannels == 0 || plan.outChannels > OP_MAX_CHANNELS || plan.inChannels > OP_MAX_CHANNELS || plan.scChannels > OP_MAX_CHANNELS) return false;
        if (layouts_.empty()) {
            // Legacy plugin: mono or stereo, input and output alike.
            plan.scChannels = 0;
            if (plan.outChannels > 2 || (hasMainIn_ && plan.inChannels != plan.outChannels)) return false;
            for (uint32_t i = 0; i < OP_MAX_CHANNELS; ++i) plan.inPerm[i] = plan.outPerm[i] = plan.scPerm[i] = static_cast<uint8_t>(i);
            plan.layout = 0;
            return true;
        }
        // The layouts the host set, if any: absent means "none set", and each candidate takes its own default.
        std::vector<uint8_t> inHost, outHost, scHost;
        const bool haveIn = hasMainIn_ && readLayout(kAudioUnitScope_Input, 0, inHost);
        const bool haveOut = readLayout(kAudioUnitScope_Output, 0, outHost);
        const bool haveSc = plan.scChannels > 0 && readLayout(kAudioUnitScope_Input, scIndex_, scHost);
        auto asLayout = [](const std::vector<uint8_t>& bytes, bool have) -> const AudioChannelLayout* { return have ? reinterpret_cast<const AudioChannelLayout*>(bytes.data()) : nullptr; };
        // The first layout whose buses fit wins. First pass: the layout's sidechain is the one
        // connected (none, if none is). If no layout has it, second pass: the unit runs without
        // the sidechain on any layout that fits.
        auto search = [&](bool withSidechain) {
            for (uint32_t index = 0; index < layouts_.size(); ++index) {
                const OpLayout& layout = layouts_[index];
                if (layout.output.channels != plan.outChannels) continue;
                if (hasMainIn_ && layout.input.channels != plan.inChannels) continue;
                if (withSidechain && layout.sidechain.channels != plan.scChannels) continue;
                Plan candidate = plan;
                if (!withSidechain) candidate.scChannels = 0;
                if (!opau::planBus(layout.output, asLayout(outHost, haveOut), plan.outChannels, candidate.outPerm)) continue;
                if (hasMainIn_ && !opau::planBus(layout.input, asLayout(inHost, haveIn), plan.inChannels, candidate.inPerm)) continue;
                if (withSidechain && plan.scChannels > 0 && !opau::planBus(layout.sidechain, asLayout(scHost, haveSc), plan.scChannels, candidate.scPerm)) continue;
                candidate.layout = index;
                plan = candidate;
                return true;
            }
            return false;
        };
        if (search(true) || search(false)) return true;
        // A mono bus on a plugin with no mono layout: the plugin's default layout, one channel in and out.
        if (plan.outChannels == 1 && (!hasMainIn_ || plan.inChannels == 1)) {
            plan.scChannels = 0;
            plan.layout = 0;
            return true;
        }
        return false;
    }
    // The layout the host set on an element, without the default the property synthesises.
    bool readLayout(AudioUnitScope scope, AudioUnitElement element, std::vector<uint8_t>& bytes) {
        bool writable = false;
        const UInt32 size = Base::GetAudioChannelLayout(scope, element, nullptr, writable);
        if (size < offsetof(AudioChannelLayout, mChannelDescriptions)) return false;
        bytes.assign(std::max<size_t>(size, sizeof(AudioChannelLayout)), 0);
        Base::GetAudioChannelLayout(scope, element, reinterpret_cast<AudioChannelLayout*>(bytes.data()), writable);
        return true;
    }

    bool queueParameter(AudioUnitParameterID id, float value, uint32_t offset) {
        OpEvent event{};
        event.offset = offset;
        event.kind = OP_EVENT_PARAM_PLAIN;
        event.index = id;
        event.value = value;
        return events_.push(event);
    }

    // The transport the host reports at the start of this render, or nothing if it reports none.
    bool fillTransport(const AudioTimeStamp& time, OpTransport& t) {
        const HostCallbackInfo& host = this->GetHostCallbackInfo();
        std::memset(&t, 0, sizeof(t));
        bool any = false;
        double samplePos = 0.0;
        bool havePos = false;
        if (host.transportStateProc2 || host.transportStateProc) {
            Boolean playing = false, changed = false, cycling = false;
            Float64 pos = 0.0, cycleStart = 0.0, cycleEnd = 0.0;
            OSStatus status = kAudioUnitErr_InvalidProperty;
            if (host.transportStateProc2) {
                Boolean recording = false;
                status = host.transportStateProc2(host.hostUserData, &playing, &recording, &changed, &pos, &cycling, &cycleStart, &cycleEnd);
                if (status == noErr && recording) t.flags |= OP_TRANSPORT_RECORDING;
            } else {
                status = host.transportStateProc(host.hostUserData, &playing, &changed, &pos, &cycling, &cycleStart, &cycleEnd);
            }
            if (status == noErr) {
                any = true;
                if (playing) t.flags |= OP_TRANSPORT_PLAYING;
                samplePos = pos;
                havePos = true;
            }
        }
        if (!havePos && (time.mFlags & kAudioTimeStampSampleTimeValid)) {
            samplePos = time.mSampleTime;
            havePos = any = true;
        }
        if (havePos) {
            t.sample_pos = static_cast<int64_t>(samplePos);
            if (rate_ > 0.0) {
                t.seconds = samplePos / rate_;
                t.flags |= OP_TRANSPORT_HAS_SECONDS;
            }
        }
        if (host.beatAndTempoProc) {
            Float64 beat = 0.0, tempo = 0.0;
            if (host.beatAndTempoProc(host.hostUserData, &beat, &tempo) == noErr) {
                any = true;
                t.quarter_notes = beat;
                t.flags |= OP_TRANSPORT_HAS_BEATS;
                if (tempo > 0.0) {
                    t.tempo = tempo;
                    t.flags |= OP_TRANSPORT_HAS_TEMPO;
                }
            }
        }
        if (host.musicalTimeLocationProc) {
            UInt32 delta = 0, denominator = 0;
            Float32 numerator = 0.0f;
            Float64 downbeat = 0.0;
            if (host.musicalTimeLocationProc(host.hostUserData, &delta, &numerator, &denominator, &downbeat) == noErr) {
                any = true;
                if (numerator >= 1.0f && denominator > 0) {
                    t.numerator = static_cast<uint32_t>(numerator);
                    t.denominator = denominator;
                    t.flags |= OP_TRANSPORT_HAS_SIGNATURE;
                    // The host reports the bar's downbeat, not its number: count bars from beat 0
                    // at this signature. A project that changes signature has its own numbering.
                    const double barLength = static_cast<double>(t.numerator) * 4.0 / static_cast<double>(denominator);
                    t.bar_number = static_cast<int64_t>(std::floor(downbeat / barLength + 1e-9));
                }
                t.bar_start = downbeat;
                t.flags |= OP_TRANSPORT_HAS_BEATS;
            }
        }
        return any;
    }
    // The transport at `frames` frames into the render: the position and, when the host gave a
    // tempo, the beat advance; the bar start stays the render's.
    static void advance(OpTransport& t, uint32_t frames, double rate) {
        if (frames == 0 || rate <= 0.0) return;
        t.sample_pos += frames;
        if (t.flags & OP_TRANSPORT_HAS_SECONDS) t.seconds += frames / rate;
        if ((t.flags & OP_TRANSPORT_HAS_TEMPO) && (t.flags & OP_TRANSPORT_PLAYING)) t.quarter_notes += frames / rate * t.tempo / 60.0;
    }

    static uint32_t midiLength(uint8_t status) {
        if (status >= 0xF8 || status == 0xF6) return 1;
        if (status == 0xF1 || status == 0xF3 || (status >= 0xC0 && status < 0xE0)) return 2;
        return 3;
    }
    // Hands the plugin's MIDI of the block just processed to the host, offsets counted from the
    // start of the render. Reads it all even without a callback, so nothing is left for a later block.
    void sendMidi(const AudioTimeStamp& time, uint32_t base) {
        const MidiOutTarget* target = midiOut_.load(std::memory_order_acquire);
        if (!target) return;
        auto* list = reinterpret_cast<MIDIPacketList*>(midiPacketBuffer_.data());
        MIDIPacket* packet = MIDIPacketListInit(list);
        OpMidiEvent batch[kMidiBatch];
        for (;;) {
            const uint32_t count = op_shell_midi_out(engine_, batch, kMidiBatch);
            if (count == 0) break;
            for (uint32_t i = 0; i < count; ++i) {
                const uint32_t length = midiLength(batch[i].data[0]);
                const MIDITimeStamp stamp = base + batch[i].offset;
                MIDIPacket* next = MIDIPacketListAdd(list, kMidiPacketBytes, packet, stamp, length, batch[i].data);
                if (!next) {
                    // Full: send what is there and start again.
                    target->proc(target->userData, &time, 0, list);
                    packet = MIDIPacketListInit(list);
                    next = MIDIPacketListAdd(list, kMidiPacketBytes, packet, stamp, length, batch[i].data);
                }
                if (next) packet = next;
            }
            if (count < kMidiBatch) break;
        }
        if (list->numPackets > 0) target->proc(target->userData, &time, 0, list);
    }
    static constexpr uint32_t kMidiBatch = 128;

    OSStatus render(AudioUnitRenderActionFlags& flags, const AudioTimeStamp& time, UInt32 frames) {
        if (!engine_ || frames == 0) return kAudioUnitErr_Uninitialized;
        if (resetPending_.exchange(false, std::memory_order_acquire)) {
            op_shell_reset(engine_);
            std::fill(delay_.begin(), delay_.end(), 0.0f);
            delayPos_ = 0;
        }
        // Pull the inputs: the main one for an effect, the sidechain when one is connected.
        AudioBufferList* inList = nullptr;
        if (hasMainIn_) {
            auto& input = this->Input(0);
            if (!input.IsActive()) return kAudioUnitErr_NoConnection;
            const OSStatus status = input.PullInput(flags, time, 0, frames);
            if (status != noErr) return status;
            inList = &input.GetBufferList();
        }
        AudioBufferList* scList = nullptr;
        if (plan_.scChannels > 0) {
            auto& sidechain = this->Input(scIndex_);
            if (sidechain.IsActive() && sidechain.PullInput(flags, time, scIndex_, frames) == noErr) scList = &sidechain.GetBufferList();
        }
        AudioBufferList& outList = this->Output(0).GetBufferList();
        const uint32_t nIn = plan_.inChannels, nOut = plan_.outChannels, nSc = scList ? plan_.scChannels : 0;
        if (outList.mNumberBuffers != nOut || (inList && inList->mNumberBuffers != nIn) || (scList && scList->mNumberBuffers != nSc)) return kAudioUnitErr_FormatNotSupported;
        for (uint32_t j = 0; j < nOut; ++j) if (!outList.mBuffers[j].mData) return kAudio_ParamError;
        for (uint32_t j = 0; j < nIn; ++j) if (!inList->mBuffers[j].mData) return kAudio_ParamError;
        for (uint32_t j = 0; j < nSc; ++j) if (!scList->mBuffers[j].mData) return kAudio_ParamError;

        // Events: what waited since the last render. Every parameter takes the unit's value now; a
        // parameter with an event changes again at the event, and ends the render on its last one.
        const uint32_t eventCount = events_.drain(pendingEvents_.data());
        std::fill(touched_.begin(), touched_.end(), uint8_t(0));
        for (uint32_t i = 0; i < eventCount; ++i) {
            const OpEvent& event = pendingEvents_[i];
            if (event.kind != OP_EVENT_PARAM_PLAIN || event.index >= touched_.size() || event.offset >= frames) continue;
            const size_t p = event.index;
            if (!touched_[p] || event.offset >= lastOffset_[p]) {
                touched_[p] = 1;
                lastOffset_[p] = event.offset;
                lastValue_[p] = event.value;
            }
        }
        for (uint32_t i = 0; i < params_.size(); ++i) {
            const float plain = this->Globals()->GetParameterRT(i);
            if (!std::isfinite(plain) || !op_shell_set_plain(engine_, i, plain)) return kAudioUnitErr_InvalidParameter;
        }
        OpTransport transport;
        const bool haveTransport = fillTransport(time, transport);

        const bool bypassable = hasMainIn_;
        const UInt32 limit = maxFrames_ > 0 ? maxFrames_ : frames;
        for (UInt32 base = 0; base < frames;) {
            const uint32_t count = static_cast<uint32_t>(std::min(limit, frames - base));
            const float* hostIn[OP_MAX_CHANNELS];
            float* hostOut[OP_MAX_CHANNELS];
            const float* canIn[OP_MAX_CHANNELS];
            float* canOut[OP_MAX_CHANNELS];
            const float* canSc[OP_MAX_CHANNELS];
            for (uint32_t j = 0; j < nIn; ++j) {
                hostIn[j] = static_cast<const float*>(inList->mBuffers[j].mData) + base;
                canIn[plan_.inPerm[j]] = hostIn[j];
            }
            for (uint32_t j = 0; j < nOut; ++j) {
                hostOut[j] = static_cast<float*>(outList.mBuffers[j].mData) + base;
                canOut[plan_.outPerm[j]] = hostOut[j];
            }
            for (uint32_t j = 0; j < nSc; ++j) canSc[plan_.scPerm[j]] = static_cast<const float*>(scList->mBuffers[j].mData) + base;

            const float target = bypass_.load(std::memory_order_relaxed) ? 0.0f : 1.0f;
            const bool settledWet = !bypassable || (mix_ == 1.0f && target == 1.0f && latency_ == 0);
            // The dry copy is taken before the plugin runs: the host may pass the same buffer for both.
            if (!settledWet) delayDry(nIn, nOut, hostIn, count);

            // Events in this piece, offsets counted from its start.
            const OpEvent* pieceEvents = pendingEvents_.data();
            uint32_t pieceCount = eventCount;
            if (frames > limit) {
                pieceCount = 0;
                for (uint32_t i = 0; i < eventCount; ++i) {
                    const OpEvent& event = pendingEvents_[i];
                    if (event.offset < base || event.offset >= base + count) continue;
                    chunkEvents_[pieceCount] = event;
                    chunkEvents_[pieceCount].offset -= base;
                    ++pieceCount;
                }
                pieceEvents = chunkEvents_.data();
            }
            OpTransport pieceTransport = transport;
            advance(pieceTransport, base, rate_);
            OpProcess args{};
            args.struct_size = sizeof(OpProcess);
            args.frames = count;
            args.inputs = nIn ? canIn : nullptr;
            args.input_channels = nIn;
            args.output_channels = nOut;
            args.outputs = canOut;
            args.sidechain = nSc ? canSc : nullptr;
            args.sidechain_channels = nSc;
            args.event_count = pieceCount;
            args.events = pieceCount ? pieceEvents : nullptr;
            args.transport = haveTransport ? &pieceTransport : nullptr;
            if (!op_shell_process_events(engine_, &args)) return kAudioUnitErr_FailedInitialization;
            if (!settledWet) crossfade(nOut, target, hostOut, count);
            if (hasMidiOut_) sendMidi(time, base);
            base += count;
        }
        // The parameters that had events end the render at the value of their last one.
        for (uint32_t i = 0; i < params_.size(); ++i) {
            if (touched_[i]) this->Globals()->SetParameterRT(i, lastValue_[i]);
        }
        flags &= ~kAudioUnitRenderAction_OutputIsSilence;
        return noErr;
    }
    // dry_[ch] gets the input delayed by the reported latency. The ring holds the last
    // latency_ samples of each channel and is written on every block, bypassed or not,
    // so it is already full when bypass starts. An output channel without an input is dry silence.
    void delayDry(uint32_t nIn, uint32_t nOut, const float* const* in, uint32_t count) {
        for (uint32_t ch = 0; ch < nOut; ++ch) {
            const float* src = ch < nIn ? in[ch] : nullptr;
            float* dry = dry_.data() + static_cast<size_t>(ch) * maxFrames_;
            if (latency_ == 0) {
                if (src) std::memcpy(dry, src, count * sizeof(float));
                else std::memset(dry, 0, count * sizeof(float));
                continue;
            }
            float* ring = delay_.data() + static_cast<size_t>(ch) * latency_;
            size_t pos = delayPos_;
            for (uint32_t i = 0; i < count; ++i) {
                dry[i] = ring[pos];
                ring[pos] = src ? src[i] : 0.0f;
                if (++pos == latency_) pos = 0;
            }
        }
        if (latency_ > 0) delayPos_ = (delayPos_ + count) % latency_;
    }
    // Moves mix_ (0 dry, 1 plugin) toward the target over about 10 ms and writes the blend.
    // Settled bypass is the delayed input exactly; settled processing leaves the output alone.
    void crossfade(uint32_t nOut, float target, float* const* out, uint32_t count) {
        if (mix_ == target && target == 1.0f) return;
        if (mix_ == target) {
            for (uint32_t ch = 0; ch < nOut; ++ch) std::memcpy(out[ch], dry_.data() + static_cast<size_t>(ch) * maxFrames_, count * sizeof(float));
            return;
        }
        float mix = mix_;
        for (uint32_t i = 0; i < count; ++i) {
            if (mix < target) mix = std::min(target, mix + mixStep_);
            else mix = std::max(target, mix - mixStep_);
            for (uint32_t ch = 0; ch < nOut; ++ch) {
                const float dry = dry_[static_cast<size_t>(ch) * maxFrames_ + i];
                out[ch][i] = mix == 0.0f ? dry : out[ch][i] * mix + dry * (1.0f - mix);
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
            const float plain = this->Globals()->GetParameter(i);
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
        for (uint32_t i = 0; i < params_.size(); ++i) this->Globals()->SetParameter(i, op_shell_get_plain(engine, i));
    }
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    // ---- telling the host about changes (ABI 6) ---------------------------------------
    // A timer on the main run loop asks the engine what changed and tells the host through
    // property notifications, which reach the listeners on the main thread, never inside a
    // render. A new latency: kAudioUnitProperty_Latency (GetLatency reads the engine, so the
    // host reads the new value at once). New parameter info: kAudioUnitProperty_ParameterList
    // on the global scope, then kAudioUnitProperty_ParameterInfo for each parameter.
    static constexpr CFTimeInterval kChangesPollSeconds = 0.030;
    static void onTimer(CFRunLoopTimerRef, void* info) { static_cast<OpenPluginAUImpl*>(info)->pollChanges(); }
    void startPolling() {
        CFRunLoopTimerContext context{};
        context.info = this;
        timer_ = CFRunLoopTimerCreate(nullptr, CFAbsoluteTimeGetCurrent() + kChangesPollSeconds, kChangesPollSeconds, 0, 0, &OpenPluginAUImpl::onTimer, &context);
        if (timer_) CFRunLoopAddTimer(CFRunLoopGetMain(), timer_, kCFRunLoopCommonModes);
    }
    void stopPolling() {
        if (timer_) {
            CFRunLoopTimerInvalidate(timer_);
            CFRelease(timer_);
            timer_ = nullptr;
        }
    }
    // Main thread. Returns the OP_CHANGE_* bits it told the host about.
    uint32_t pollChanges() {
        if (!engine_) return 0;
        const uint32_t flags = op_shell_take_changes(engine_) & (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO);
        if (flags & OP_CHANGE_LATENCY) this->PropertyChanged(kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0);
        if (flags & OP_CHANGE_PARAM_INFO) {
            this->PropertyChanged(kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0);
            for (uint32_t i = 0; i < params_.size(); ++i) this->PropertyChanged(kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, i);
        }
        return flags;
    }
    CFRunLoopTimerRef timer_ = nullptr;
    void* engine_ = nullptr;
    bool abiOk_ = false;
    bool kindOk_ = false;
    bool hasMainIn_ = true;
    bool hasMidiOut_ = false;
    bool hasSidechain_ = false;
    UInt32 scIndex_ = 1;
    std::vector<OpLayout> layouts_;
    std::vector<AUChannelInfo> channelInfo_;
    Plan plan_;
    double rate_ = 0.0;
    EventQueue events_;
    std::vector<OpEvent> pendingEvents_;
    std::vector<OpEvent> chunkEvents_;
    std::vector<uint8_t> touched_;
    std::vector<uint32_t> lastOffset_;
    std::vector<float> lastValue_;
    std::vector<uint8_t> midiPacketBuffer_;
    std::atomic<const MidiOutTarget*> midiOut_{nullptr};
    std::vector<std::unique_ptr<MidiOutTarget>> midiTargets_;
    std::atomic<bool> offline_{false};
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

#if OP_AU_TYPE_CODE == 1
using OpenPluginAU = OpenPluginAUImpl<MuBase, 1>;
AUSDK_COMPONENT_ENTRY(ausdk::AUMusicDeviceFactory, OpenPluginAU)
#elif OP_AU_TYPE_CODE == 2
using OpenPluginAU = OpenPluginAUImpl<MfBase, 2>;
AUSDK_COMPONENT_ENTRY(ausdk::AUMIDIEffectFactory, OpenPluginAU)
#else
using OpenPluginAU = OpenPluginAUImpl<FxBase, 0>;
AUSDK_COMPONENT_ENTRY(ausdk::AUBaseFactory, OpenPluginAU)
#endif
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
