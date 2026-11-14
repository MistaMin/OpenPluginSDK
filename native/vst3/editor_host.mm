// SPDX-License-Identifier: MIT
// VST3EditorProof: loads a VST3 bundle built with -DOP_EDITOR=ON as a host
// does and checks the editor the shell hosts through op_editor_*:
//
//   - the view supports NSView, has a size, and attaches one subview;
//   - the processor can be deactivated and activated again with the editor
//     open (the shell re-binds the editor to the new engine);
//   - optionally, a click at a point produces paired begin/perform/end edits
//     on one parameter, and the controller then holds the value it reported;
//   - removed() takes the view out, and tearing down with the processor
//     first, while a second view is open, does not crash.
//
//   VST3EditorProof <bundle.vst3> [x y param]     (x, y in points from the top left)
//
// Objective-C++: the VST3 interfaces are C++ classes, and the view is AppKit.
#import <Cocoa/Cocoa.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <initializer_list>

using namespace Steinberg;
using namespace Steinberg::Vst;

#define CHECK(cond)                                                    \
    do {                                                               \
        if (!(cond)) {                                                 \
            std::printf("VST3 editor proof failed line %d: %s\n", __LINE__, #cond); \
            return 1;                                                  \
        }                                                              \
    } while (0)

// The host side of parameter editing: records what the controller reports.
struct Handler final : IComponentHandler {
    int begins = 0, performs = 0, ends = 0;
    ParamID last = 0xffffffff;
    ParamValue value = -1.0;
    tresult PLUGIN_API beginEdit(ParamID) override { ++begins; return kResultOk; }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue v) override {
        ++performs;
        last = id;
        value = v;
        return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID) override { ++ends; return kResultOk; }
    tresult PLUGIN_API restartComponent(int32) override { return kResultOk; }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IComponentHandler::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = this;
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

static void spin(double seconds) { CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, false); }

int main(int argc, char** argv) {
    if (argc != 2 && argc != 5) {
        std::printf("usage: VST3EditorProof <bundle.vst3> [x y param]\n");
        return 2;
    }
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSBundle* bundle = [NSBundle bundleWithPath:[NSString stringWithUTF8String:argv[1]]];
        CHECK(bundle && [bundle load]);
        CFBundleRef cf = CFBundleCreate(nullptr, (CFURLRef)[bundle bundleURL]);
        auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(cf, CFSTR("bundleEntry")));
        auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(cf, CFSTR("GetPluginFactory")));
        CHECK(entry && getFactory && entry(cf));
        IPluginFactory* factory = getFactory();
        CHECK(factory);
        PClassInfo effect{};
        bool found = false;
        for (int32 i = 0; i < factory->countClasses(); ++i) {
            PClassInfo info{};
            if (factory->getClassInfo(i, &info) == kResultOk && std::strcmp(info.category, kVstAudioEffectClass) == 0) {
                effect = info;
                found = true;
            }
        }
        CHECK(found);
        IComponent* component = nullptr;
        CHECK(factory->createInstance(effect.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        IAudioProcessor* processor = nullptr;
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk);
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        IEditController* controller = nullptr;
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        Handler handler;
        CHECK(controller->setComponentHandler(&handler) == kResultOk);

        // Connect the two halves, as a host does; the editor finds its engine through this.
        IConnectionPoint* fromComponent = nullptr;
        IConnectionPoint* fromController = nullptr;
        CHECK(component->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromComponent)) == kResultOk);
        CHECK(controller->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromController)) == kResultOk);
        CHECK(fromComponent->connect(fromController) == kResultOk && fromController->connect(fromComponent) == kResultOk);

        SpeakerArrangement stereo = SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&stereo, 1, &stereo, 1) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, 512, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        std::printf("tail %u samples\n", processor->getTailSamples());

        IPlugView* view = controller->createView(ViewType::kEditor);
        CHECK(view);
        CHECK(view->isPlatformTypeSupported(kPlatformTypeNSView) == kResultTrue);
        ViewRect rect{};
        CHECK(view->getSize(&rect) == kResultOk && rect.getWidth() > 0 && rect.getHeight() > 0);
        std::printf("editor %dx%d\n", rect.getWidth(), rect.getHeight());
        NSRect frame = NSMakeRect(0, 0, rect.getWidth(), rect.getHeight());
        NSWindow* window = [[NSWindow alloc] initWithContentRect:frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        NSView* parent = [window contentView];
        CHECK(view->attached(parent, kPlatformTypeNSView) == kResultOk);
        CHECK(parent.subviews.count == 1);
        NSView* editor = parent.subviews[0];
        spin(0.2);

        // The engine is destroyed and made again with the editor open.
        CHECK(component->setActive(false) == kResultOk);
        spin(0.1);
        CHECK(component->setActive(true) == kResultOk);
        spin(0.1);

        if (argc == 5) {
            const double x = std::atof(argv[2]);
            const double y = std::atof(argv[3]);
            const ParamID param = static_cast<ParamID>(std::atoi(argv[4]));
            NSPoint at = NSMakePoint(x, frame.size.height - y);
            for (NSEventType type : {NSEventTypeLeftMouseDown, NSEventTypeLeftMouseUp}) {
                NSEvent* click = [NSEvent mouseEventWithType:type location:at modifierFlags:0 timestamp:0 windowNumber:window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:1];
                if (type == NSEventTypeLeftMouseDown) [editor mouseDown:click];
                else [editor mouseUp:click];
            }
            spin(0.1);
            std::printf("click: %d begin / %d perform / %d end, parameter %u = %.4f\n", handler.begins, handler.performs, handler.ends, handler.last, handler.value);
            CHECK(handler.begins > 0 && handler.begins == handler.ends && handler.performs >= handler.begins);
            CHECK(handler.last == param);
            CHECK(controller->getParamNormalized(param) == handler.value);
        }

        CHECK(view->removed() == kResultOk);
        CHECK(parent.subviews.count == 0);
        view->release();
        // Open once more, then tear down with the processor going first.
        view = controller->createView(ViewType::kEditor);
        CHECK(view && view->attached(parent, kPlatformTypeNSView) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        fromComponent->disconnect(fromController);
        fromController->disconnect(fromComponent);
        fromComponent->release();
        fromController->release();
        processor->release();
        CHECK(component->terminate() == kResultOk);
        component->release();
        spin(0.2);
        CHECK(view->removed() == kResultOk);
        view->release();
        CHECK(controller->terminate() == kResultOk);
        controller->release();
        [window release];
        std::printf("VST3 editor proof passed\n");
    }
    return 0;
}
