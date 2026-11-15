// SPDX-License-Identifier: MIT
// VST3EditorProof: loads a VST3 bundle built with -DOP_EDITOR=ON as a host
// does and checks the editor the shell hosts through op_editor_*:
//
//   - the view supports NSView, has a size, and attaches one subview;
//   - the processor can be deactivated and activated again with the editor
//     open (the shell re-binds the editor to the new engine);
//   - optionally, a click at a point produces paired begin/perform/end edits
//     on one parameter, and the controller then holds the value it reported;
//   - the view resizes as a host resizes it: canResize, checkSizeConstraint
//     (clamped to the editor's range and shape), onSize, getSize, the NSView's
//     frame, and a click at the scaled position of the Warm toggle edits the
//     parameter while the old position hits nothing;
//   - an editor-initiated size (double-click on the bare plate) reaches
//     IPlugFrame::resizeView, and the view takes the size the frame grants;
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

// The host's frame: records the request and grants it, as hosts do by calling onSize.
struct Frame final : IPlugFrame {
    IPlugView* view = nullptr;
    int requests = 0;
    int32 width = 0, height = 0;
    bool grant = true;
    tresult PLUGIN_API resizeView(IPlugView* v, ViewRect* newSize) override {
        ++requests;
        width = newSize->getWidth();
        height = newSize->getHeight();
        if (!grant) return kResultFalse;
        return v->onSize(newSize);
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IPlugFrame::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = this;
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

static void click(NSView* editor, NSWindow* window, double height, double x, double y, int count) {
    NSPoint at = NSMakePoint(x, height - y);
    for (NSEventType type : {NSEventTypeLeftMouseDown, NSEventTypeLeftMouseUp}) {
        NSEvent* e = [NSEvent mouseEventWithType:type location:at modifierFlags:0 timestamp:0 windowNumber:window.windowNumber context:nil eventNumber:0 clickCount:count pressure:1];
        if (type == NSEventTypeLeftMouseDown) [editor mouseDown:e];
        else [editor mouseUp:e];
    }
}

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
        Frame plugFrame;
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

        // Resizing, as a host does. The range comes from the view itself.
        CHECK(view->canResize() == kResultTrue);
        ViewRect big(0, 0, 100000, 100000);
        CHECK(view->checkSizeConstraint(&big) == kResultOk);
        ViewRect small(0, 0, 1, 1);
        CHECK(view->checkSizeConstraint(&small) == kResultOk);
        std::printf("range %dx%d to %dx%d\n", small.getWidth(), small.getHeight(), big.getWidth(), big.getHeight());
        CHECK(big.getWidth() > rect.getWidth() && small.getWidth() < rect.getWidth());
        // A size off the shape is brought to the shape.
        ViewRect odd(0, 0, big.getWidth(), small.getHeight() + (big.getHeight() - small.getHeight()) / 2);
        CHECK(view->checkSizeConstraint(&odd) == kResultOk);
        CHECK(std::abs(odd.getWidth() * big.getHeight() - odd.getHeight() * big.getWidth()) <= big.getWidth());
        // The click point scales with the view: examples/editor keeps its shape from the origin.
        const double factor = double(big.getWidth()) / rect.getWidth();
        const double cx = argc == 5 ? std::atof(argv[2]) : 135.0;
        const double cy = argc == 5 ? std::atof(argv[3]) : 68.0;
        const ParamID cparam = argc == 5 ? static_cast<ParamID>(std::atoi(argv[4])) : 1;
        ViewRect to = big;
        CHECK(view->onSize(&to) == kResultOk);
        ViewRect now{};
        CHECK(view->getSize(&now) == kResultOk && now.getWidth() == big.getWidth() && now.getHeight() == big.getHeight());
        spin(0.05);
        CHECK(editor.frame.size.width == big.getWidth() && editor.frame.size.height == big.getHeight());
        std::printf("resized to %dx%d\n", now.getWidth(), now.getHeight());
        // The old position of the control hits nothing at this size...
        const int before = handler.begins;
        click(editor, window, big.getHeight(), cx, cy, 1);
        spin(0.05);
        CHECK(handler.begins == before);
        // ...and the scaled one hits it.
        click(editor, window, big.getHeight(), cx * factor, cy * factor, 1);
        spin(0.05);
        std::printf("scaled click: %d begin / %d end, parameter %u\n", handler.begins, handler.ends, handler.last);
        CHECK(handler.begins == before + 1 && handler.ends == handler.begins && handler.last == cparam);
        // Smaller than the range: the view stays at the smallest size.
        ViewRect tiny(0, 0, 10, 10);
        CHECK(view->onSize(&tiny) == kResultOk);
        CHECK(view->getSize(&now) == kResultOk && now.getWidth() == small.getWidth() && now.getHeight() == small.getHeight());
        // The editor asks for a size: a double-click on the bare plate.
        ViewRect open(0, 0, rect.getWidth(), rect.getHeight());
        CHECK(view->onSize(&open) == kResultOk);
        CHECK(view->setFrame(&plugFrame) == kResultOk);
        click(editor, window, rect.getHeight(), 5, 5, 2);
        spin(0.05);
        CHECK(plugFrame.requests == 1);
        CHECK(view->getSize(&now) == kResultOk && now.getWidth() == plugFrame.width && now.getHeight() == plugFrame.height);
        CHECK(plugFrame.width > rect.getWidth());
        CHECK(editor.frame.size.width == plugFrame.width && editor.frame.size.height == plugFrame.height);
        std::printf("editor asked for %dx%d, view is %dx%d\n", plugFrame.width, plugFrame.height, now.getWidth(), now.getHeight());
        // A frame that refuses leaves the size alone.
        plugFrame.grant = false;
        click(editor, window, now.getHeight(), 5, 5, 2);
        spin(0.05);
        CHECK(plugFrame.requests == 2);
        ViewRect after{};
        CHECK(view->getSize(&after) == kResultOk && after.getWidth() == now.getWidth() && after.getHeight() == now.getHeight());
        CHECK(view->setFrame(nullptr) == kResultOk);

        CHECK(view->removed() == kResultOk);
        CHECK(parent.subviews.count == 0);
        view->release();

        // Two views on one controller: the second replaces the first's editor.
        // Removing and releasing the first must leave the second's editor alone.
        {
            NSWindow* secondWindow = [[NSWindow alloc] initWithContentRect:frame styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
            NSView* secondParent = [secondWindow contentView];
            IPlugView* first = controller->createView(ViewType::kEditor);
            CHECK(first && first->attached(parent, kPlatformTypeNSView) == kResultOk);
            IPlugView* second = controller->createView(ViewType::kEditor);
            CHECK(second && second->attached(secondParent, kPlatformTypeNSView) == kResultOk);
            CHECK(secondParent.subviews.count == 1);
            CHECK(first->removed() == kResultOk);
            first->release();
            spin(0.05);
            CHECK(secondParent.subviews.count == 1);
            ViewRect size{};
            CHECK(second->getSize(&size) == kResultOk && size.getWidth() > 0);
            const int before = handler.begins;
            click(secondParent.subviews[0], secondWindow, size.getHeight(), cx * size.getWidth() / rect.getWidth(), cy * size.getHeight() / rect.getHeight(), 1);
            spin(0.05);
            std::printf("second view after the first closed: %d begin\n", handler.begins - before);
            CHECK(handler.begins == before + 1);
            CHECK(second->removed() == kResultOk);
            CHECK(secondParent.subviews.count == 0);
            second->release();
            [secondWindow release];
        }
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
