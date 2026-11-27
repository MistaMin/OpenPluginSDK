// SPDX-License-Identifier: MIT
// Telling a VST3 host about changes (shell ABI 6). The shell is built over
// changes_stub.cpp, whose latency follows its parameter (256 x the normalized
// value) the way the Rust library's does. The host here is a DAW's minimum: a
// HostApplication (so the two halves can message each other), connected
// halves, and an IComponentHandler that records every restartComponent and
// does what a host does with kLatencyChanged: deactivate, activate, read
// getLatencySamples.
//
// Proved:
//  - a parameter that moves the latency, sent with a block, reaches the host
//    as one restartComponent(kLatencyChanged), on the main thread, from the
//    processor's main-thread timer (the poll never runs on the audio thread);
//  - after the restart the host reads the new latency back, and the new engine
//    the restart made has nothing more to report (no restart loop);
//  - blocks with an unchanged latency ask nothing;
//  - new parameter info reaches the host as kParamTitlesChanged |
//    kParamValuesChanged, once, and the host then reads the renamed table
//    (ABI 9): getParameterInfo has the new title and unit and
//    getParamStringByValue the new text, and typed text is read by it; before
//    the rename, and on a controller whose halves were never connected (no
//    engine to read), the first table stays (the negative controls);
//  - a component that is never connected to its controller, or a controller
//    with no handler, takes the change without harm.
//
//   ctest --test-dir build-shell -R vst3_changes_proof --output-on-failure
#include "op_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <pthread.h>
#include <string>
#include <thread>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 changes proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

namespace {
CFBundleRef gBundle = nullptr;
using RenameFn = uint32_t (*)(void);
// UTF-16 to ASCII, for the proof's own strings.
std::string ascii(const TChar* text) {
    std::string out;
    for (size_t i = 0; text && text[i]; ++i) out += static_cast<char>(text[i] < 0x80 ? text[i] : '?');
    return out;
}
// What a host reads for parameter 0 (ID 0): its title, its unit, and the text of `normalized`.
std::string title0(IEditController* controller) {
    ParameterInfo info{};
    CHECK(controller->getParameterInfo(0, info) == kResultOk && info.id == 0);
    return ascii(info.title);
}
std::string units0(IEditController* controller) {
    ParameterInfo info{};
    CHECK(controller->getParameterInfo(0, info) == kResultOk && info.id == 0);
    return ascii(info.units);
}
std::string text0(IEditController* controller, ParamValue normalized) {
    String128 text{};
    CHECK(controller->getParamStringByValue(0, normalized, text) == kResultOk);
    return ascii(text);
}
using TakesFn = void (*)(uint32_t*, uint32_t*);
using EnginesFn = uint32_t (*)(void);
RenameFn stubRename = nullptr;
TakesFn stubTakes = nullptr;
EnginesFn stubEngines = nullptr;

struct Handler final : IComponentHandler, IComponentHandler2 {
    std::vector<int32> restarts;
    int dirty = 0;
    tresult PLUGIN_API setDirty(TBool state) override {
        if (state) ++dirty;
        if (!pthread_main_np()) ++offMain;
        return kResultOk;
    }
    tresult PLUGIN_API requestOpenEditor(FIDString) override { return kResultOk; }
    tresult PLUGIN_API startGroupEdit() override { return kResultOk; }
    tresult PLUGIN_API finishGroupEdit() override { return kResultOk; }
    int offMain = 0;
    tresult PLUGIN_API beginEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API performEdit(ParamID, ParamValue) override { return kResultOk; }
    tresult PLUGIN_API endEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API restartComponent(int32 flags) override {
        restarts.push_back(flags);
        if (!pthread_main_np()) ++offMain;
        return kResultOk;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IComponentHandler::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = static_cast<IComponentHandler*>(this);
            return kResultOk;
        }
        if (FUnknownPrivate::iidEqual(iid, IComponentHandler2::iid)) {
            *obj = static_cast<IComponentHandler2*>(this);
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

// Run the main run loop (the processor's timer fires there) until `done` or two seconds.
template <class Done>
bool pump_until(Done done) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline) return false;
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
    }
    return true;
}
// Run the main run loop for `ms` milliseconds, to see that nothing more arrives.
void pump_for(int ms) {
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    while (std::chrono::steady_clock::now() < until) CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
}

struct Instance {
    HostApplication* host;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    IConnectionPoint* fromComponent = nullptr;
    IConnectionPoint* fromController = nullptr;
    bool active = false;
    bool connected = false;

    Instance(IPluginFactory* factory, const TUID effectId, HostApplication* h) : host(h) {
        CHECK(factory->createInstance(effectId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(host) == kResultOk);
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(host) == kResultOk);
        SpeakerArrangement arrangement = SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, 256, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromComponent)) == kResultOk);
        CHECK(controller->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromController)) == kResultOk);
    }
    void connect() {
        CHECK(fromComponent->connect(fromController) == kResultOk && fromController->connect(fromComponent) == kResultOk);
        connected = true;
    }
    void activate() {
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        active = true;
    }
    void deactivate() {
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        active = false;
    }
    // One block of 256 frames, with the parameter at `lookahead` (normalized) when given.
    void block(const double* lookahead = nullptr) {
        constexpr int32 frames = 256;
        std::vector<float> left(frames, 0.25f), right(frames, 0.25f);
        float* channels[] = {left.data(), right.data()};
        AudioBusBuffers in{}, out{};
        in.numChannels = out.numChannels = 2;
        in.channelBuffers32 = out.channelBuffers32 = channels;
        ParameterChanges changes;
        if (lookahead) {
            int32 index = 0;
            auto* queue = changes.addParameterData(0, index);
            queue->addPoint(0, *lookahead, index);
        }
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = frames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &in;
        data.outputs = &out;
        data.inputParameterChanges = lookahead ? &changes : nullptr;
        // The block runs on its own thread, as a host's audio thread does.
        tresult result = kResultFalse;
        std::thread audio([&] { result = processor->process(data); });
        audio.join();
        CHECK(result == kResultOk);
    }
    // What a host does with kLatencyChanged.
    uint32 restart() {
        deactivate();
        activate();
        return processor->getLatencySamples();
    }
    ~Instance() {
        if (active) deactivate();
        if (connected) {
            fromComponent->disconnect(fromController);
            fromController->disconnect(fromComponent);
        }
        fromComponent->release();
        fromController->release();
        processor->release();
        controller->terminate();
        controller->release();
        component->terminate();
        component->release();
    }
};
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    CHECK(pthread_main_np());
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    CHECK(bundle && CFBundleLoadExecutable(bundle));
    gBundle = bundle;
    auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
    auto exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
    stubRename = reinterpret_cast<RenameFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_changes_rename")));
    stubTakes = reinterpret_cast<TakesFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_changes_takes")));
    stubEngines = reinterpret_cast<EnginesFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_changes_engines")));
    CHECK(entry && exitFn && getFactory && stubRename && stubTakes && stubEngines && entry(bundle));
    IPluginFactory* factory = getFactory();
    CHECK(factory);
    TUID effectId{};
    bool found = false;
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo info{};
        CHECK(factory->getClassInfo(i, &info) == kResultOk);
        if (std::strcmp(info.category, kVstAudioEffectClass) == 0) {
            std::memcpy(effectId, info.cid, 16);
            found = true;
        }
    }
    CHECK(found);
    {
        IPtr<HostApplication> host = owned(new HostApplication);
        // 1. A latency parameter mid-session: one kLatencyChanged, on the main thread.
        {
            Instance a(factory, effectId, host);
            Handler handler;
            CHECK(a.controller->setComponentHandler(&handler) == kResultOk);
            a.connect();
            a.activate();
            CHECK(a.processor->getLatencySamples() == 0);
            for (int i = 0; i < 4; ++i) a.block();
            pump_for(120);
            CHECK(handler.restarts.empty());
            const double half = 0.5;
            a.block(&half);
            for (int i = 0; i < 8; ++i) a.block();
            CHECK(pump_until([&] { return !handler.restarts.empty(); }));
            pump_for(120);
            CHECK(handler.restarts.size() == 1);
            CHECK(handler.restarts[0] == kLatencyChanged);
            CHECK(handler.offMain == 0);
            const uint32_t enginesBefore = stubEngines();
            const uint32 latency = a.restart();
            CHECK(latency == 128);
            CHECK(stubEngines() == enginesBefore + 1);  // the restart built a new engine
            std::printf("VST3 changes: lookahead 0.5 -> one restartComponent(kLatencyChanged) on the main thread, latency read back %u\n", latency);
            // The new engine came up with the latency the host read: nothing more, even while playing.
            for (int i = 0; i < 4; ++i) a.block();
            pump_for(150);
            CHECK(handler.restarts.size() == 1);
            // 2. New parameter info: titles and values, once, then the renamed table.
            CHECK(title0(a.controller) == "Lookahead");
            const std::string before = text0(a.controller, 0.5);
            CHECK(before.rfind("0.50 ", 0) == 0);  // the stand-ins' first-table text
            CHECK(stubRename() == 1);
            CHECK(pump_until([&] { return handler.restarts.size() == 2; }));
            pump_for(120);
            CHECK(handler.restarts.size() == 2);
            CHECK(handler.restarts[1] == (kParamTitlesChanged | kParamValuesChanged));
            CHECK(title0(a.controller) == "Lookahead (renamed)");
            CHECK(units0(a.controller) == "smp");
            CHECK(text0(a.controller, 0.5) == "128 smp (renamed)");
            {
                ParamValue typed = -1.0;
                CHECK(a.controller->getParamValueByString(0, const_cast<TChar*>(STR16("64 smp")), typed) == kResultOk && typed == 0.25);
            }
            // Negative control: the library's call without an instance still reads the first table.
            {
                using NameFn = uint32_t (*)(char*, uint32_t);
                auto typeName = reinterpret_cast<NameFn>(CFBundleGetFunctionPointerForName(gBundle, CFSTR("op_stub_changes_type_name")));
                char name[64] = {};
                CHECK(typeName && typeName(name, sizeof(name)) && std::strcmp(name, "Lookahead") == 0);
            }
            std::printf("VST3 changes: renamed table -> getParameterInfo \"%s\" [%s], getParamStringByValue(0.5) \"%s\" (was \"Lookahead\", \"%s\")\n",
                        title0(a.controller).c_str(), units0(a.controller).c_str(), text0(a.controller, 0.5).c_str(), before.c_str());
            // 3. Both at once arrive as one call.
            const double quarter = 0.25;
            a.block(&quarter);
            CHECK(stubRename() == 1);
            CHECK(pump_until([&] { return handler.restarts.size() == 3; }));
            pump_for(120);
            CHECK(handler.restarts.size() == 3);
            CHECK(handler.restarts[2] == (kLatencyChanged | kParamTitlesChanged | kParamValuesChanged));
            CHECK(a.restart() == 64);
            CHECK(handler.offMain == 0);
            // 4. Saved state that is not a parameter changed: one setDirty, and no restart.
            auto stubDirty = reinterpret_cast<RenameFn>(CFBundleGetFunctionPointerForName(gBundle, CFSTR("op_stub_changes_dirty")));
            CHECK(stubDirty && handler.dirty == 0);
            for (int i = 0; i < 4; ++i) a.block();
            CHECK(stubDirty() == 1);
            CHECK(pump_until([&] { return handler.dirty == 1; }));
            pump_for(120);
            CHECK(handler.dirty == 1 && handler.restarts.size() == 3 && handler.offMain == 0);
            uint32_t mainTakes = 0, otherTakes = 0;
            stubTakes(&mainTakes, &otherTakes);
            CHECK(mainTakes > 0 && otherTakes == 0);  // the poll never ran on the audio thread
            std::printf("VST3 changes: new parameter info -> kParamTitlesChanged | kParamValuesChanged once; %u polls, all on the main thread\n", mainTakes);
            CHECK(a.controller->setComponentHandler(nullptr) == kResultOk);
        }
        // 4. A controller with no handler, and halves that are never connected, take it quietly.
        {
            Instance b(factory, effectId, host);
            b.connect();
            b.activate();
            const double full = 1.0;
            b.block(&full);
            pump_for(120);
            CHECK(b.restart() == 256);
        }
        {
            Instance c(factory, effectId, host);
            Handler handler;
            CHECK(c.controller->setComponentHandler(&handler) == kResultOk);
            c.activate();
            const double some = 0.125;
            c.block(&some);
            pump_for(120);
            CHECK(handler.restarts.empty());
            // Negative control: a rename the controller never hears of, with no engine to read.
            CHECK(stubRename() == 1);
            pump_for(120);
            CHECK(handler.restarts.empty() && title0(c.controller) == "Lookahead" && text0(c.controller, 0.5).rfind("0.50 ", 0) == 0);
            CHECK(c.controller->setComponentHandler(nullptr) == kResultOk);
            std::printf("VST3 changes: no handler, or unconnected halves, take the change without harm\n");
        }
    }
    CHECK(exitFn());
    std::printf("VST3 changes proof passed\n");
    return 0;
}
