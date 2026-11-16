// SPDX-License-Identifier: MIT
// The host's track in the Audio Unit shell. The component is the shell over
// vst3/track_stub.cpp, a stand-in plugin that records what
// op_shell_set_track_info tells it, and the host names the track the way Logic
// does: kAudioUnitProperty_ContextName, a CFString on the global scope. The
// Audio Unit API has no property for a track's colour or kind, so the proof
// also shows that the shell invents neither.
//
// Proved:
//  - negative control: a unit the host never names tells its plugin nothing,
//    initialized, rendered and with the run loop running; and other properties
//    (the nickname, offline render) tell nothing either;
//  - a name (with a character outside the BMP) reaches the plugin as UTF-8,
//    with no colour and no kind, on the main thread, inside the host's call;
//  - the same name again is not repeated;
//  - a name set from another thread is not delivered on that thread: it
//    arrives on the main thread, from the shell's timer;
//  - a name set while a render thread is running is delivered, on the main
//    thread, never on the render thread;
//  - a new engine (uninitialize, initialize) is told again, and a name given
//    while there is no engine is delivered when there is one;
//  - removing the property tells the plugin the track has no name, and an
//    empty name after that says nothing new;
//  - a name far longer than the ABI keeps does no harm.
// With -DOP_EDITOR=ON the component also has the recording editor of
// include/stub_editor.h behind the shell's real Cocoa view, and the proof makes
// that view as a host does:
//  - negative control: the view of a unit that was never named tells its
//    editor nothing;
//  - a view opened on a named unit tells its editor the name at once, with no
//    colour and no kind, on the main thread;
//  - a new name and a removed name reach the open editor, and the same name
//    again does not.
//
//   ctest --test-dir build-shell -R au_track_proof --output-on-failure
#include "proof_au.h"
#include <atomic>
#include <chrono>
#include <pthread.h>
#include <thread>
#if OP_HAS_EDITOR
#import <AudioUnit/AUCocoaUIView.h>
#import <Cocoa/Cocoa.h>
#endif

namespace {
using CountFn = uint32_t (*)(void);
using CallFn = uint32_t (*)(uint32_t, char*, uint32_t, uint32_t*, uint32_t*, uint32_t*, uint32_t*);
CountFn stubCount = nullptr;
CallFn stubCall = nullptr;

struct Call {
    std::string name;
    bool hasName = false;
    uint32_t rgba = 0xDEADBEEF, flags = 0xDEADBEEF;
    bool mainThread = false;
};
Call callAt(CallFn call, uint32_t n) {
    Call c;
    char name[300] = {};
    uint32_t has = 0, mainThread = 0;
    CHECK(call(n, name, sizeof(name), &has, &c.rgba, &c.flags, &mainThread) == 1);
    c.name = name;
    c.hasName = has != 0;
    c.mainThread = mainThread != 0;
    return c;
}
// A name only: the Audio Unit API carries no colour and no kind.
bool nameOnly(const Call& c, const char* name) { return c.hasName && c.name == name && c.rgba == 0 && c.flags == 0 && c.mainThread; }
bool noName(const Call& c) { return !c.hasName && c.rgba == 0 && c.flags == 0 && c.mainThread; }

template <class Done>
bool pump_until(Done done) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline) return false;
        @autoreleasepool {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
        }
    }
    return true;
}
void pump_for(int ms) {
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    while (std::chrono::steady_clock::now() < until) {
        @autoreleasepool {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
        }
    }
}
// The host names the track: kAudioUnitProperty_ContextName, as UTF-8 here.
OSStatus set_name(AudioUnit unit, const char* utf8) {
    CFStringRef name = CFStringCreateWithCString(nullptr, utf8, kCFStringEncodingUTF8);
    CHECK(name);
    const OSStatus status = AudioUnitSetProperty(unit, kAudioUnitProperty_ContextName, kAudioUnitScope_Global, 0, &name, sizeof(name));
    CFRelease(name);
    return status;
}
// The host takes the name away (a NULL value removes a property).
OSStatus remove_name(AudioUnit unit) { return AudioUnitSetProperty(unit, kAudioUnitProperty_ContextName, kAudioUnitScope_Global, 0, nullptr, 0); }

#if OP_HAS_EDITOR
CountFn editorCount = nullptr;
CountFn editorLive = nullptr;
CallFn editorCall = nullptr;
// The view the unit's factory class makes, as a host asks for it; retained. The pool is
// drained here so the proof's reference is the only one left.
NSView* make_view(AudioUnit unit) {
    NSView* view = nil;
    @autoreleasepool {
    AudioUnitCocoaViewInfo info{};
    UInt32 infoSize = sizeof(info);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_CocoaUI, kAudioUnitScope_Global, 0, &info, &infoSize) == noErr);
    CHECK(info.mCocoaAUViewBundleLocation && info.mCocoaAUViewClass[0]);
    NSBundle* bundle = [NSBundle bundleWithURL:(NSURL*)info.mCocoaAUViewBundleLocation];
    CHECK(bundle && [bundle load]);
    Class factoryClass = NSClassFromString((NSString*)info.mCocoaAUViewClass[0]);
    CFRelease(info.mCocoaAUViewBundleLocation);
    CFRelease(info.mCocoaAUViewClass[0]);
    CHECK(factoryClass && [factoryClass conformsToProtocol:@protocol(AUCocoaUIBase)]);
    NSObject<AUCocoaUIBase>* factory = [[factoryClass alloc] init];
    view = [[factory uiViewForAudioUnit:unit withSize:NSMakeSize(0, 0)] retain];
    [factory release];
    }
    CHECK(view);
    return view;
}
// The host shows the view in its window, and closes it.
void show_view(NSWindow* window, NSView* view) {
    @autoreleasepool {
        [[window contentView] addSubview:view];
    }
}
void close_view(NSView* view) {
    @autoreleasepool {
        [view removeFromSuperview];
        [view release];
    }
}
#endif
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    CHECK(pthread_main_np());
    AudioComponent component = proof_register_component(argv[1]);
    stubCount = reinterpret_cast<CountFn>(proof_bundle_symbol("op_stub_track_count"));
    stubCall = reinterpret_cast<CallFn>(proof_bundle_symbol("op_stub_track_call"));
    CHECK(stubCount && stubCall);

    // Negative control: a unit the host never names. An engine, renders, the timer: nothing.
    {
        AudioUnit quiet = proof_make_unit(component, 2);
        CHECK(AudioUnitInitialize(quiet) == noErr);
        proof_render_run(quiet, 2, 0, 4);
        pump_for(150);
        CHECK(stubCount() == 0);
        // Other properties a host sets are not the track.
        CFStringRef nick = CFSTR("Nickname");
        CHECK(AudioUnitSetProperty(quiet, kAudioUnitProperty_NickName, kAudioUnitScope_Global, 0, &nick, sizeof(nick)) == noErr);
        UInt32 offline = 1;
        CHECK(AudioUnitSetProperty(quiet, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, sizeof(offline)) == noErr);
        pump_for(150);
        CHECK(stubCount() == 0);
        CHECK(AudioUnitUninitialize(quiet) == noErr);
        CHECK(AudioComponentInstanceDispose(quiet) == noErr);
        std::printf("AU track: a unit the host never names tells its plugin nothing (negative control)\n");
    }

    AudioUnit unit = proof_make_unit(component, 2);
    CHECK(AudioUnitInitialize(unit) == noErr);
    pump_for(60);
    CHECK(stubCount() == 0);

    // 1. A name, delivered inside the host's call on the main thread. U+266A is in the BMP,
    //    U+1D11E is not.
    const char* vox = "Vox \xE2\x99\xAA \xF0\x9D\x84\x9E";
    CHECK(set_name(unit, vox) == noErr);
    CHECK(stubCount() == 1);
    CHECK(nameOnly(callAt(stubCall, 0), vox));
    // The host reads back what it set.
    {
        CFStringRef read = nullptr;
        UInt32 size = sizeof(read);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ContextName, kAudioUnitScope_Global, 0, &read, &size) == noErr && read);
        char text[64] = {};
        CHECK(CFStringGetCString(read, text, sizeof(text), kCFStringEncodingUTF8) && std::string(text) == vox);
        CFRelease(read);
    }
    std::printf("AU track: kAudioUnitProperty_ContextName \"%s\" -> the plugin, UTF-8, no colour, no kind, main thread\n", vox);

    // 2. The same name again says nothing new.
    CHECK(set_name(unit, vox) == noErr);
    pump_for(120);
    CHECK(stubCount() == 1);

    // 3. Set from another thread: not delivered there, delivered on the main thread.
    {
        std::thread other([&] { CHECK(set_name(unit, "Bass") == noErr); });
        other.join();
        CHECK(stubCount() == 1);
        CHECK(pump_until([] { return stubCount() == 2; }));
        CHECK(nameOnly(callAt(stubCall, 1), "Bass"));
        std::printf("AU track: a name set from another thread reaches the plugin on the main thread\n");
    }

    // 4. Set while a render thread runs: delivered on the main thread, not the render thread.
    {
        std::atomic<bool> stop{false};
        std::atomic<int> rendered{0};
        std::thread audio([&] {
            ProofBuffers buffers(2, kProofBlock);
            double clock = 0.0;
            while (!stop.load()) {
                CHECK(proof_render(unit, buffers, kProofBlock, clock) == noErr);
                clock += kProofBlock;
                rendered.fetch_add(1);
            }
        });
        CHECK(pump_until([&] { return rendered.load() > 4; }));
        CHECK(set_name(unit, "Drums") == noErr);
        CHECK(stubCount() == 3);
        const int before = rendered.load();
        CHECK(pump_until([&] { return rendered.load() > before + 4; }));
        stop.store(true);
        audio.join();
        CHECK(stubCount() == 3);
        CHECK(nameOnly(callAt(stubCall, 2), "Drums"));
    }

    // 5. A new engine is told again.
    CHECK(AudioUnitUninitialize(unit) == noErr);
    pump_for(60);
    CHECK(stubCount() == 3);
    CHECK(AudioUnitInitialize(unit) == noErr);
    CHECK(stubCount() == 4);
    CHECK(nameOnly(callAt(stubCall, 3), "Drums"));

    // 6. A name given while there is no engine waits for one.
    CHECK(AudioUnitUninitialize(unit) == noErr);
    CHECK(set_name(unit, "Early") == noErr);
    pump_for(120);
    CHECK(stubCount() == 4);
    CHECK(AudioUnitInitialize(unit) == noErr);
    CHECK(stubCount() == 5);
    CHECK(nameOnly(callAt(stubCall, 4), "Early"));
    pump_for(120);
    CHECK(stubCount() == 5);
    std::printf("AU track: a new engine is told again, and a name given without an engine waits for one\n");

    // 7. The host takes the name away: the plugin hears a track with no name. An empty name
    //    after that is the same thing and says nothing.
    CHECK(remove_name(unit) == noErr);
    CHECK(pump_until([] { return stubCount() == 6; }));
    CHECK(noName(callAt(stubCall, 5)));
    CHECK(set_name(unit, "") == noErr);
    pump_for(120);
    CHECK(stubCount() == 6);
    std::printf("AU track: a removed name reaches the plugin as no name\n");

    // 8. A name far longer than the ABI keeps.
    {
        const std::string longName(5000, 'n');
        CHECK(set_name(unit, longName.c_str()) == noErr);
        CHECK(stubCount() == 7);
        const Call c = callAt(stubCall, 6);
        CHECK(c.hasName && !c.name.empty() && c.name.find_first_not_of('n') == std::string::npos && c.rgba == 0 && c.flags == 0);
    }

#if OP_HAS_EDITOR
    @autoreleasepool {
        [NSApplication sharedApplication];
        editorCount = reinterpret_cast<CountFn>(proof_bundle_symbol("op_stub_editor_track_count"));
        editorLive = reinterpret_cast<CountFn>(proof_bundle_symbol("op_stub_editor_live"));
        editorCall = reinterpret_cast<CallFn>(proof_bundle_symbol("op_stub_editor_track_call"));
        CHECK(editorCount && editorLive && editorCall);
        NSWindow* window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 200, 100) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];

        // Negative control: the view of a unit that was never named.
        AudioUnit quiet = proof_make_unit(component, 2);
        CHECK(AudioUnitInitialize(quiet) == noErr);
        NSView* quietView = make_view(quiet);
        CHECK(editorLive() == 1);
        show_view(window, quietView);
        pump_for(250);
        CHECK(editorCount() == 0);
        close_view(quietView);
        CHECK(AudioUnitUninitialize(quiet) == noErr);
        CHECK(AudioComponentInstanceDispose(quiet) == noErr);
        pump_for(60);
        CHECK(editorLive() == 0 && editorCount() == 0);
        std::printf("AU track: the view of a unit that was never named tells its editor nothing (negative control)\n");

        // A view opened on a named unit tells its editor at once.
        CHECK(set_name(unit, vox) == noErr);
        NSView* view = make_view(unit);
        CHECK(editorLive() == 1 && editorCount() == 1);
        CHECK(nameOnly(callAt(editorCall, 0), vox));
        show_view(window, view);
        pump_for(250);
        CHECK(editorCount() == 1);

        // A new name, the same name again, and a removed name, with the view open.
        CHECK(set_name(unit, "Keys") == noErr);
        CHECK(pump_until([] { return editorCount() == 2; }));
        CHECK(nameOnly(callAt(editorCall, 1), "Keys"));
        CHECK(set_name(unit, "Keys") == noErr);
        pump_for(250);
        CHECK(editorCount() == 2);
        CHECK(remove_name(unit) == noErr);
        CHECK(pump_until([] { return editorCount() == 3; }));
        CHECK(noName(callAt(editorCall, 2)));
        std::printf("AU track: the editor behind the shell's Cocoa view hears the name when it opens and when it changes, on the main thread\n");

        close_view(view);
        pump_for(60);
        CHECK(editorLive() == 0);
        [window release];
    }
#endif

    CHECK(AudioUnitUninitialize(unit) == noErr);
    CHECK(AudioComponentInstanceDispose(unit) == noErr);
    std::printf("AU track proof passed\n");
    return 0;
}
