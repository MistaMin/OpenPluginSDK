// SPDX-License-Identifier: MIT
// The host's track in the AAX shell, through the shell's real
// AAX_CEffectParameters over the stand-in ABI (proof_stub.cpp, which records
// op_shell_set_track_info). The host names the track as Pro Tools does:
// AAX_eNotificationEvent_TrackNameChanged with a const AAX_IString as its
// payload. AAX has no notification and no call for a track's colour or kind,
// so the proof also shows that the shell invents neither.
//
// Proved:
//  - negative control: a data model the host never names tells its plugin
//    nothing, and other notifications (offline mode, the session path, the
//    track's position) tell nothing either;
//  - a name (with a character outside the BMP) sent on the main thread reaches
//    the plugin as UTF-8, with no colour and no kind, inside the host's call;
//  - a name sent from another host thread, with an audio thread alive, is not
//    delivered on either: it arrives on the main thread, from the main queue
//    (macOS; a system with no main queue here delivers on the calling thread);
//  - a data model destroyed with a delivery still waiting on the main queue
//    does no harm;
//  - the same name again is not repeated; a new name is told;
//  - an empty name reaches the plugin as no name;
//  - a notification without a payload is refused and tells nothing;
//  - a name given before the engine exists is delivered when EffectInit makes it;
//  - the view reads the name from the data model (GetCustomData).
// With -DOP_EDITOR=ON the shell's real AAX_CEffectGUI is made over the recording
// editor of include/stub_editor.h, with a fake controller and view container:
//  - negative control: the view of an effect that was never named tells its
//    editor nothing, however often the host's timer runs;
//  - a view opened on a named effect tells its editor the name at once;
//  - a new name sent from another thread reaches the open editor from the
//    host's GUI timer (the main thread), not from the notification's thread,
//    and the same name again does not;
//  - a name the host sends to the view only still reaches the plugin and the
//    editor.
//
//   ctest --test-dir build-shell -R aax_track_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "proof_stub.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CString.h"
#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#endif
#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <thread>
#if OP_HAS_EDITOR
#include "AAX_IACFViewContainer.h"
#include "AAX_IEffectGUI.h"
#include "AAX_UIDs.h"
#include "stub_editor.h"
#endif

using namespace openplugin_aax;

namespace {
void require(bool ok, const char* what) {
    if (!ok) {
        std::fprintf(stderr, "AAX track proof failed: %s\n", what);
        std::exit(1);
    }
}
StubTrack callAt(uint32_t n) {
    StubTrack call{};
    require(op_stub_track_call(n, &call) == 1, "the call exists");
    return call;
}
// A name only: AAX carries no colour and no kind.
bool nameOnly(const StubTrack& c, const char* name) { return c.hasName && std::strcmp(c.name, name) == 0 && c.rgba == 0 && c.flags == 0; }
// The host's notification: the payload is a const AAX_IString.
AAX_Result name(AAX_CEffectParameters* parameters, const char* utf8) {
    const AAX_CString text(utf8);
    return proofNotifyData(parameters, AAX_eNotificationEvent_TrackNameChanged, static_cast<const AAX_IString*>(&text), sizeof(AAX_IString*));
}
// The host's main thread at work: the main queue is served by its run loop. False if `done`
// did not come true in five seconds.
template <class Done>
bool pumpUntil(Done done) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline) return false;
#ifdef __APPLE__
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
#else
        std::this_thread::yield();
#endif
    }
    return true;
}
void pumpFor(int ms) {
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    pumpUntil([&] { return std::chrono::steady_clock::now() >= until; });
}
TrackData trackData(AAX_CEffectParameters* parameters) {
    TrackData data{};
    std::memset(&data, 0xAA, sizeof data);
    uint32_t written = 0;
    require(parameters->GetCustomData(kTrackData, sizeof data, &data, &written) == AAX_SUCCESS && written == sizeof data, "the data model answers kTrackData");
    return data;
}

#if OP_HAS_EDITOR
// What Pro Tools hands the view: a controller that leads to the data model, and a view container.
struct FakeController final : IACFUnknown {
    AAX_CEffectParameters* parameters;
    explicit FakeController(AAX_CEffectParameters* p) : parameters(p) {}
    ACFRESULT ACFMETHODCALLTYPE QueryInterface(const acfIID& iid, void** out) override {
        if (!out) return ACF_E_INVALIDARG;
        *out = nullptr;
        if (std::memcmp(&iid, &IID_IAAXEffectParametersV1, sizeof(acfIID)) == 0) return parameters->QueryInterface(iid, out);
        return ACF_E_NOINTERFACE;
    }
    acfUInt32 ACFMETHODCALLTYPE AddRef() override { return 1; }
    acfUInt32 ACFMETHODCALLTYPE Release() override { return 1; }
};
struct FakeContainer final : AAX_IACFViewContainer {
    int parent = 0;  // stands for the NSView; the recording editor never touches it
    ACFRESULT ACFMETHODCALLTYPE QueryInterface(const acfIID& iid, void** out) override {
        if (!out) return ACF_E_INVALIDARG;
        *out = nullptr;
        if (std::memcmp(&iid, &IID_IAAXViewContainerV1, sizeof(acfIID)) != 0) return ACF_E_NOINTERFACE;
        *out = static_cast<AAX_IACFViewContainer*>(this);
        return ACF_OK;
    }
    acfUInt32 ACFMETHODCALLTYPE AddRef() override { return 1; }
    acfUInt32 ACFMETHODCALLTYPE Release() override { return 1; }
    int32_t GetType() override { return AAX_eViewContainer_Type_NSView; }
    void* GetPtr() override { return &parent; }
    AAX_Result GetModifiers(uint32_t* modifiers) override {
        if (modifiers) *modifiers = 0;
        return AAX_SUCCESS;
    }
    AAX_Result SetViewSize(AAX_Point&) override { return AAX_SUCCESS; }
    AAX_Result HandleParameterMouseDown(AAX_CParamID, uint32_t) override { return AAX_SUCCESS; }
    AAX_Result HandleParameterMouseDrag(AAX_CParamID, uint32_t) override { return AAX_SUCCESS; }
    AAX_Result HandleParameterMouseUp(AAX_CParamID, uint32_t) override { return AAX_SUCCESS; }
};
struct EditorCall {
    std::string name;
    bool hasName = false;
    uint32_t rgba = 0xDEADBEEF, flags = 0xDEADBEEF;
};
EditorCall editorAt(uint32_t n) {
    EditorCall c;
    char text[300] = {};
    uint32_t has = 0;
    require(op_stub_editor_track_call(n, text, sizeof text, &has, &c.rgba, &c.flags, nullptr) == 1, "the editor call exists");
    c.name = text;
    c.hasName = has != 0;
    return c;
}
bool editorNameOnly(const EditorCall& c, const char* name) { return c.hasName && c.name == name && c.rgba == 0 && c.flags == 0; }
// An effect as the host builds it: the data model (held by one reference of the proof's),
// then the view over it, with its container.
struct Effect {
    AAX_CEffectParameters* parameters = createParameters();
    FakeController controller{parameters};
    FakeContainer container;
    AAX_IEffectGUI* gui = nullptr;
    Effect() {
        require(parameters != nullptr, "createParameters");
        parameters->AddRef();
        require(proofInit(parameters, 48000.0, 2), "the data model builds over the stand-in");
    }
    void open() {
        gui = createGUI();
        require(gui != nullptr, "createGUI");
        gui->AddRef();
        require(gui->Initialize(&controller) == AAX_SUCCESS, "the view initializes");
        require(gui->SetViewContainer(&container) == AAX_SUCCESS, "the view takes its container");
    }
    void close() {
        require(gui->SetViewContainer(nullptr) == AAX_SUCCESS, "the view lets its container go");
        require(gui->Uninitialize() == AAX_SUCCESS, "the view uninitializes");
        gui->Release();
        gui = nullptr;
    }
    ~Effect() { parameters->Release(); }
};
#endif
}

int main() {
    // Negative control: an effect the host never names.
    {
        std::unique_ptr<AAX_CEffectParameters> quiet(createParameters());
        require(quiet && proofInit(quiet.get(), 48000.0, 2), "the data model builds over the stand-in");
        require(proofNotify(quiet.get(), AAX_eNotificationEvent_EnteringOfflineMode), "offline mode is taken");
        require(proofNotify(quiet.get(), AAX_eNotificationEvent_ExitingOfflineMode), "offline mode is taken");
        const AAX_CString path("/Sessions/Song.ptx");
        proofNotifyData(quiet.get(), AAX_eNotificationEvent_SessionPathChanged, static_cast<const AAX_IString*>(&path), sizeof(AAX_IString*));
        const int32_t position = 3;
        proofNotifyData(quiet.get(), AAX_eNotificationEvent_TrackPositionChanged, &position, sizeof position);
        require(op_stub_track_count() == 0, "an effect the host never names tells its plugin nothing");
        const TrackData none = trackData(quiet.get());
        require(none.serial == 0 && none.name[0] == '\0', "and has no track to show a view");
        std::printf("AAX track: an effect the host never names tells its plugin nothing (negative control)\n");
    }

    std::unique_ptr<AAX_CEffectParameters> parameters(createParameters());
    require(parameters && proofInit(parameters.get(), 48000.0, 2), "the data model builds over the stand-in");
    void* engine = proofEngine(parameters.get());
    require(engine != nullptr && op_stub_track_count() == 0, "an engine, and nothing told yet");

    // 1. A name on the main thread: delivered inside the host's call. U+266A is in the BMP,
    //    U+1D11E is not.
    const char* vox = "Vox \xE2\x99\xAA \xF0\x9D\x84\x9E";
    require(name(parameters.get(), vox) == AAX_SUCCESS && op_stub_track_count() == 1, "told inside the host's call");
    {
        const StubTrack call = callAt(0);
        require(nameOnly(call, vox) && call.instance == engine && call.onThisThread, "the name as UTF-8, no colour, no kind, to this engine, on the main thread");
        std::printf("AAX track: TrackNameChanged \"%s\" -> the plugin, UTF-8, no colour, no kind, on the main thread\n", vox);
    }

    // 1b. A name from another host thread, while an audio thread is alive: the plugin hears it
    //     on the main thread, not on the notification's thread and not on the audio thread.
    {
        std::atomic<bool> running{false}, finished{false};
        bool audioRanIt = true;
        uint32_t countOnHostThread = 99;
        std::thread audio([&] {
            running = true;
            while (!finished) std::this_thread::yield();
            StubTrack seen{};
            audioRanIt = op_stub_track_call(1, &seen) == 1 && seen.onThisThread != 0;
        });
        while (!running) std::this_thread::yield();
        std::thread host([&] {
            require(name(parameters.get(), "Strings") == AAX_SUCCESS, "the notification is taken");
            countOnHostThread = op_stub_track_count();
        });
        host.join();
#ifdef __APPLE__
        require(countOnHostThread == 1 && op_stub_track_count() == 1, "not delivered on the notification's thread");
        require(trackData(parameters.get()).serial != 0 && std::strcmp(trackData(parameters.get()).name, "Strings") == 0, "the view can already read it");
#endif
        require(pumpUntil([] { return op_stub_track_count() == 2; }), "delivered from the main queue");
        finished = true;
        audio.join();
        const StubTrack call = callAt(1);
        require(nameOnly(call, "Strings") && call.instance == engine, "the new name, to this engine");
#ifdef __APPLE__
        require(call.onThisThread, "on the main thread");
#endif
        require(!audioRanIt, "not on the audio thread");
        pumpFor(50);
        require(op_stub_track_count() == 2, "once");
        std::printf("AAX track: a name sent from another thread reaches the plugin on the main thread, not the audio thread\n");
    }

    // 1c. A data model that goes away with a delivery still waiting on the main queue.
    {
        const uint32_t before = op_stub_track_count();
        std::unique_ptr<AAX_CEffectParameters> gone(createParameters());
        require(gone && proofInit(gone.get(), 48000.0, 2), "the data model builds over the stand-in");
        std::thread host([&] { require(name(gone.get(), "Gone") == AAX_SUCCESS, "the notification is taken"); });
        host.join();
#ifdef __APPLE__
        gone.reset();
        pumpFor(100);
        require(op_stub_track_count() == before, "nothing is delivered to a data model that is gone");
#else
        require(op_stub_track_count() == before + 1, "delivered on the calling thread");
#endif
    }
    const uint32_t base = op_stub_track_count();

    // 2. The same name again is not repeated; a new one is told.
    require(name(parameters.get(), "Strings") == AAX_SUCCESS && op_stub_track_count() == base, "the same name is not repeated");
    require(name(parameters.get(), "Bass") == AAX_SUCCESS && op_stub_track_count() == base + 1 && nameOnly(callAt(base), "Bass"), "a new name is told");

    // 3. The view's read of the data model.
    {
        const TrackData first = trackData(parameters.get());
        require(first.serial != 0 && std::strcmp(first.name, "Bass") == 0, "kTrackData holds the name");
        require(name(parameters.get(), "Drums") == AAX_SUCCESS, "another name");
        const TrackData second = trackData(parameters.get());
        require(second.serial != first.serial && std::strcmp(second.name, "Drums") == 0, "and its count moves with the name");
        uint32_t written = 99;
        TrackData data{};
        require(parameters->GetCustomData(kTrackData, sizeof data - 1, &data, &written) != AAX_SUCCESS, "a block of another size is refused");
    }

    // 4. An empty name is no name. A notification without a payload is refused.
    require(name(parameters.get(), "") == AAX_SUCCESS && op_stub_track_count() == base + 3, "an empty name is told");
    {
        const StubTrack call = callAt(base + 2);
        require(!call.hasName && call.rgba == 0 && call.flags == 0, "as no name");
    }
    require(proofNotifyData(parameters.get(), AAX_eNotificationEvent_TrackNameChanged, nullptr, 0) != AAX_SUCCESS && op_stub_track_count() == base + 3,
            "no payload: refused, nothing told");
    std::printf("AAX track: the same name is told once, an empty name is no name, a missing payload is refused\n");

    // 5. A name given before EffectInit made the engine waits for it.
    {
        std::unique_ptr<AAX_CEffectParameters> early(createParameters());
        require(early && proofEngine(early.get()) == nullptr, "no engine yet");
        require(name(early.get(), "Early") == AAX_SUCCESS && op_stub_track_count() == base + 3, "nothing to tell yet");
        require(proofInit(early.get(), 48000.0, 2) && op_stub_track_count() == base + 4, "the new engine is told");
        const StubTrack call = callAt(base + 3);
        require(nameOnly(call, "Early") && call.instance == proofEngine(early.get()), "the name given earlier, to the new engine");
        std::printf("AAX track: a name given before the engine exists is delivered when it does\n");
    }

#if OP_HAS_EDITOR
    {
        // Negative control: the view of an effect that was never named.
        Effect quiet;
        quiet.open();
        require(op_stub_editor_live() == 1, "the view made its editor");
        for (int i = 0; i < 5; ++i) require(quiet.gui->TimerWakeup() == AAX_SUCCESS, "the GUI timer runs");
        require(op_stub_editor_track_count() == 0, "the view of an effect that was never named tells its editor nothing");
        quiet.close();
        require(op_stub_editor_live() == 0, "the editor is gone with the view");
        std::printf("AAX track: the view of an effect that was never named tells its editor nothing (negative control)\n");

        // A view opened on a named effect tells its editor at once.
        Effect effect;
        const uint32_t pluginCalls = op_stub_track_count();
        require(name(effect.parameters, vox) == AAX_SUCCESS && op_stub_track_count() == pluginCalls + 1, "the plugin is told with no view open");
        effect.open();
        require(op_stub_editor_track_count() == 1 && editorNameOnly(editorAt(0), vox), "the editor hears the name as the view opens");
        require(effect.gui->TimerWakeup() == AAX_SUCCESS && op_stub_editor_track_count() == 1, "and not again from the timer");

        // A new name: the editor hears it from the GUI timer, not from the notification's thread.
        const uint32_t beforeKeys = op_stub_track_count();
        std::thread host([&] { require(name(effect.parameters, "Keys") == AAX_SUCCESS, "the notification is taken"); });
        host.join();
        require(op_stub_editor_track_count() == 1, "the notification's thread does not touch the editor");
        require(pumpUntil([&] { return op_stub_track_count() == beforeKeys + 1; }) && nameOnly(callAt(beforeKeys), "Keys"), "the plugin hears it on the main thread");
        require(op_stub_editor_track_count() == 1, "and the editor still waits for the GUI timer");
        require(effect.gui->TimerWakeup() == AAX_SUCCESS && op_stub_editor_track_count() == 2 && editorNameOnly(editorAt(1), "Keys"),
                "the GUI timer tells the editor");
        require(name(effect.parameters, "Keys") == AAX_SUCCESS && effect.gui->TimerWakeup() == AAX_SUCCESS && op_stub_editor_track_count() == 2,
                "the same name is not repeated");

        // A name the host sends to the view only.
        const uint32_t before = op_stub_track_count();
        const AAX_CString pad("Pad");
        require(effect.gui->NotificationReceived(AAX_eNotificationEvent_TrackNameChanged, static_cast<const AAX_IString*>(&pad), sizeof(AAX_IString*)) == AAX_SUCCESS,
                "the view takes the notification");
        require(op_stub_track_count() == before + 1 && nameOnly(callAt(before), "Pad"), "and the plugin hears it");
        require(op_stub_editor_track_count() == 2, "the editor waits for the timer");
        require(effect.gui->TimerWakeup() == AAX_SUCCESS && op_stub_editor_track_count() == 3 && editorNameOnly(editorAt(2), "Pad"), "then hears it");

        // An empty name reaches the editor as no name.
        require(name(effect.parameters, "") == AAX_SUCCESS && effect.gui->TimerWakeup() == AAX_SUCCESS && op_stub_editor_track_count() == 4, "an empty name");
        const EditorCall none = editorAt(3);
        require(!none.hasName && none.rgba == 0 && none.flags == 0, "is no name");
        effect.close();
        require(op_stub_editor_live() == 0, "the editor is gone with the view");
        std::printf("AAX track: the editor behind the shell's AAX_CEffectGUI hears the name when it opens and from the GUI timer\n");
    }
#endif
    std::printf("AAX track proof passed\n");
    return 0;
}
