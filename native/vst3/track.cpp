// SPDX-License-Identifier: MIT
// The host's track in the VST3 shell. The shell is built over track_stub.cpp, a
// plugin that records what op_shell_set_track_info tells it, and a host like a
// DAW sends the channel context through the interface it queries on the edit
// controller, Vst::ChannelContext::IInfoListener::setChannelContextInfos.
//
// Proved:
//  - the controller offers IInfoListener;
//  - the name (UTF-16, with a character outside the BMP) and the colour
//    (ARGB, as the host sends it) reach the plugin as UTF-8 and 0xRRGGBBAA with
//    the colour flag, once an engine exists (setActive), on the main thread;
//  - a later call that lists only the name, or only the colour, keeps the other;
//  - a NULL list is refused and delivers nothing;
//  - a new engine (deactivate, activate) is told again;
//  - what a host says before it connects the controller to the processor is
//    delivered when it does;
//  - a controller that is not connected at all takes the call without harm.
//
//   ctest --test-dir build-shell -R vst3_track_proof --output-on-failure
#include "op_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstchannelcontextinfo.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 track proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

namespace {
using CountFn = uint32_t (*)(void);
using CallFn = uint32_t (*)(uint32_t, char*, uint32_t, uint32_t*, uint32_t*, uint32_t*, uint32_t*);
using ResetFn = void (*)(void);
CountFn stubCount = nullptr;
CallFn stubCall = nullptr;
ResetFn stubReset = nullptr;

struct Call {
    std::string name;
    bool hasName = false;
    uint32_t rgba = 0, flags = 0;
    bool mainThread = false;
};
Call callAt(uint32_t n) {
    Call c;
    char name[300] = {};
    uint32_t has = 0, mainThread = 0;
    CHECK(stubCall(n, name, sizeof(name), &has, &c.rgba, &c.flags, &mainThread) == 1);
    c.name = name;
    c.hasName = has != 0;
    c.mainThread = mainThread != 0;
    return c;
}

// What a host sends: a list with a name and/or a colour (ARGB, as VST3 defines it).
struct Info {
    IPtr<IAttributeList> list = HostAttributeList::make();
    Info& name(const char16_t* text) {
        CHECK(list->setString(ChannelContext::kChannelNameKey, reinterpret_cast<const TChar*>(text)) == kResultOk);
        return *this;
    }
    Info& color(uint32_t argb) {
        CHECK(list->setInt(ChannelContext::kChannelColorKey, static_cast<int64>(argb)) == kResultOk);
        return *this;
    }
};

struct Instance {
    HostApplication* host;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    IConnectionPoint* fromComponent = nullptr;
    IConnectionPoint* fromController = nullptr;
    bool active = false;

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
        ProcessSetup setup{kRealtime, kSample32, 1024, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromComponent)) == kResultOk);
        CHECK(controller->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromController)) == kResultOk);
    }
    void connect() {
        CHECK(fromComponent->connect(fromController) == kResultOk && fromController->connect(fromComponent) == kResultOk);
    }
    void activate() {
        CHECK(component->setActive(true) == kResultOk);
        active = true;
    }
    void deactivate() {
        CHECK(component->setActive(false) == kResultOk);
        active = false;
    }
    tresult tell(Info& info) {
        FUnknownPtr<ChannelContext::IInfoListener> listener(controller);
        CHECK(listener != nullptr);
        return listener->setChannelContextInfos(info.list);
    }
    ~Instance() {
        if (active) deactivate();
        fromComponent->disconnect(fromController);
        fromController->disconnect(fromComponent);
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
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    CHECK(bundle && CFBundleLoadExecutable(bundle));
    auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
    auto exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
    stubCount = reinterpret_cast<CountFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_track_count")));
    stubCall = reinterpret_cast<CallFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_track_call")));
    stubReset = reinterpret_cast<ResetFn>(CFBundleGetFunctionPointerForName(bundle, CFSTR("op_stub_track_reset")));
    CHECK(entry && exitFn && getFactory && stubCount && stubCall && stubReset && entry(bundle));
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
        // 1. Name and colour reach the plugin, as UTF-8 and 0xRRGGBBAA, once an engine exists.
        {
            Instance a(factory, effectId, host);
            a.connect();
            stubReset();
            Info first;
            first.name(u"Vocals ♪ \U0001F600").color(0xFF336699u);
            CHECK(a.tell(first) == kResultOk);
            CHECK(stubCount() == 0);  // no engine yet: kept until setActive
            a.activate();
            CHECK(stubCount() == 1);
            Call c = callAt(0);
            CHECK(c.hasName && c.name == "Vocals \xE2\x99\xAA \xF0\x9F\x98\x80");
            CHECK(c.rgba == 0x336699FFu);
            CHECK(c.flags == OP_TRACK_HAS_COLOUR);
            CHECK(c.mainThread);
            std::printf("VST3 track: name \"%s\" and colour %08X reach the plugin\n", c.name.c_str(), c.rgba);
            // 2. A later call with only the name keeps the colour; only the colour keeps the name.
            Info renamed;
            renamed.name(u"Bass");
            CHECK(a.tell(renamed) == kResultOk);
            CHECK(stubCount() == 2);
            c = callAt(1);
            CHECK(c.hasName && c.name == "Bass" && c.rgba == 0x336699FFu && c.flags == OP_TRACK_HAS_COLOUR);
            Info recoloured;
            recoloured.color(0x80FF0000u);
            CHECK(a.tell(recoloured) == kResultOk);
            CHECK(stubCount() == 3);
            c = callAt(2);
            CHECK(c.name == "Bass" && c.rgba == 0xFF000080u && c.flags == OP_TRACK_HAS_COLOUR && c.mainThread);
            // 3. A NULL list is refused and delivers nothing.
            FUnknownPtr<ChannelContext::IInfoListener> listener(a.controller);
            CHECK(listener && listener->setChannelContextInfos(nullptr) == kInvalidArgument);
            CHECK(stubCount() == 3);
            // 4. A new engine is told what the host last said.
            a.deactivate();
            a.activate();
            CHECK(stubCount() == 4);
            c = callAt(3);
            CHECK(c.name == "Bass" && c.rgba == 0xFF000080u);
            std::printf("VST3 track: later calls merge, a NULL list is refused, a new engine is told again\n");
        }
        // 5. What a host says before it connects the two halves is delivered when it does.
        {
            Instance b(factory, effectId, host);
            stubReset();
            Info early;
            early.name(u"Early").color(0xFF010203u);
            CHECK(b.tell(early) == kResultOk);
            b.connect();
            b.activate();
            CHECK(stubCount() == 1);
            const Call c = callAt(0);
            CHECK(c.name == "Early" && c.rgba == 0x010203FFu && c.flags == OP_TRACK_HAS_COLOUR);
            std::printf("VST3 track: told before connect, delivered on connect\n");
        }
        // 6. A controller that is never connected takes the call and delivers nothing.
        {
            Instance d(factory, effectId, host);
            stubReset();
            Info lonely;
            lonely.name(u"Alone");
            CHECK(d.tell(lonely) == kResultOk);
            d.activate();
            CHECK(stubCount() == 0);
            std::printf("VST3 track: an unconnected controller takes the call without harm\n");
        }
        // 7. A colour alone: no name, so the plugin hears none.
        {
            Instance e(factory, effectId, host);
            e.connect();
            stubReset();
            Info colour;
            colour.color(0xFFAABBCCu);
            CHECK(e.tell(colour) == kResultOk);
            e.activate();
            CHECK(stubCount() == 1);
            const Call c = callAt(0);
            CHECK(!c.hasName && c.rgba == 0xAABBCCFFu && c.flags == OP_TRACK_HAS_COLOUR);
        }
    }
    CHECK(exitFn());
    std::printf("VST3 track proof passed\n");
    return 0;
}
