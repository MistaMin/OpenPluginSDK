// SPDX-License-Identifier: MIT
// AUEditorProof: loads an Audio Unit bundle built with -DOP_EDITOR=ON in this
// process, as a host does, and checks the Cocoa view the shell publishes:
//
//   - the unit answers kAudioUnitProperty_CocoaUI with a bundle and a factory
//     class, and the class makes a view of the editor's size inside a window;
//   - the unit can be uninitialized and initialized again with the view open
//     (the shell re-binds the editor to the new engine);
//   - a click at a point produces begin, value and end events for one
//     parameter, seen by an AUEventListener, and the unit's value changes;
//   - the view follows its frame as a host sets it: a larger frame resizes the
//     editor within its range and shape, a click at the scaled position of the
//     Warm toggle edits the parameter while the old position hits nothing, and
//     a frame outside the range is clamped for the editor;
//   - an editor-initiated size (double-click on the bare plate) changes the
//     container's frame, which is what a host observes to size its window;
//   - the view goes away, and tearing down with the unit first, while a second
//     view is open, does not crash.
//
//   AUEditorProof <bundle.component> [x y param]     (x, y in points from the top left)
//
// Without x y param it clicks the centre of the Warm toggle of examples/editor.
// Nothing is installed: the factory is registered in this process only.
// Objective-C++: the view is AppKit.
#import <AudioToolbox/AudioToolbox.h>
#import <AudioUnit/AUCocoaUIView.h>
#import <Cocoa/Cocoa.h>
#include "proof_au.h"

// Counts what the host's event listener is told.
struct Events {
    int begins = 0, ends = 0, values = 0;
    AudioUnitParameterID lastBegin = 0xffffffff, lastEnd = 0xffffffff, lastValue = 0xffffffff;
    AudioUnitParameterValue value = -1.f;
};

static void on_event(void* user, void*, const AudioUnitEvent* event, UInt64, AudioUnitParameterValue value) {
    auto* events = static_cast<Events*>(user);
    const AudioUnitParameterID id = event->mArgument.mParameter.mParameterID;
    switch (event->mEventType) {
    case kAudioUnitEvent_BeginParameterChangeGesture: ++events->begins; events->lastBegin = id; break;
    case kAudioUnitEvent_EndParameterChangeGesture: ++events->ends; events->lastEnd = id; break;
    case kAudioUnitEvent_ParameterValueChange: ++events->values; events->lastValue = id; events->value = value; break;
    default: break;
    }
}

static void spin(double seconds) { CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, false); }

// The factory class the unit names, from its bundle; the view it makes is retained.
static NSView* make_view(AudioUnit unit, NSSize size, const char** className) {
    AudioUnitCocoaViewInfo info{};
    UInt32 infoSize = sizeof(info);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_CocoaUI, kAudioUnitScope_Global, 0, &info, &infoSize) == noErr);
    CHECK(info.mCocoaAUViewBundleLocation && info.mCocoaAUViewClass[0]);
    static char name[256];
    CHECK(CFStringGetCString(info.mCocoaAUViewClass[0], name, sizeof(name), kCFStringEncodingUTF8));
    *className = name;
    NSBundle* bundle = [NSBundle bundleWithURL:(NSURL*)info.mCocoaAUViewBundleLocation];
    CHECK(bundle && [bundle load]);
    CFRelease(info.mCocoaAUViewBundleLocation);
    CFRelease(info.mCocoaAUViewClass[0]);
    Class factoryClass = NSClassFromString([NSString stringWithUTF8String:name]);
    CHECK(factoryClass && [factoryClass conformsToProtocol:@protocol(AUCocoaUIBase)]);
    NSObject<AUCocoaUIBase>* factory =[[factoryClass alloc] init];
    CHECK(factory && [factory interfaceVersion] == 0);
    NSView* view = [[factory uiViewForAudioUnit:unit withSize:size] retain];
    [factory release];
    return view;
}

int main(int argc, char** argv) {
    if (argc != 2 && argc != 5) {
        std::printf("usage: AUEditorProof <bundle.component> [x y param]\n");
        return 2;
    }
    @autoreleasepool {
        [NSApplication sharedApplication];
        AudioComponent component = proof_register_component(argv[1]);
        AudioUnit unit = proof_make_unit(component, 2);
        CHECK(AudioUnitInitialize(unit) == noErr);

        const char* className = nullptr;
        NSView* container = make_view(unit, NSMakeSize(0, 0), &className);
        CHECK(container);
        const NSSize size = container.frame.size;
        CHECK(size.width > 0 && size.height > 0);
        std::printf("view %s, editor %.0fx%.0f\n", className, size.width, size.height);
        NSWindow* window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, size.width, size.height) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        NSView* parent = [window contentView];
        [parent addSubview:container];
        CHECK(container.superview == parent && container.window == window);
        CHECK(container.subviews.count == 1);
        NSView* editor = container.subviews[0];
        CHECK(editor.frame.size.width == size.width && editor.frame.size.height == size.height);
        spin(0.2);

        // The engine is destroyed and made again with the view open.
        CHECK(AudioUnitUninitialize(unit) == noErr);
        spin(0.15);
        CHECK(AudioUnitInitialize(unit) == noErr);
        spin(0.15);

        // The default is the Warm toggle of examples/editor.
        const double x = argc == 5 ? std::atof(argv[2]) : 135.0;
        const double y = argc == 5 ? std::atof(argv[3]) : 68.0;
        const AudioUnitParameterID param = argc == 5 ? static_cast<AudioUnitParameterID>(std::atoi(argv[4])) : 1;

        Events events;
        AUEventListenerRef listener = nullptr;
        CHECK(AUEventListenerCreate(on_event, &events, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode, 0.01f, 0.01f, &listener) == noErr);
        for (AudioUnitEventType type : {kAudioUnitEvent_BeginParameterChangeGesture, kAudioUnitEvent_EndParameterChangeGesture, kAudioUnitEvent_ParameterValueChange}) {
            AudioUnitEvent event{};
            event.mEventType = type;
            event.mArgument.mParameter = AudioUnitParameter{unit, param, kAudioUnitScope_Global, 0};
            CHECK(AUEventListenerAddEventType(listener, nullptr, &event) == noErr);
        }
        AudioUnitParameterValue before = -1.f;
        CHECK(AudioUnitGetParameter(unit, param, kAudioUnitScope_Global, 0, &before) == noErr);

        NSPoint at = NSMakePoint(x, size.height - y);
        for (NSEventType type : {NSEventTypeLeftMouseDown, NSEventTypeLeftMouseUp}) {
            NSEvent* click = [NSEvent mouseEventWithType:type location:at modifierFlags:0 timestamp:0 windowNumber:window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:1];
            if (type == NSEventTypeLeftMouseDown) [editor mouseDown:click];
            else [editor mouseUp:click];
        }
        spin(0.2);
        AudioUnitParameterValue after = -1.f;
        CHECK(AudioUnitGetParameter(unit, param, kAudioUnitScope_Global, 0, &after) == noErr);
        std::printf("click: %d begin / %d value / %d end, parameter %u: %.4f -> %.4f (listener saw %.4f)\n",
            events.begins, events.values, events.ends, param, before, after, events.value);
        CHECK(events.begins > 0 && events.begins == events.ends && events.values >= events.begins);
        CHECK(events.lastBegin == param && events.lastEnd == param && events.lastValue == param);
        CHECK(after != before);
        CHECK(events.value == after);

        // Resizing: the host sets the view's frame. The range comes from the view.
        auto click = [&](NSView* target, double height, double px, double py, int count) {
            NSPoint where = NSMakePoint(px, height - py);
            for (NSEventType type : {NSEventTypeLeftMouseDown, NSEventTypeLeftMouseUp}) {
                NSEvent* e = [NSEvent mouseEventWithType:type location:where modifierFlags:0 timestamp:0 windowNumber:window.windowNumber context:nil eventNumber:0 clickCount:count pressure:1];
                if (type == NSEventTypeLeftMouseDown) [target mouseDown:e];
                else [target mouseUp:e];
            }
            spin(0.2);
        };
        [container setFrameSize:NSMakeSize(100000, 100000)];
        const NSSize big = editor.frame.size;  // the largest size the editor takes
        [container setFrameSize:NSMakeSize(1, 1)];
        const NSSize small = editor.frame.size;
        std::printf("range %.0fx%.0f to %.0fx%.0f\n", small.width, small.height, big.width, big.height);
        CHECK(big.width > size.width && small.width < size.width);
        CHECK(container.frame.size.width == 1);  // the container keeps the frame the host gave it
        [container setFrameSize:big];
        CHECK(editor.frame.size.width == big.width && editor.frame.size.height == big.height);
        std::printf("resized to %.0fx%.0f\n", big.width, big.height);
        const double factor = big.width / size.width;
        const int mark = events.begins;
        click(editor, big.height, x, y, 1);  // the old position hits nothing at this size
        CHECK(events.begins == mark);
        click(editor, big.height, x * factor, y * factor, 1);
        std::printf("scaled click: %d begin / %d end\n", events.begins, events.ends);
        CHECK(events.begins == mark + 1 && events.ends == events.begins);
        CHECK(events.lastBegin == param);
        // The editor asks for a size: a double-click on the bare plate.
        [container setFrameSize:size];
        CHECK(editor.frame.size.width == size.width);
        click(editor, size.height, 5, 5, 2);
        const NSSize asked = container.frame.size;
        std::printf("editor asked for %.0fx%.0f\n", asked.width, asked.height);
        CHECK(asked.width > size.width);
        CHECK(editor.frame.size.width == asked.width && editor.frame.size.height == asked.height);
        [container setFrameSize:size];
        AUListenerDispose(listener);

        // Closing the view: the container leaves the window and is freed.
        [container removeFromSuperview];
        CHECK(parent.subviews.count == 0);
        [container release];
        spin(0.1);

        // Open once more, then tear down with the unit going first.
        container = make_view(unit, NSMakeSize(0, 0), &className);
        CHECK(container);
        [parent addSubview:container];
        spin(0.1);
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
        spin(0.2);  // the view's timer fires with its unit gone
        [container removeFromSuperview];
        [container release];
        spin(0.1);
        [window release];
        std::printf("AU editor proof passed\n");
    }
    return 0;
}
