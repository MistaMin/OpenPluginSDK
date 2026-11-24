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
// The host's name for the track (kAudioUnitProperty_ContextName) goes to
// op_shell_set_track_info on the main thread, and to the view.
#include "op_shell.h"
#include "op_shell_calls.h"
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
#include <limits>
#include <pthread.h>
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
    // The clump of the parameter's group (1, 2, ... in the order groups first appear), 0 for none.
    UInt32 clump = 0;
};
bool isNumberText(const std::string& text) {
    char* end = nullptr;
    std::strtod(text.c_str(), &end);
    return end != text.c_str() && *end == '\0';
}
// A choice parameter has whole steps from min to max, no unit, and labels that are words. The ABI
// has no "choice" flag, so a stepped number (labels like "3") stays an ordinary stepped parameter.
std::vector<std::string> choiceLabels(const void* engine, uint32_t index, const OpParamDesc& desc) {
    std::vector<std::string> labels;
    if (!(desc.flags & OP_PARAM_DISCRETE) || desc.unit[0] != '\0' || desc.steps < 3 || desc.steps > 256) return labels;
    if (desc.max - desc.min != static_cast<double>(desc.steps - 1) || desc.min != std::floor(desc.min)) return labels;
    bool allNumbers = true;
    for (uint32_t step = 0; step < desc.steps; ++step) {
        labels.push_back(op_shell_calls::instance_param_text(engine, index, desc.min + step));
        if (!isNumberText(labels.back())) allNumbers = false;
    }
    if (allNumbers) labels.clear();
    return labels;
}
// A CFString as UTF-8. Only the first 1024 UTF-16 units are read: the ABI keeps 255
// characters of a track name. Not for the render thread; it allocates.
std::string utf8FromCF(CFStringRef text) {
    if (!text) return {};
    const CFIndex length = std::min<CFIndex>(CFStringGetLength(text), 1024);
    std::string out(static_cast<size_t>(CFStringGetMaximumSizeForEncoding(length, kCFStringEncodingUTF8)) + 1, '\0');
    CFIndex used = 0;
    CFStringGetBytes(text, CFRangeMake(0, length), kCFStringEncodingUTF8, '?', false, reinterpret_cast<UInt8*>(out.data()), static_cast<CFIndex>(out.size()), &used);
    out.resize(static_cast<size_t>(used));
    return out;
}
// The shell's own key in the kAudioUnitProperty_ClassInfo dictionary. The value is the
// opaque blob of op_shell_save_state, as CFData. The base class's keys stay as they are.
#define OP_AU_STATE_KEY CFSTR("OpenPluginState")
// ---- one timer for every unit ---------------------------------------------------------
// A single timer on the main run loop while any unit lives, not one per unit. Each tick
// calls every unit's poll. Units are made and destroyed on any thread, so the list has a
// lock. It is never held during a poll: a poll notifies the host, which may make or
// dispose of units, or wait for a thread that does.
class Poller {
public:
    using Poll = void (*)(void*);
    static void add(void* unit, Poll poll) {
        State& s = state();
        std::lock_guard<std::mutex> lock(s.lock);
        s.units.push_back({unit, poll});
        if (s.timer) return;
        s.timer = CFRunLoopTimerCreate(nullptr, CFAbsoluteTimeGetCurrent() + kSeconds, kSeconds, 0, 0, &Poller::tick, nullptr);
        if (s.timer) CFRunLoopAddTimer(CFRunLoopGetMain(), s.timer, kCFRunLoopCommonModes);
    }
    static void remove(void* unit) {
        State& s = state();
        std::lock_guard<std::mutex> lock(s.lock);
        s.units.erase(std::remove_if(s.units.begin(), s.units.end(), [unit](const auto& entry) { return entry.first == unit; }), s.units.end());
        if (!s.units.empty() || !s.timer) return;
        CFRunLoopTimerInvalidate(s.timer);
        CFRelease(s.timer);
        s.timer = nullptr;
    }

private:
    static constexpr CFTimeInterval kSeconds = 0.030;
    struct State {
        std::mutex lock;
        std::vector<std::pair<void*, Poll>> units;
        CFRunLoopTimerRef timer = nullptr;
    };
    static State& state() {
        static State* s = new State;  // never destroyed: no order to get wrong at exit
        return *s;
    }
    static void tick(CFRunLoopTimerRef, void*) {
        State& s = state();
        // By index: a poll may add or remove units.
        for (size_t i = 0;; ++i) {
            std::pair<void*, Poll> unit;
            {
                std::lock_guard<std::mutex> lock(s.lock);
                if (i >= s.units.size()) return;
                unit = s.units[i];
            }
            unit.second(unit.first);
        }
    }
};
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
    // A full queue makes room: the oldest parameter event goes (a later one, or the unit's
    // own value, stands in for it), else the oldest event that is not a note-off. Only a
    // queue of nothing but note-offs refuses the new event. Every loss is counted.
    bool push(const OpEvent& event) {
        Hold hold(flag_);
        if (count_ >= OP_MAX_EVENTS) {
            ++dropped_;
            uint32_t victim = count_;
            for (uint32_t i = 0; i < count_; ++i) {
                if (events_[i].kind == OP_EVENT_PARAM_PLAIN) {
                    victim = i;
                    break;
                }
                if (victim == count_ && events_[i].kind != OP_EVENT_NOTE_OFF) victim = i;
            }
            if (victim == count_) return false;
            std::memmove(&events_[victim], &events_[victim + 1], (count_ - victim - 1) * sizeof(OpEvent));
            --count_;
        }
        events_[count_++] = event;
        return true;
    }
    // Events lost to a full queue since the last call.
    uint32_t takeDropped() {
        Hold hold(flag_);
        const uint32_t dropped = dropped_;
        dropped_ = 0;
        return dropped;
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
    uint32_t dropped_ = 0;
    OpEvent events_[OP_MAX_EVENTS];
};

struct MidiOutTarget {
    AUMIDIOutputCallback proc;
    void* userData;
};

// How a block's channels are laid out, decided at Initialize.
struct Plan {
    uint32_t layout = 0;
    // The layout is selected on the new engine (op_shell_set_layout). False only for the
    // one-channel engine of a plugin with no mono layout, which runs as it did on ABI 1.
    bool select = true;
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
            loadParameterIds(info.param_count);
            // Without pins the IDs are 0..n-1 and the element keeps them in an array, as it
            // always did. Pinned IDs are any numbers, so the element keeps them by ID: each
            // is made here, before the unit is initialized, and none is added later.
            if (identityIds_) this->Globals()->UseIndexedParameters(info.param_count);
            else for (uint32_t i = 0; i < info.param_count; ++i) this->Globals()->SetParameter(ids_[i], 0.0f);
            for (uint32_t i = 0; i < info.param_count; ++i) {
                if (!op_shell_param(i, &params_[i].desc)) continue;
                this->Globals()->SetParameter(ids_[i], static_cast<float>(params_[i].desc.default_plain));
                params_[i].labels = choiceLabels(nullptr, i, params_[i].desc);
                // Clumps are flat: one per distinct group path, named by the whole path.
                const std::string group = op_shell_calls::param_group(i);
                if (group.empty()) continue;
                auto known = std::find(clumps_.begin(), clumps_.end(), group);
                if (known == clumps_.end()) known = clumps_.insert(clumps_.end(), group);
                params_[i].clump = static_cast<UInt32>(known - clumps_.begin()) + 1;
            }
            const uint32_t kind = op_shell_kind();
            // aumu is the instrument; aufx and aumf are both audio effects (an aumf also takes MIDI).
            kindOk_ = (kType == 1) == (kind == OP_KIND_INSTRUMENT);
            hasMainIn_ = kind != OP_KIND_INSTRUMENT;
            const uint32_t notePorts = op_shell_note_ports();
            hasMidiOut_ = (notePorts & OP_NOTE_PORT_OUTPUT) != 0;
            supportsMpe_ = (notePorts & OP_NOTE_PORT_MPE) != 0;
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
        if (scope == kAudioUnitScope_Global && id == kOpenPluginAUProperty_EventsDropped) {
            size = sizeof(UInt32);
            writable = false;
            return noErr;
        }
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
            // Whether the plugin takes MPE (Plugin::mpe_input), for every plugin: 0 says no.
            if (id == kAudioUnitProperty_SupportsMPE) {
                size = sizeof(UInt32);
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
        if (scope == kAudioUnitScope_Global && id == kOpenPluginAUProperty_EventsDropped) {
            *static_cast<UInt32*>(data) = events_.takeDropped();
            return noErr;
        }
        if (id == kAudioUnitProperty_ParameterStringFromValue) {
            auto* request = static_cast<AudioUnitParameterStringFromValue*>(data);
            const int index = indexOf(request->inParamID);
            if (index < 0) return kAudioUnitErr_InvalidParameter;
            const double plain = request->inValue ? static_cast<double>(*request->inValue) : static_cast<double>(this->Globals()->GetParameter(request->inParamID));
            if (!std::isfinite(plain)) return kAudioUnitErr_InvalidParameterValue;
            const std::string text = op_shell_calls::instance_param_text(engine_, static_cast<uint32_t>(index), plain);
            // The host releases the string.
            request->outString = CFStringCreateWithBytes(nullptr, reinterpret_cast<const UInt8*>(text.data()), static_cast<CFIndex>(text.size()), kCFStringEncodingUTF8, false);
            return request->outString ? OSStatus(noErr) : OSStatus(kAudioUnitErr_InvalidPropertyValue);
        }
        if (id == kAudioUnitProperty_ParameterValueFromString) {
            auto* request = static_cast<AudioUnitParameterValueFromString*>(data);
            const int index = indexOf(request->inParamID);
            if (index < 0 || !request->inString) return kAudioUnitErr_InvalidParameter;
            const CFIndex room = CFStringGetMaximumSizeForEncoding(CFStringGetLength(request->inString), kCFStringEncodingUTF8) + 1;
            if (room > 4096) return kAudioUnitErr_InvalidPropertyValue;
            std::string text(static_cast<size_t>(room), '\0');
            if (!CFStringGetCString(request->inString, text.data(), room, kCFStringEncodingUTF8)) return kAudioUnitErr_InvalidPropertyValue;
            double plain = 0.0;
            if (!op_shell_instance_param_parse(engine_, static_cast<uint32_t>(index), text.c_str(), &plain) || !std::isfinite(plain)) return kAudioUnitErr_InvalidPropertyValue;
            request->outValue = static_cast<AudioUnitParameterValue>(plain);
            return noErr;
        }
        if (scope == kAudioUnitScope_Global) {
            if (id == kAudioUnitProperty_OfflineRender) {
                *static_cast<UInt32*>(data) = offline_.load(std::memory_order_relaxed) ? 1 : 0;
                return noErr;
            }
            if (id == kAudioUnitProperty_SupportsMPE) {
                *static_cast<UInt32*>(data) = supportsMpe_ ? 1 : 0;
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
                // The target this one replaces is kept, because a render may still hold it; older ones go.
                auto target = std::make_unique<MidiOutTarget>(MidiOutTarget{callback->midiOutputCallback, callback->userData});
                midiOut_.store(callback->midiOutputCallback ? target.get() : nullptr, std::memory_order_release);
                midiTargets_.push_back(std::move(target));
                if (midiTargets_.size() > 2) midiTargets_.erase(midiTargets_.begin());
                return noErr;
            }
        }
        return Base::SetProperty(id, scope, element, data, size);
    }
#if OP_HAS_EDITOR
    void* engine() const { return engine_; }
    uint32_t paramCount() const { return static_cast<uint32_t>(params_.size()); }
    float paramValue(uint32_t index) { return this->Globals()->GetParameter(ids_[index]); }
    // What the host last said about the track, for the view: the name as UTF-8 (empty for
    // none) and a count that moves with every change. False until the host has said anything.
    bool track(char* name, uint32_t capacity, uint32_t* serial) {
        std::lock_guard<std::mutex> lock(trackLock_);
        if (!track_.valid) return false;
        if (name && capacity) {
            size_t n = std::min<size_t>(track_.name.size(), capacity - 1);
            while (n > 0 && n < track_.name.size() && (static_cast<unsigned char>(track_.name[n]) & 0xC0) == 0x80) --n;
            std::memcpy(name, track_.name.data(), n);
            name[n] = '\0';
        }
        if (serial) *serial = track_.serial;
        return true;
    }
#endif
    // ---- the host's track ----------------------------------------------------------------
    // An Audio Unit host names the track in kAudioUnitProperty_ContextName (a CFString on the
    // global scope; Logic and GarageBand set it). The base class stores it and calls this.
    // The Audio Unit API has no property for a track's colour or kind, so the plugin hears a
    // name only. The name is read here, on the thread that set the property; the plugin is
    // told on the main thread: at once when this is it, else from the main run loop's timer.
    void PropertyChanged(AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element) override {
        if (id == kAudioUnitProperty_ContextName && scope == kAudioUnitScope_Global) noteTrack(utf8FromCF(this->GetContextName()));
        Base::PropertyChanged(id, scope, element);
    }

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
        const EngineSetup setup = engineSetup(plan);
        maxFrames_ = setup.frames;
        // The engine RestoreState made to check a blob already holds that blob; it is this
        // engine when it was made for the same rate, block size and layout.
        const bool adopted = scratch_ && scratchSetup_ == setup;
        engine_ = adopted ? scratch_.release() : makeEngine(setup);
        scratch_.reset();
        if (!engine_) {
            lastError_.store(kAudioUnitErr_FormatNotSupported, std::memory_order_relaxed);
            return kAudioUnitErr_FormatNotSupported;
        }
        const double rate = setup.rate;
        const UInt32 frames = setup.frames;
        plan_ = plan;
        op_shell_reset(engine_);
        op_shell_set_offline(engine_, offline_.load(std::memory_order_relaxed) ? 1 : 0);
        resetPending_.store(false, std::memory_order_relaxed);
        events_.clear();
        // A state restored while uninitialized (or kept over Uninitialize) goes in now, unless
        // this is the engine that checked it. It was checked when it arrived, so a refusal here
        // is not expected. The parameters need no sync: the unit's values are already the
        // blob's, and the render thread pushes them.
        if (!adopted && !pending_.empty()) op_shell_load_state(engine_, pending_.data(), static_cast<uint32_t>(pending_.size()));
        pending_.clear();
        pushAll_.store(true, std::memory_order_release);
        // A new engine has not heard about the track.
        {
            std::lock_guard<std::mutex> lock(trackLock_);
            if (track_.valid) trackPending_.store(true, std::memory_order_release);
        }
        if (pthread_main_np()) deliverTrack();
        // Everything the render thread uses is sized here, on the message thread, so it never allocates.
        // The bypass delay holds the largest latency the plugin may report (ABI 9), so the render
        // thread follows a latency that changes at run time by moving its read point alone.
        latency_ = op_shell_latency(engine_);
        delayCap_ = std::min(op_shell_max_latency(engine_), kMaxBypassDelay);
        delayPos_ = 0;
        delay_.clear();
        dry_.clear();
        if (hasMainIn_) {
            if (delayCap_ > 0) delay_.assign(static_cast<size_t>(plan.outChannels) * (delayCap_ + 1), 0.0f);
            dry_.assign(static_cast<size_t>(plan.outChannels) * frames, 0.0f);
        }
        mix_ = bypass_.load(std::memory_order_relaxed) ? 0.0f : 1.0f;
        mixStep_ = static_cast<float>(1.0 / std::max(1.0, kCrossfadeSeconds * rate));
        rate_ = rate;
        touched_.assign(params_.size(), 0);
        lastOffset_.assign(params_.size(), 0);
        lastValue_.assign(params_.size(), 0.0f);
        lastPushed_.assign(params_.size(), 0.0f);
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
    // The base class writes the dictionary's values into the element unchecked, so a value
    // that is not finite or is out of range is put back to the parameter's default.
    // Without an engine the blob is loaded into a scratch engine made as Initialize would
    // make it, which Initialize then adopts, and is held for a save.
    OSStatus RestoreState(CFPropertyListRef plist) override {
        if (!plist || CFGetTypeID(plist) != CFDictionaryGetTypeID()) return restoreBase(plist);
        const void* value = CFDictionaryGetValue(static_cast<CFDictionaryRef>(plist), OP_AU_STATE_KEY);
        if (!value) return (foreignKinds_ & (1u << (OP_FOREIGN_AU_CLASSINFO - 1))) ? restoreForeign(plist) : restoreBase(plist);
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
            pushAll_.store(true, std::memory_order_release);
            return status;
        }
        // The formats the host has set so far; a unit they do not fit yet gets a stereo engine,
        // which checks the blob and is not the one Initialize needs.
        Plan plan;
        if (!makePlan(plan)) {
            plan = Plan{};
            plan.select = false;
            plan.outChannels = 2;
        }
        EngineSetup setup = engineSetup(plan);
        if (!(setup.rate > 0.0)) setup.rate = 44100.0;
        // A refusal, of the blob or by the base class, takes the scratch engine with it.
        std::unique_ptr<void, EngineDeleter> scratch = std::move(scratch_);
        if (!scratch || !(scratchSetup_ == setup)) scratch.reset(makeEngine(setup));
        if (!scratch || !op_shell_load_state(scratch.get(), bytes, size)) return kAudioUnitErr_InvalidPropertyValue;
        const OSStatus status = Base::RestoreState(plist);
        if (status != noErr) return status;
        syncParameters(scratch.get());
        pending_.assign(bytes, bytes + size);
        scratch_ = std::move(scratch);
        scratchSetup_ = setup;
        return noErr;
    }
    // A dictionary the shell did not write (no OP_AU_STATE_KEY: a session or preset made by the
    // plugin this one replaces) and the plugin reads that kind (ABI 10). The base restore runs as
    // for any old preset; then the whole dictionary, as a binary property list, is offered to the
    // plugin, which reads what the base class cannot see (its own keys). The plugin's values then
    // win. A refusal leaves the base restore as it was. Without an engine a scratch engine takes
    // the state, as in RestoreState, and Initialize adopts it.
    OSStatus restoreForeign(CFPropertyListRef plist) {
        const OSStatus status = restoreBase(plist);
        CFDataRef bytes = CFPropertyListCreateData(nullptr, plist, kCFPropertyListBinaryFormat_v1_0, 0, nullptr);
        if (!bytes) return status;
        const CFIndex length = CFDataGetLength(bytes);
        if (length <= 0 || length > (16 << 20)) {
            CFRelease(bytes);
            return status;
        }
        void* target = engine_;
        std::unique_ptr<void, EngineDeleter> scratch;
        EngineSetup setup;
        if (!target) {
            Plan plan;
            if (!makePlan(plan)) {
                plan = Plan{};
                plan.select = false;
                plan.outChannels = 2;
            }
            setup = engineSetup(plan);
            if (!(setup.rate > 0.0)) setup.rate = 44100.0;
            scratch = std::move(scratch_);
            if (!scratch || !(scratchSetup_ == setup)) scratch.reset(makeEngine(setup));
            target = scratch.get();
        }
        const bool taken = target && op_shell_load_foreign_state(target, OP_FOREIGN_AU_CLASSINFO, CFDataGetBytePtr(bytes), static_cast<uint32_t>(length)) != 0;
        CFRelease(bytes);
        if (!taken) {
            // The scratch engine, if one was made, goes with the refusal; nothing was kept from it.
            return status;
        }
        syncParameters(target);
        if (engine_) {
            pushAll_.store(true, std::memory_order_release);
        } else {
            std::vector<uint8_t> blob;
            if (saveBlob(target, blob)) pending_ = std::move(blob);
            scratch_ = std::move(scratch);
            scratchSetup_ = setup;
        }
        return status;
    }
    const uint32_t foreignKinds_ = op_shell_foreign_kinds();
    // The base restore, then every parameter value it left that SetParameter would refuse
    // is replaced by the default.
    OSStatus restoreBase(CFPropertyListRef plist) {
        const OSStatus status = Base::RestoreState(plist);
        for (size_t i = 0; i < params_.size(); ++i) {
            if (!validValue(ids_[i], this->Globals()->GetParameter(ids_[i]))) this->Globals()->SetParameter(ids_[i], static_cast<float>(params_[i].desc.default_plain));
        }
        return status;
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
    // The parameters in the order of the plugin's table, whatever their IDs. With pinned IDs
    // the element holds them sorted by ID, and may hold a value a preset of another version
    // named; the host is shown the table.
    OSStatus GetParameterList(AudioUnitScope scope, AudioUnitParameterID* list, UInt32& count) override {
        if (identityIds_ || scope != kAudioUnitScope_Global) return Base::GetParameterList(scope, list, count);
        count = static_cast<UInt32>(ids_.size());
        if (list) std::copy(ids_.begin(), ids_.end(), list);
        return noErr;
    }
    OSStatus GetParameterInfo(AudioUnitScope scope, AudioUnitParameterID paramId, AudioUnitParameterInfo& info) override {
        const int index = indexOf(paramId);
        if (scope != kAudioUnitScope_Global || index < 0) return kAudioUnitErr_InvalidParameter;
        const auto id = static_cast<size_t>(index);
        std::memset(&info, 0, sizeof(info));
        const OpParamDesc& desc = params_[id].desc;
        // The host releases the CFString. A name that is not UTF-8 gives NULL, which the SDK would read.
        CFStringRef name = CFStringCreateWithCString(nullptr, desc.name, kCFStringEncodingUTF8);
        if (!name) name = CFStringCreateWithCString(nullptr, desc.id, kCFStringEncodingUTF8);
        if (!name) return kAudioUnitErr_InvalidPropertyValue;
        this->FillInParameterName(info, name, true);
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
        if (params_[id].clump) this->HasClump(info, params_[id].clump);
        return noErr;
    }
    // The name of a group's clump (kAudioUnitProperty_ParameterClumpName). AU clumps are flat, so
    // the name is the whole path ("Filter/Envelope"), which keeps "Osc 1/Env" and "Osc 2/Env"
    // apart. A host that asks for a shorter name gets the last name of the path, cut to fit. A
    // plugin without groups has no clumps and answers as before (the base class's error).
    OSStatus CopyClumpName(AudioUnitScope scope, UInt32 clumpID, UInt32 desiredLength, CFStringRef* outName) override {
        if (scope != kAudioUnitScope_Global || clumpID == 0 || clumpID > clumps_.size() || !outName)
            return Base::CopyClumpName(scope, clumpID, desiredLength, outName);
        const std::string& path = clumps_[clumpID - 1];
        CFStringRef name = CFStringCreateWithBytes(nullptr, reinterpret_cast<const UInt8*>(path.data()), static_cast<CFIndex>(path.size()), kCFStringEncodingUTF8, false);
        if (!name) return kAudio_ParamError;
        if (desiredLength > 0 && CFStringGetLength(name) > static_cast<CFIndex>(desiredLength)) {
            const size_t slash = path.rfind('/');
            const std::string last = slash == std::string::npos ? path : path.substr(slash + 1);
            CFRelease(name);
            CFStringRef shortName = CFStringCreateWithBytes(nullptr, reinterpret_cast<const UInt8*>(last.data()), static_cast<CFIndex>(last.size()), kCFStringEncodingUTF8, false);
            if (!shortName) return kAudio_ParamError;
            CFIndex keep = std::min(CFStringGetLength(shortName), static_cast<CFIndex>(desiredLength));
            // Do not cut a surrogate pair in two.
            if (keep < CFStringGetLength(shortName) && keep > 0 && CFStringIsSurrogateHighCharacter(CFStringGetCharacterAtIndex(shortName, keep - 1))) --keep;
            name = CFStringCreateWithSubstring(nullptr, shortName, CFRangeMake(0, keep));
            CFRelease(shortName);
            if (!name) return kAudio_ParamError;
        }
        *outName = name;  // the host releases it
        return noErr;
    }
    // The labels of a choice parameter, one per step from min (kAudioUnitParameterUnit_Indexed).
    OSStatus GetParameterValueStrings(AudioUnitScope scope, AudioUnitParameterID paramId, CFArrayRef* strings) override {
        const int index = indexOf(paramId);
        if (scope != kAudioUnitScope_Global || index < 0 || params_[static_cast<size_t>(index)].labels.empty()) return kAudioUnitErr_InvalidProperty;
        const auto id = static_cast<size_t>(index);
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
        const int index = indexOf(id);
        if (index < 0 || !std::isfinite(value)) return false;
        const OpParamDesc& desc = params_[static_cast<size_t>(index)].desc;
        return value >= desc.min - 1e-4 && value <= desc.max + 1e-4;
    }
    // A switch or choice set between its steps (a host's automation curve, a script) is stored on
    // the nearest step, as the plugin uses it. Otherwise the unit keeps 0.6 for a switch that is on,
    // and a host that labels Boolean and Indexed values itself shows it as off.
    AudioUnitParameterValue snapDiscrete(AudioUnitParameterID id, AudioUnitParameterValue value) const {
        const OpParamDesc& desc = params_[static_cast<size_t>(indexOf(id))].desc;
        if (!(desc.flags & OP_PARAM_DISCRETE) || desc.steps < 2 || !(desc.max > desc.min)) return value;
        const double divisions = static_cast<double>(desc.steps - 1);
        const double position = std::round((value - desc.min) / (desc.max - desc.min) * divisions) / divisions;
        return static_cast<AudioUnitParameterValue>(desc.min + (desc.max - desc.min) * std::clamp(position, 0.0, 1.0));
    }
    // A value with no offset is stored at once and reaches the plugin at the start of the next
    // render. A value with an offset (AudioUnitSetParameter's buffer offset, a scheduled
    // immediate event) waits as a sample-offset event and reaches the plugin at that sample.
    OSStatus SetParameter(AudioUnitParameterID id, AudioUnitScope scope, AudioUnitElement element, AudioUnitParameterValue value, UInt32 offset) override {
        if (scope == kAudioUnitScope_Global && element == 0) {
            if (!validValue(id, value)) return kAudioUnitErr_InvalidParameter;
            value = snapDiscrete(id, value);
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
            plan.select = false;
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
        const int index = indexOf(id);
        if (index < 0) return false;
        OpEvent event{};
        event.offset = offset;
        event.kind = OP_EVENT_PARAM_PLAIN;
        event.index = static_cast<uint32_t>(index);
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
        // Only a value the engine may not hold crosses the ABI: one that moved, or every one
        // after the main thread wrote the engine (pushAll_). A value that is not finite is
        // not pushed; the engine keeps its last one.
        const bool all = pushAll_.exchange(false, std::memory_order_acquire);
        for (uint32_t i = 0; i < params_.size(); ++i) {
            const float plain = this->Globals()->GetParameterRT(ids_[i]);
            if (!std::isfinite(plain) || (!all && plain == lastPushed_[i])) continue;
            if (!op_shell_set_plain(engine_, i, plain)) return kAudioUnitErr_InvalidParameter;
            lastPushed_[i] = plain;
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
            const bool settledWet = !bypassable || (mix_ == 1.0f && target == 1.0f && delayCap_ == 0);
            // The latency now (ABI 9; not the host's read, so no change is lost to it), within the
            // delay sized at Initialize.
            latency_ = std::min(op_shell_latency_now(engine_), delayCap_);
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
        // The engine may not have applied the event (a block during a state call is skipped),
        // so the next render pushes the value again.
        for (uint32_t i = 0; i < params_.size(); ++i) {
            if (!touched_[i]) continue;
            this->Globals()->SetParameterRT(ids_[i], lastValue_[i]);
            lastPushed_[i] = kNeverPushed;
        }
        flags &= ~kAudioUnitRenderAction_OutputIsSilence;
        return noErr;
    }
    // dry_[ch] gets the input delayed by latency_, the latency now. The ring holds the last
    // delayCap_ + 1 samples of each channel (the largest latency the plugin may report, sized at
    // Initialize) and is written on every block, bypassed or not, so it is already full when
    // bypass starts or the latency moves: a new latency only moves the read point. An output
    // channel without an input is dry silence.
    void delayDry(uint32_t nIn, uint32_t nOut, const float* const* in, uint32_t count) {
        const size_t size = static_cast<size_t>(delayCap_) + 1;
        for (uint32_t ch = 0; ch < nOut; ++ch) {
            const float* src = ch < nIn ? in[ch] : nullptr;
            float* dry = dry_.data() + static_cast<size_t>(ch) * maxFrames_;
            if (delayCap_ == 0) {
                if (src) std::memcpy(dry, src, count * sizeof(float));
                else std::memset(dry, 0, count * sizeof(float));
                continue;
            }
            float* ring = delay_.data() + static_cast<size_t>(ch) * size;
            size_t pos = delayPos_;
            size_t read = pos >= latency_ ? pos - latency_ : pos + size - latency_;
            for (uint32_t i = 0; i < count; ++i) {
                ring[pos] = src ? src[i] : 0.0f;
                dry[i] = ring[read];
                if (++pos == size) pos = 0;
                if (++read == size) read = 0;
            }
        }
        if (delayCap_ > 0) delayPos_ = (delayPos_ + count) % size;
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
            const float plain = this->Globals()->GetParameter(ids_[i]);
            if (std::isfinite(plain)) op_shell_set_plain(engine, i, plain);
        }
        pushAll_.store(true, std::memory_order_release);
        return op_shell_calls::save_blob(engine, blob);
    }
    // Makes the unit's parameter values the engine's. The render thread pushes the unit's
    // values into the engine every block, so after a restore they must agree.
    void syncParameters(void* engine) {
        for (uint32_t i = 0; i < params_.size(); ++i) this->Globals()->SetParameter(ids_[i], op_shell_get_plain(engine, i));
    }
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    // What an engine is made for: the stream format's rate, the largest slice and the plan's layout.
    struct EngineSetup {
        double rate = 0.0;
        UInt32 frames = 0;
        uint32_t channels = 0;
        uint32_t layout = 0;  // kNoLayout when none is selected
        bool operator==(const EngineSetup& o) const { return rate == o.rate && frames == o.frames && channels == o.channels && layout == o.layout; }
    };
    static constexpr uint32_t kNoLayout = ~0u;
    EngineSetup engineSetup(const Plan& plan) {
        const double rate = this->Output(0).GetStreamFormat().mSampleRate;
        UInt32 frames = this->GetMaxFramesPerSlice();
        if (frames < 1) frames = 512;
        if (frames > 65536) frames = 65536;
        return {rate, frames, std::max(plan.inChannels, plan.outChannels) > 1 ? 2u : 1u, !layouts_.empty() && plan.select ? plan.layout : kNoLayout};
    }
    // A new engine on its layout, or null. The layout is always selected, layout 0 included: a
    // new engine reports layout 0 but has not been prepared for it (the plugin saw only prepare
    // with one or two channels).
    static void* makeEngine(const EngineSetup& setup) {
        void* engine = op_shell_create(setup.rate, setup.frames, setup.channels);
        if (engine && setup.layout != kNoLayout && !op_shell_set_layout(engine, setup.layout)) {
            op_shell_destroy(engine);
            engine = nullptr;
        }
        return engine;
    }
    // Holds pending_ whenever it exists; only while there is no engine.
    std::unique_ptr<void, EngineDeleter> scratch_;
    EngineSetup scratchSetup_;
    // ---- telling the host about changes (ABI 6) ---------------------------------------
    // The timer on the main run loop (Poller) asks the engine what changed and tells the host through
    // property notifications, which reach the listeners on the main thread, never inside a
    // render. A new latency: kAudioUnitProperty_Latency (GetLatency reads the engine, so the
    // host reads the new value at once). New parameter info: kAudioUnitProperty_ParameterList
    // on the global scope, then kAudioUnitProperty_ParameterInfo for each parameter.
    static void onTimer(void* info) {
        auto* self = static_cast<OpenPluginAUImpl*>(info);
        self->pollTrack();
        self->pollChanges();
    }
    void startPolling() { Poller::add(this, &OpenPluginAUImpl::onTimer); }
    void stopPolling() { Poller::remove(this); }
    // Main thread. Returns the OP_CHANGE_* bits it told the host about.
    uint32_t pollChanges() {
        if (!engine_) return 0;
        const uint32_t flags = op_shell_take_changes(engine_) & (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO);
        if (flags & OP_CHANGE_LATENCY) this->PropertyChanged(kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0);
        if (flags & OP_CHANGE_PARAM_INFO) {
            // The engine's table first (ABI 9): names, units and choice labels, so the host
            // reads the renamed table when it hears.
            for (uint32_t i = 0; i < params_.size(); ++i) {
                OpParamDesc desc{};
                if (!op_shell_instance_param(engine_, i, &desc)) continue;
                params_[i].desc = desc;
                params_[i].labels = choiceLabels(engine_, i, desc);
            }
            this->PropertyChanged(kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0);
            for (uint32_t i = 0; i < params_.size(); ++i) this->PropertyChanged(kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, ids_[i]);
        }
        return flags;
    }
    // ---- the host's track (see PropertyChanged) ------------------------------------------
    struct TrackState {
        bool valid = false;   // the host has said something
        std::string name;     // UTF-8; empty for none
        uint32_t serial = 0;  // moves with every change
    };
    // Any thread but the render thread. A name equal to the last one changes nothing.
    void noteTrack(std::string name) {
        {
            std::lock_guard<std::mutex> lock(trackLock_);
            if (track_.valid && track_.name == name) return;
            track_.valid = true;
            track_.name = std::move(name);
            ++track_.serial;
            trackPending_.store(true, std::memory_order_release);
        }
        if (pthread_main_np()) deliverTrack();
    }
    // Main thread. Tells the engine what the host last said, once.
    void deliverTrack() {
        if (!engine_ || !trackPending_.exchange(false, std::memory_order_acq_rel)) return;
        std::string name;
        {
            std::lock_guard<std::mutex> lock(trackLock_);
            name = track_.name;
        }
        op_shell_set_track_info(engine_, name.empty() ? nullptr : name.c_str(), 0, 0);
    }
    // Main thread, from the timer. A host that removes the property (the base class then
    // drops the name without a notification) is seen here, and a name set from another
    // thread, or before there was an engine, is delivered here.
    void pollTrack() {
        bool removed = false;
        if (!this->GetContextName()) {
            std::lock_guard<std::mutex> lock(trackLock_);
            removed = track_.valid && !track_.name.empty();
        }
        if (removed) noteTrack(std::string());
        deliverTrack();
    }
    std::mutex trackLock_;
    TrackState track_;
    std::atomic<bool> trackPending_{false};
    void* engine_ = nullptr;
    bool abiOk_ = false;
    bool kindOk_ = false;
    bool hasMainIn_ = true;
    bool hasMidiOut_ = false;
    bool supportsMpe_ = false;
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
    // What the render thread last gave the engine for each parameter; kNeverPushed forces a push.
    std::vector<float> lastPushed_;
    static constexpr float kNeverPushed = std::numeric_limits<float>::quiet_NaN();
    // The main thread wrote the engine's values (a state call, a new engine): push them all.
    std::atomic<bool> pushAll_{true};
    std::vector<uint8_t> midiPacketBuffer_;
    std::atomic<const MidiOutTarget*> midiOut_{nullptr};
    std::vector<std::unique_ptr<MidiOutTarget>> midiTargets_;
    std::atomic<bool> offline_{false};
    std::atomic<bool> resetPending_{false};
    std::atomic<OSStatus> lastError_{noErr};
    UInt32 maxFrames_ = 512;
    static constexpr double kCrossfadeSeconds = 0.010;
    std::atomic<bool> bypass_{false};
    // The bypass delay of this render (the latency now, at most delayCap_), and the largest
    // latency the delay holds, sized at Initialize. Larger than this is not a latency: past
    // it the shell delays the bypassed signal by the cap.
    static constexpr uint32_t kMaxBypassDelay = 1u << 22;
    uint32_t latency_ = 0;
    uint32_t delayCap_ = 0;
    size_t delayPos_ = 0;
    float mix_ = 1.0f;
    float mixStep_ = 1.0f;
    std::vector<float> delay_;
    std::vector<float> dry_;
    std::vector<CachedParam> params_;
    // ---- parameter IDs (ABI 8) ---------------------------------------------------------
    // The AudioUnitParameterID the host stores for each parameter (op_shell_param_host_id):
    // the parameter's index for a plugin without pins, as always, or its pinned ID, which
    // stays with it through any reordering of the table. The ABI and everything in this
    // shell address a parameter by its index; ids_ and indexOf are the edge to the host.
    void loadParameterIds(uint32_t count) {
        ids_.assign(count, 0);
        sortedIds_.clear();
        identityIds_ = true;
        bool usable = true;
        for (uint32_t i = 0; i < count; ++i) {
            ids_[i] = op_shell_param_host_id(OP_HOST_AU, i);
            if (ids_[i] != i) identityIds_ = false;
            if (ids_[i] == OP_NO_HOST_ID) usable = false;
        }
        if (!identityIds_ && usable) {
            for (uint32_t i = 0; i < count; ++i) sortedIds_.push_back({ids_[i], i});
            std::sort(sortedIds_.begin(), sortedIds_.end());
            for (size_t i = 1; i < sortedIds_.size(); ++i) usable = usable && sortedIds_[i].first != sortedIds_[i - 1].first;
        }
        // The library checks its pins; one that answers no ID, or one ID twice, gets the indices.
        if (!identityIds_ && !usable) {
            for (uint32_t i = 0; i < count; ++i) ids_[i] = i;
            sortedIds_.clear();
            identityIds_ = true;
        }
    }
    // The index of the parameter the host calls `id`, or -1. No allocation: also on the render thread.
    int indexOf(AudioUnitParameterID id) const {
        if (identityIds_) return id < ids_.size() ? static_cast<int>(id) : -1;
        const auto at = std::lower_bound(sortedIds_.begin(), sortedIds_.end(), std::pair<AudioUnitParameterID, uint32_t>{id, 0});
        return at != sortedIds_.end() && at->first == id ? static_cast<int>(at->second) : -1;
    }
    std::vector<AudioUnitParameterID> ids_;
    std::vector<std::pair<AudioUnitParameterID, uint32_t>> sortedIds_;
    bool identityIds_ = true;
    // Group path of each clump; clump ID n is clumps_[n - 1].
    std::vector<std::string> clumps_;
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
// The ID the shell gives the host: the library's answer when it is usable (it is for
// every index when it is for one), else the index, as loadParameterIds decides.
uint32_t op_au_param_id(uint32_t index) {
    static const std::vector<uint32_t> ids = [] {
        OpShellInfo info{};
        std::vector<uint32_t> list(op_shell_info(&info) ? info.param_count : 0);
        bool usable = true;
        for (uint32_t i = 0; i < list.size(); ++i) {
            list[i] = op_shell_param_host_id(OP_HOST_AU, i);
            usable = usable && list[i] != OP_NO_HOST_ID;
        }
        std::vector<uint32_t> sorted = list;
        std::sort(sorted.begin(), sorted.end());
        usable = usable && std::adjacent_find(sorted.begin(), sorted.end()) == sorted.end();
        if (!usable) for (uint32_t i = 0; i < list.size(); ++i) list[i] = i;
        return list;
    }();
    return index < ids.size() ? ids[index] : index;
}
uint32_t op_au_track(void* instance, char* name, uint32_t capacity, uint32_t* serial) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    return unit && unit->track(name, capacity, serial) ? 1 : 0;
}
uint32_t op_au_param(void* instance, uint32_t index, float* value) {
    std::lock_guard<std::mutex> lock(gLiveLock);
    OpenPluginAU* unit = live(instance);
    if (!unit || !value || index >= unit->paramCount()) return 0;
    *value = unit->paramValue(index);
    return 1;
}
#endif
