// SPDX-License-Identifier: MIT
// VST3 shell on ABI 2, through the VST3 hosting interfaces only: notes in and MIDI out
// (IEventList and IMidiMapping), ProcessContext to the transport, the aux sidechain
// bus, speaker arrangements and channel order for mono, stereo, 5.1, 7.1 and 7.1.4,
// kOffline, and the category from the plugin kind. The plugins are the stand-ins of
// events_stub.cpp (the example has no notes, sidechain or surround layouts); they put
// what they see into audio and into their plugin state (events_probe.h).
//
//   VST3EventsProof <Events Stub.vst3> <Synth Stub.vst3> <Note Stub.vst3>
#include "events_probe.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstmidicontrollers.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <new>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 events proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
constexpr double kRate = 48000.0;
constexpr int kBlock = 512;

using Audio = std::vector<std::vector<float>>;

// operator new counted while process() runs (the shell is C++; the plugin ABI is checked by its own tests).
static thread_local bool g_armed = false;  // per thread: only the arming thread is counted
static long g_allocs = 0;
void* operator new(std::size_t size) {
    if (g_armed) ++g_allocs;
    if (void* p = std::malloc(size ? size : 1)) return p;
    throw std::bad_alloc();
}
void operator delete(void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }

struct Plug {
    CFBundleRef bundle = nullptr;
    bool (*exitFn)() = nullptr;
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    std::string category;
    bool active = false;

    explicit Plug(const char* path) {
        CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(path), static_cast<CFIndex>(std::strlen(path)), true);
        CHECK(url);
        bundle = CFBundleCreate(nullptr, url);
        CFRelease(url);
        CHECK(bundle && CFBundleLoadExecutable(bundle));
        auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
        exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
        auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
        CHECK(entry && exitFn && getFactory && entry(bundle));
        factory = getFactory();
        CHECK(factory);
        IPluginFactory2* factory2 = nullptr;
        CHECK(factory->queryInterface(IPluginFactory2::iid, reinterpret_cast<void**>(&factory2)) == kResultOk && factory2);
        PClassInfo2 effect{};
        bool found = false;
        for (int32 i = 0; i < factory2->countClasses(); ++i) {
            PClassInfo2 info{};
            CHECK(factory2->getClassInfo2(i, &info) == kResultOk);
            if (std::strcmp(info.category, kVstAudioEffectClass) == 0) {
                effect = info;
                found = true;
            }
        }
        CHECK(found);
        category = effect.subCategories;
        factory2->release();
        CHECK(factory->createInstance(effect.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        // Parameter text is UTF-8 in the ABI and UTF-16 in VST3: the stub's text holds
        // 2, 3 and 4 byte characters, and must arrive as the same characters.
        String128 text{};
        CHECK(controller->getParamStringByValue(0, 0.5, text) == kResultOk);
        static const TChar expected[] = {'0', '.', '5', '0', ' ', 0x00E9, 0x2713, 0xD83D, 0xDE00, ' ', 's', 't', 'u', 'b', 0};
        CHECK(std::memcmp(text, expected, sizeof(expected)) == 0);
        double typed = -1.0;
        CHECK(controller->getParamValueByString(0, text, typed) == kResultOk && typed == 0.5);
        // So is a parameter's name: the stub's has two 2 byte characters.
        ParameterInfo named{};
        CHECK(controller->getParameterInfo(0, named) == kResultOk);
        static const TChar name[] = {'M', 0x00F6, 'd', 0x00E9, 0};
        CHECK(std::memcmp(named.title, name, sizeof(name)) == 0);
    }
    ~Plug() {
        if (active) stop();
        processor->release();
        controller->terminate();
        controller->release();
        component->terminate();
        component->release();
        factory->release();
        exitFn();
        CFRelease(bundle);
    }
    void stop() {
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        active = false;
    }
    // Buses active as asked, then ready to process.
    void start(const std::vector<bool>& inputsActive, bool outputActive) {
        if (active) stop();
        for (size_t i = 0; i < inputsActive.size(); ++i) CHECK(component->activateBus(kAudio, kInput, static_cast<int32>(i), inputsActive[i]) == kResultOk);
        if (component->getBusCount(kAudio, kOutput) > 0) CHECK(component->activateBus(kAudio, kOutput, 0, outputActive) == kResultOk);
        for (int32 i = 0; i < component->getBusCount(kEvent, kInput); ++i) CHECK(component->activateBus(kEvent, kInput, i, true) == kResultOk);
        for (int32 i = 0; i < component->getBusCount(kEvent, kOutput); ++i) CHECK(component->activateBus(kEvent, kOutput, i, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, kBlock, kRate};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        active = true;
    }
    // A host changes the arrangement of an inactive processor only (the shell refuses otherwise).
    tresult arrange(std::vector<SpeakerArrangement> in, std::vector<SpeakerArrangement> out) {
        if (active) stop();
        return processor->setBusArrangements(in.data(), static_cast<int32>(in.size()), out.data(), static_cast<int32>(out.size()));
    }
    SpeakerArrangement arrangementOf(BusDirection dir, int32 index) {
        SpeakerArrangement arr = ~0ull;
        CHECK(processor->getBusArrangement(dir, index, arr) == kResultTrue);
        return arr;
    }
    EventsProbe probe() {
        MemoryStream stream;
        CHECK(component->getState(&stream) == kResultOk);
        const uint8_t* bytes = reinterpret_cast<const uint8_t*>(stream.getData());
        const size_t size = static_cast<size_t>(stream.getSize());
        int32 count = 0;
        std::memcpy(&count, bytes + 4, 4);
        const size_t head = 8 + 8 * static_cast<size_t>(count) + 4;
        CHECK(size == head + 12 + sizeof(EventsProbe));
        uint32_t tag = 0;
        std::memcpy(&tag, bytes + head, 4);
        CHECK(tag == 0x4F505342);
        EventsProbe p{};
        std::memcpy(&p, bytes + head + 12, sizeof(p));
        CHECK(p.magic == kEventsProbeMagic);
        return p;
    }
};

struct Result {
    Audio out;
    std::vector<Event> events;
};

// Set: the aux bus is sent active with its channel pointers null, as a host may for a
// sidechain it has not connected.
static bool g_nullSide = false;

// One block. `in`, `out` and `side` are host-order channel buffers; an empty
// `side` with `auxBus` set sends the aux bus inactive (0 channels).
static Result process(Plug& p, int frames, const Audio* in, const Audio* side, int outChannels, bool auxBus, ParameterChanges* params = nullptr,
                      EventList* inEvents = nullptr, ProcessContext* context = nullptr, int32 mode = kRealtime) {
    Result result;
    result.out.assign(static_cast<size_t>(outChannels), std::vector<float>(static_cast<size_t>(frames), 7.0f));
    std::vector<float*> outPtrs;
    for (auto& channel : result.out) outPtrs.push_back(channel.data());
    Audio inCopy = in ? *in : Audio();
    Audio sideCopy = side ? *side : Audio();
    std::vector<float*> inPtrs, sidePtrs;
    for (auto& channel : inCopy) inPtrs.push_back(channel.data());
    for (auto& channel : sideCopy) sidePtrs.push_back(g_nullSide ? nullptr : channel.data());
    AudioBusBuffers inputs[2]{};
    int32 numInputs = 0;
    if (in) {
        inputs[numInputs].numChannels = static_cast<int32>(inPtrs.size());
        inputs[numInputs].channelBuffers32 = inPtrs.data();
        ++numInputs;
    }
    if (auxBus) {
        inputs[numInputs].numChannels = static_cast<int32>(sidePtrs.size());
        inputs[numInputs].channelBuffers32 = sidePtrs.empty() ? nullptr : sidePtrs.data();
        ++numInputs;
    }
    AudioBusBuffers output{};
    output.numChannels = outChannels;
    output.channelBuffers32 = outPtrs.data();
    EventList outEvents(256);
    ProcessData data{};
    data.processMode = mode;
    data.symbolicSampleSize = kSample32;
    data.numSamples = frames;
    data.numInputs = numInputs;
    data.inputs = numInputs ? inputs : nullptr;
    data.numOutputs = outChannels ? 1 : 0;
    data.outputs = outChannels ? &output : nullptr;
    data.inputParameterChanges = params;
    data.inputEvents = inEvents;
    data.outputEvents = &outEvents;
    data.processContext = context;
    g_armed = true;
    const tresult processed = p.processor->process(data);
    g_armed = false;
    CHECK(processed == kResultOk);
    CHECK(g_allocs == 0);  // process() allocates nothing: notes, MIDI out, transport, sidechain, surround
    for (int32 i = 0; i < outEvents.getEventCount(); ++i) {
        Event e{};
        CHECK(outEvents.getEvent(i, e) == kResultOk);
        result.events.push_back(e);
    }
    return result;
}
static Audio constant(size_t channels, int frames, float value) { return Audio(channels, std::vector<float>(static_cast<size_t>(frames), value)); }
static Event noteOn(int32 offset, int16 pitch, float velocity) {
    Event e{};
    e.type = Event::kNoteOnEvent;
    e.sampleOffset = offset;
    e.noteOn.channel = 0;
    e.noteOn.pitch = pitch;
    e.noteOn.velocity = velocity;
    e.noteOn.noteId = -1;
    return e;
}
static Event noteOff(int32 offset, int16 pitch) {
    Event e{};
    e.type = Event::kNoteOffEvent;
    e.sampleOffset = offset;
    e.noteOff.channel = 0;
    e.noteOff.pitch = pitch;
    e.noteOff.noteId = -1;
    return e;
}
static void point(ParameterChanges& changes, ParamID id, int32 offset, double value) {
    int32 index = 0;
    auto* queue = changes.addParameterData(id, index);
    CHECK(queue);
    queue->addPoint(offset, value, index);
}
static bool near(float a, float b) { return std::fabs(a - b) < 1e-6f; }
static ParamID mapped(IMidiMapping* mapping, int16 channel, int16 controller) {
    ParamID id = 0;
    CHECK(mapping->getMidiControllerAssignment(0, channel, controller, id) == kResultTrue);
    return id;
}

static void check_notes_and_midi(Plug& p, IMidiMapping* mapping, int outputs, bool hasInput) {
    CHECK(near(1.0f, 1.0f));
    ParameterChanges params;
    point(params, mapped(mapping, 0, 1), 50, 0.5);                       // CC 1 = 64 at 50
    point(params, mapped(mapping, 0, kPitchBend), 200, 1.0);             // pitch bend max at 200
    EventList events(16);
    Event on = noteOn(100, 60, 0.5f);
    Event off = noteOff(300, 60);
    Event pressure{};
    pressure.type = Event::kLegacyMIDICCOutEvent;
    pressure.sampleOffset = 250;
    pressure.midiCCOut.controlNumber = kAfterTouch;
    pressure.midiCCOut.value = 40;
    CHECK(events.addEvent(on) == kResultOk && events.addEvent(off) == kResultOk && events.addEvent(pressure) == kResultOk);
    const Audio silence = constant(outputs ? static_cast<size_t>(outputs) : 0, kBlock, 0.0f);
    const Result r = process(p, kBlock, hasInput ? &silence : nullptr, nullptr, outputs, false, &params, &events);
    if (outputs) {
        for (int s = 0; s < kBlock; ++s) {
            const float expected = s >= 100 && s < 300 ? 61.0f / 128.0f : 0.0f;  // the held note, sample-exact
            CHECK(near(r.out[0][s], expected));
            for (int c = 1; c < outputs; ++c) CHECK(near(r.out[c][s], 0.0f));
        }
    }
    CHECK(r.events.size() == 5);
    CHECK(r.events[0].sampleOffset == 50 && r.events[0].type == Event::kLegacyMIDICCOutEvent && r.events[0].midiCCOut.controlNumber == 1 &&
          r.events[0].midiCCOut.value == 64 && r.events[0].midiCCOut.channel == 0);
    CHECK(r.events[1].sampleOffset == 100 && r.events[1].type == Event::kNoteOnEvent && r.events[1].noteOn.pitch == 72 &&
          std::lround(r.events[1].noteOn.velocity * 127.0f) == 64 && r.events[1].noteOn.channel == 0);
    CHECK(r.events[2].sampleOffset == 200 && r.events[2].type == Event::kLegacyMIDICCOutEvent && r.events[2].midiCCOut.controlNumber == kPitchBend &&
          r.events[2].midiCCOut.value == 127 && r.events[2].midiCCOut.value2 == 127);
    CHECK(r.events[3].sampleOffset == 250 && r.events[3].type == Event::kLegacyMIDICCOutEvent && r.events[3].midiCCOut.controlNumber == kAfterTouch &&
          r.events[3].midiCCOut.value == 40);
    CHECK(r.events[4].sampleOffset == 300 && r.events[4].type == Event::kNoteOffEvent && r.events[4].noteOff.pitch == 72);
}

int main(int argc, char** argv) {
    CHECK(argc == 4);

    // ---- effect with sidechain and surround layouts -----------------------------------------
    {
        Plug p(argv[1]);
        CHECK(p.category == "Fx|Tools");
        CHECK(p.component->getBusCount(kAudio, kInput) == 2);
        CHECK(p.component->getBusCount(kAudio, kOutput) == 1);
        CHECK(p.component->getBusCount(kEvent, kInput) == 1);
        CHECK(p.component->getBusCount(kEvent, kOutput) == 1);
        BusInfo info{};
        CHECK(p.component->getBusInfo(kAudio, kInput, 0, info) == kResultOk);
        CHECK(info.busType == kMain && (info.flags & BusInfo::kDefaultActive));
        CHECK(p.component->getBusInfo(kAudio, kInput, 1, info) == kResultOk);
        CHECK(info.busType == kAux && !(info.flags & BusInfo::kDefaultActive));
        CHECK(info.channelCount == 2);
        std::printf("VST3 events: effect category %s, main + aux sidechain (inactive by default), event buses in and out\n", p.category.c_str());

        // IMidiMapping: controllers, pressure and pitch bend map to distinct hidden parameters.
        IMidiMapping* mapping = nullptr;
        CHECK(p.controller->queryInterface(IMidiMapping::iid, reinterpret_cast<void**>(&mapping)) == kResultOk && mapping);
        CHECK(p.controller->getParameterCount() == 1 + 1 + 16 * 130);
        ParamID a = mapped(mapping, 0, 1), b = mapped(mapping, 0, kPitchBend), c = mapped(mapping, 15, kAfterTouch);
        CHECK(a != b && b != c && a != c);
        ParamID none = 0;
        CHECK(mapping->getMidiControllerAssignment(0, 16, 1, none) != kResultTrue);
        CHECK(mapping->getMidiControllerAssignment(1, 0, 1, none) != kResultTrue);
        int hidden = 0;
        for (int32 i = 0; i < p.controller->getParameterCount(); ++i) {
            ParameterInfo pi{};
            CHECK(p.controller->getParameterInfo(i, pi) == kResultOk);
            if (pi.flags & ParameterInfo::kIsHidden) {
                ++hidden;
                CHECK(pi.id >= 0x4D494400);
            }
        }
        CHECK(hidden == 16 * 130);
        std::printf("VST3 events: IMidiMapping assigns 16 x 130 hidden parameters (CC, pressure, pitch bend)\n");

        // setBusArrangements / getBusArrangement over the plugin's layouts.
        using namespace SpeakerArr;
        CHECK(p.arrange({kStereo, kStereo}, {kStereo}) == kResultTrue);
        CHECK(p.arrangementOf(kInput, 0) == kStereo && p.arrangementOf(kInput, 1) == kStereo && p.arrangementOf(kOutput, 0) == kStereo);
        CHECK(p.arrange({kStereo, kStereo}, {kMono}) != kResultTrue);   // no such layout
        CHECK(p.arrange({kStereo, kMono}, {kStereo}) != kResultTrue);   // sidechain width not offered
        CHECK(p.arrange({k30Cine, kEmpty}, {k30Cine}) != kResultTrue);  // 3.0 is not one of the layouts
        CHECK(p.arrangementOf(kInput, 0) == kStereo && p.arrangementOf(kOutput, 0) == kStereo);  // a refusal changes nothing
        CHECK(p.arrange({kMono, kEmpty}, {kMono}) == kResultTrue);
        CHECK(p.arrangementOf(kInput, 0) == kMono && p.arrangementOf(kOutput, 0) == kMono);
        CHECK(p.arrange({k51, kEmpty}, {k51}) == kResultTrue);
        CHECK(p.arrange({k71Music, kEmpty}, {k71Music}) == kResultTrue);
        CHECK(p.arrange({k71_4, kEmpty}, {k71_4}) == kResultTrue);
        CHECK(p.arrangementOf(kOutput, 0) == k71_4);
        std::printf("VST3 events: arrangements mono, stereo (+ sidechain), 5.1, 7.1, 7.1.4 accepted; 3.0, mixed in/out and a mono sidechain refused\n");

        // Notes in, MIDI out, sample-accurate.
        CHECK(p.arrange({kStereo, kEmpty}, {kStereo}) == kResultTrue);
        p.start({true, false}, true);
        {  // Self-check of the counter: the shell allocates in getState (the plugin state blob).
            MemoryStream stream;
            stream.setSize(4096);
            g_allocs = 0;
            g_armed = true;
            p.component->getState(&stream);
            g_armed = false;
            CHECK(g_allocs > 0);
            g_allocs = 0;
        }
        check_notes_and_midi(p, mapping, 2, true);
        std::printf("VST3 events: note on 60 at 100 and off at 300 sound exactly there; CC and pitch bend (IMidiMapping), pressure (legacy) and notes come back as output events at the same offsets\n");

        // Transport.
        {
            ProcessContext ctx{};
            ctx.state = ProcessContext::kPlaying | ProcessContext::kRecording | ProcessContext::kTempoValid | ProcessContext::kTimeSigValid |
                        ProcessContext::kProjectTimeMusicValid | ProcessContext::kBarPositionValid;
            ctx.sampleRate = kRate;
            ctx.projectTimeSamples = 96000;
            ctx.tempo = 123.5;
            ctx.timeSigNumerator = 3;
            ctx.timeSigDenominator = 4;
            ctx.projectTimeMusic = 7.25;
            ctx.barPositionMusic = 6.0;
            const Audio in = constant(2, kBlock, 0.0f);
            process(p, kBlock, &in, nullptr, 2, true, nullptr, nullptr, &ctx);
            EventsProbe probe = p.probe();
            CHECK(probe.have_transport);
            const OpTransport& t = probe.transport;
            CHECK(t.flags == (OP_TRANSPORT_PLAYING | OP_TRANSPORT_RECORDING | OP_TRANSPORT_HAS_TEMPO | OP_TRANSPORT_HAS_SIGNATURE | OP_TRANSPORT_HAS_BEATS |
                              OP_TRANSPORT_HAS_SECONDS));
            CHECK(t.tempo == 123.5 && t.numerator == 3 && t.denominator == 4);
            CHECK(t.sample_pos == 96000 && std::fabs(t.seconds - 2.0) < 1e-12);
            CHECK(t.quarter_notes == 7.25 && t.bar_start == 6.0 && t.bar_number == 2);  // 3/4: bars of 3 quarter notes
            // Only what the host marks valid is passed on.
            ProcessContext stopped{};
            stopped.state = ProcessContext::kTempoValid;
            stopped.sampleRate = kRate;
            stopped.projectTimeSamples = 480;
            stopped.tempo = 90.0;
            process(p, kBlock, &in, nullptr, 2, true, nullptr, nullptr, &stopped);
            const OpTransport& u = p.probe().transport;
            CHECK(u.flags == (OP_TRANSPORT_HAS_TEMPO | OP_TRANSPORT_HAS_SECONDS) && u.tempo == 90.0 && u.sample_pos == 480 && u.numerator == 0);
            // No ProcessContext keeps the previous transport.
            process(p, kBlock, &in, nullptr, 2, true);
            const OpTransport& v = p.probe().transport;
            CHECK(v.flags == u.flags && v.tempo == 90.0 && v.sample_pos == 480);
            std::printf("VST3 events: ProcessContext reaches the plugin (tempo 123.5, 3/4, 96000 samples = 2 s, 7.25 quarter notes, bar start 6, bar 2, playing and recording); invalid fields stay unset; no context keeps the last\n");
        }

        // Sidechain.
        {
            const Audio in = constant(2, kBlock, 0.1f);
            const Audio side = constant(2, kBlock, 0.25f);
            p.stop();
            CHECK(p.arrange({SpeakerArr::kStereo, SpeakerArr::kStereo}, {SpeakerArr::kStereo}) == kResultTrue);
            p.start({true, true}, true);
            Result r = process(p, kBlock, &in, &side, 2, true);
            for (int s = 0; s < kBlock; ++s) CHECK(near(r.out[0][s], 0.35f) && near(r.out[1][s], 0.1f));
            CHECK(p.probe().sidechain_channels == 2);
            p.start({true, false}, true);  // aux inactive: the host sends it with no channels
            r = process(p, kBlock, &in, nullptr, 2, true);
            for (int s = 0; s < kBlock; ++s) CHECK(near(r.out[0][s], 0.1f));
            CHECK(p.probe().sidechain_channels == 0);
            std::printf("VST3 events: the aux bus reaches the plugin as the sidechain (0.1 + 0.25 = 0.35); inactive, it arrives as none\n");

            // Null channel pointers on an active aux bus, in a block the shell splits: more
            // events at offset 0 than one call takes ends the first piece at sample 1, so the
            // rest starts one sample in. A null channel must stay null there (read as silence),
            // not become a pointer one sample past null.
            p.start({true, true}, true);
            EventList many(OP_MAX_EVENTS + 64);
            for (int32 k = 0; k < static_cast<int32>(OP_MAX_EVENTS); ++k) {
                Event on = noteOn(0, static_cast<int16>(k % 128), 0.5f);
                on.noteOn.channel = static_cast<int16>((k / 128) % 16);
                CHECK(many.addEvent(on) == kResultOk);
            }
            ParameterChanges cc;
            for (int16 controller = 1; controller <= 16; ++controller) point(cc, mapped(mapping, 0, controller), 0, 0.5);
            g_nullSide = true;
            r = process(p, kBlock, &in, &side, 2, true, &cc, &many);
            g_nullSide = false;
            for (int s = 1; s < kBlock; ++s) CHECK(std::isfinite(r.out[0][s]) && std::isfinite(r.out[1][s]));
            std::printf("VST3 events: an active aux bus with null channel pointers, in a block split one sample in, reads as silence\n");
        }

        // Channel order. The stub passes channel i only if it holds the tag of the
        // speaker the layout names for i, so a wrong mapping shows as -1.
        {
            struct Case {
                SpeakerArrangement arrangement;
                uint32_t layout;
                const char* name;
            };
            const Case cases[] = {{kMono, 2, "mono"}, {kStereo, 1, "stereo"}, {k51, 3, "5.1"}, {k71Music, 4, "7.1"}, {k71_4, 5, "7.1.4"}};
            for (const Case& cs : cases) {
                CHECK(p.arrange({cs.arrangement, kEmpty}, {cs.arrangement}) == kResultTrue);
                p.start({true, false}, true);
                const int channels = SpeakerArr::getChannelCount(cs.arrangement);
                // Host channel j carries the speaker with the j-th lowest bit.
                Audio in;
                for (int bit = 0; bit < 64; ++bit)
                    if (cs.arrangement & (1ull << bit)) in.push_back(std::vector<float>(kBlock, 0.01f * (static_cast<float>(bit) + 1.0f)));
                CHECK(static_cast<int>(in.size()) == channels);
                ParameterChanges mode;
                point(mode, 0, 0, 1.0);
                Result r = process(p, kBlock, &in, nullptr, channels, true, &mode);
                for (int c = 0; c < channels; ++c)
                    for (int s = 0; s < kBlock; ++s) CHECK(near(r.out[c][s], in[c][0]));
                CHECK(p.probe().layout == cs.layout);
                if (cs.arrangement == k71Music) {
                    // The same tags in the SDK's order (FL FR FC LFE SL SR BL BR) are wrong for VST3's order: the stub must see it.
                    const int canonical[] = {0, 1, 2, 3, 9, 10, 4, 5};
                    Audio wrong;
                    for (int role : canonical) wrong.push_back(std::vector<float>(kBlock, 0.01f * (static_cast<float>(role) + 1.0f)));
                    Result bad = process(p, kBlock, &wrong, nullptr, channels, true);
                    bool flagged = false;
                    for (int c = 0; c < channels; ++c) flagged = flagged || bad.out[c][0] == -1.0f;
                    CHECK(flagged);
                }
                ParameterChanges back;
                point(back, 0, 0, 0.0);
                process(p, kBlock, &in, nullptr, channels, true, &back);
                std::printf("VST3 events: %s: %d channels, every role arrives at its canonical index and returns to its host channel\n", cs.name, channels);
            }
        }

        // Offline.
        {
            CHECK(p.arrange({kStereo, kEmpty}, {kStereo}) == kResultTrue);
            p.start({true, false}, true);
            const Audio in = constant(2, kBlock, 0.0f);
            process(p, kBlock, &in, nullptr, 2, true, nullptr, nullptr, nullptr, kOffline);
            CHECK(p.probe().offline == 1);
            process(p, kBlock, &in, nullptr, 2, true, nullptr, nullptr, nullptr, kRealtime);
            CHECK(p.probe().offline == 0);
            std::printf("VST3 events: kOffline reaches the plugin as the offline flag, and clears with kRealtime\n");
        }

        // An arrangement set while the processor is active is refused and changes nothing.
        // (Accepted, 5.1 over buffers made for stereo overran the bypass buffers.)
        {
            std::vector<SpeakerArrangement> wide{k51, kEmpty}, wideOut{k51};
            CHECK(p.active && p.processor->setBusArrangements(wide.data(), 2, wideOut.data(), 1) == kResultFalse);
            CHECK(p.arrangementOf(kInput, 0) == kStereo && p.arrangementOf(kOutput, 0) == kStereo);
            const Audio in = constant(2, kBlock, 0.0f);
            process(p, kBlock, &in, nullptr, 2, true);
            CHECK(p.probe().layout == 1 && p.probe().output_channels == 2);
            std::printf("VST3 events: setBusArrangements(5.1) on an active stereo processor is refused; it stays stereo\n");
        }

        // Parameter values cross the ABI only when they may differ from the engine's. Every
        // probe is a getState, which hands the engine the values itself (one call for the one
        // parameter) and makes the next block push them all once more.
        {
            const Audio in = constant(2, kBlock, 0.0f);
            const uint32_t before = p.probe().param_sets;
            for (int block = 0; block < 64; ++block) process(p, kBlock, &in, nullptr, 2, true);
            const uint32_t steady = p.probe().param_sets;
            CHECK(steady - before == 2);  // the block after the probe, and this probe: not one per block
            // A point reaches the engine as an event. The next block pushes its value again, in
            // case the engine skipped the block that carried it; the blocks after push nothing.
            ParameterChanges same;
            point(same, 0, 0, 0.0);
            process(p, kBlock, &in, nullptr, 2, true, &same);
            for (int block = 0; block < 8; ++block) process(p, kBlock, &in, nullptr, 2, true);
            CHECK(p.probe().param_sets - steady == 3);  // the block after the probe, the block after the point, this probe
            std::printf("VST3 events: 64 steady blocks make %u parameter calls into the engine (one per block before); a point is pushed again once\n", steady - before);
        }

        // A state restored while inactive is loaded into a scratch engine, which the next
        // activation adopts when it was made for the same rate, block size and layout.
        {
            const Audio in = constant(2, kBlock, 0.0f);
            MemoryStream saved;
            CHECK(p.component->getState(&saved) == kResultOk);
            const auto restoreInactive = [&] {
                p.stop();
                saved.seek(0, IBStream::kIBSeekSet, nullptr);
                CHECK(p.component->setState(&saved) == kResultOk);
            };
            const uint32_t before = p.probe().creates;
            restoreInactive();
            p.start({true, false}, true);
            process(p, kBlock, &in, nullptr, 2, true);
            const uint32_t adopted = p.probe().creates;
            CHECK(adopted - before == 1);
            // Another block size: the scratch engine does not fit and a new one is made.
            restoreInactive();
            ProcessSetup half{kRealtime, kSample32, kBlock / 2, kRate};
            CHECK(p.processor->setupProcessing(half) == kResultOk);
            CHECK(p.component->setActive(true) == kResultOk && p.processor->setProcessing(true) == kResultOk);
            p.active = true;
            process(p, kBlock / 2, &in, nullptr, 2, true);
            CHECK(p.probe().creates - adopted == 2);
            std::printf("VST3 events: setState before setActive builds one engine, not two; a changed block size builds a new one\n");
        }
        mapping->release();
    }

    // ---- instrument ---------------------------------------------------------------------------
    {
        Plug p(argv[2]);
        CHECK(p.category == "Instrument|Synth");
        CHECK(p.component->getBusCount(kAudio, kInput) == 0);
        CHECK(p.component->getBusCount(kAudio, kOutput) == 1);
        CHECK(p.component->getBusCount(kEvent, kInput) == 1);
        using namespace SpeakerArr;
        CHECK(p.arrange({}, {kStereo}) == kResultTrue);
        CHECK(p.arrange({}, {k51}) == kResultTrue);
        CHECK(p.arrange({}, {kMono}) != kResultTrue);
        CHECK(p.arrangementOf(kOutput, 0) == k51);
        CHECK(p.arrange({}, {kStereo}) == kResultTrue);
        IMidiMapping* mapping = nullptr;
        CHECK(p.controller->queryInterface(IMidiMapping::iid, reinterpret_cast<void**>(&mapping)) == kResultOk);
        p.start({}, true);
        check_notes_and_midi(p, mapping, 2, false);
        mapping->release();
        CHECK(p.probe().input_channels == 0);
        std::printf("VST3 events: instrument category %s, no audio input bus, output stereo or 5.1, plays notes\n", p.category.c_str());
    }

    // ---- note effect: events only ---------------------------------------------------------------
    {
        Plug p(argv[3]);
        CHECK(p.category == "Fx");
        CHECK(p.component->getBusCount(kAudio, kInput) == 0 && p.component->getBusCount(kAudio, kOutput) == 0);
        CHECK(p.component->getBusCount(kEvent, kInput) == 1 && p.component->getBusCount(kEvent, kOutput) == 1);
        CHECK(p.arrange({}, {}) == kResultTrue);
        IMidiMapping* mapping = nullptr;
        CHECK(p.controller->queryInterface(IMidiMapping::iid, reinterpret_cast<void**>(&mapping)) == kResultOk);
        p.start({}, false);
        check_notes_and_midi(p, mapping, 0, false);
        mapping->release();
        const EventsProbe probe = p.probe();
        CHECK(probe.input_channels == 0 && probe.output_channels == 0);
        std::printf("VST3 events: note effect category %s, no audio buses, notes in and out\n", p.category.c_str());
    }
    std::printf("VST3 events proof: ok\n");
    return 0;
}
