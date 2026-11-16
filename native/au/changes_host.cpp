// SPDX-License-Identifier: MIT
// Telling an Audio Unit host about changes (shell ABI 6). The component is the
// Audio Unit shell over vst3/changes_stub.cpp, whose latency follows its one
// parameter (256 x its value, 0..1) the way the Rust library's does. The host
// listens to kAudioUnitProperty_Latency, kAudioUnitProperty_ParameterList and
// kAudioUnitProperty_ParameterInfo, renders on its own thread, and runs the
// main run loop, where the shell's timer polls the engine.
//
// Proved:
//  - a parameter that moves the latency mid-session is told once, as a
//    kAudioUnitProperty_Latency notification on the main thread, and the host
//    reads the new latency back (128 samples at 48 kHz);
//  - renders with an unchanged latency notify nothing;
//  - new parameter info is told once: kAudioUnitProperty_ParameterList on the
//    global scope, then kAudioUnitProperty_ParameterInfo for the parameter;
//  - the poll never runs on the render thread, and an uninitialized unit (no
//    engine) says nothing;
//  - ABI 9, a renamed table: when the host hears, kAudioUnitProperty_ParameterInfo
//    has the new name and unit and kAudioUnitProperty_ParameterStringFromValue the
//    new text, and typed text is read by it; the first table before the rename,
//    and through the library's call without an instance (negative controls);
//  - ABI 9, the bypass follows the latency: bypassed, the output is the input
//    delayed by exactly the latency now (0 -> 128 -> 64 -> 192) with the delay
//    sized at Initialize for the declared largest latency (256); a plugin
//    that declares none keeps the latency of Initialize (the negative control:
//    delayed by 0, not by 128). au_alloc_changes_proof renders over the same
//    bundle with the counters armed while the latency moves.
//
//   ctest --test-dir build-shell -R au_changes_proof --output-on-failure
#include "proof_au.h"
#include <chrono>
#include <string>
#include <pthread.h>
#include <thread>

namespace {
struct Heard {
    int latency = 0, list = 0, info = 0, offMain = 0;
    AudioUnitElement infoElement = 0xFFFFFFFF;
};
void listener(void* refcon, AudioUnit, AudioUnitPropertyID id, AudioUnitScope scope, AudioUnitElement element) {
    auto* heard = static_cast<Heard*>(refcon);
    if (!pthread_main_np()) ++heard->offMain;
    if (id == kAudioUnitProperty_Latency && scope == kAudioUnitScope_Global) ++heard->latency;
    if (id == kAudioUnitProperty_ParameterList && scope == kAudioUnitScope_Global) ++heard->list;
    if (id == kAudioUnitProperty_ParameterInfo && scope == kAudioUnitScope_Global) {
        ++heard->info;
        heard->infoElement = element;
    }
}
template <class Done>
bool pump_until(Done done) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline) return false;
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
    }
    return true;
}
void pump_for(int ms) {
    const auto until = std::chrono::steady_clock::now() + std::chrono::milliseconds(ms);
    while (std::chrono::steady_clock::now() < until) CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
}
// `blocks` renders on a thread of their own, as a host's render thread.
void render_on_audio_thread(AudioUnit unit, int blocks, double& clock) {
    std::thread audio([&] {
        ProofBuffers buffers(2, kProofBlock);
        for (int i = 0; i < blocks; ++i) {
            CHECK(proof_render(unit, buffers, kProofBlock, clock) == noErr);
            clock += kProofBlock;
        }
    });
    audio.join();
}
// What a host reads for parameter 0: its name, its unit, the text of `plain`.
std::string name0(AudioUnit unit, AudioUnitParameterUnit* unitOut = nullptr) {
    AudioUnitParameterInfo info{};
    UInt32 size = sizeof(info);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, 0, &info, &size) == noErr);
    CHECK((info.flags & kAudioUnitParameterFlag_HasCFNameString) && info.cfNameString);
    char name[128] = {};
    CHECK(CFStringGetCString(info.cfNameString, name, sizeof(name), kCFStringEncodingUTF8));
    if (info.flags & kAudioUnitParameterFlag_CFNameRelease) CFRelease(info.cfNameString);
    if (unitOut) *unitOut = info.unit;
    return name;
}
std::string text0(AudioUnit unit, float plain) {
    AudioUnitParameterStringFromValue request{0, &plain, nullptr};
    UInt32 size = sizeof(request);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterStringFromValue, kAudioUnitScope_Global, 0, &request, &size) == noErr && request.outString);
    char text[128] = {};
    CHECK(CFStringGetCString(request.outString, text, sizeof(text), kCFStringEncodingUTF8));
    CFRelease(request.outString);
    return text;
}
float value0(AudioUnit unit, CFStringRef text) {
    AudioUnitParameterValueFromString request{0, text, -1.0f};
    UInt32 size = sizeof(request);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterValueFromString, kAudioUnitScope_Global, 0, &request, &size) == noErr);
    return request.outValue;
}
// Is block `block` of a bypassed render (channel 0) the proof tone delayed by `latency`, exactly?
bool delayed_by(const std::vector<std::vector<float>>& out, int first, int block, UInt32 latency) {
    for (long i = 0; i < static_cast<long>(kProofBlock); ++i) {
        const long n = static_cast<long>(block) * kProofBlock + i;
        const float want = n >= static_cast<long>(latency) ? proof_tone(n - latency) : 0.0f;
        if (out[0][static_cast<size_t>((block - first) * static_cast<long>(kProofBlock) + i)] != want) return false;
    }
    return true;
}
UInt32 latency_samples(AudioUnit unit) {
    Float64 seconds = -1.0;
    UInt32 size = sizeof(seconds);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0, &seconds, &size) == noErr);
    return static_cast<UInt32>(std::lround(seconds * 48000.0));
}
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    CHECK(pthread_main_np());
    AudioComponent component = proof_register_component(argv[1]);
    auto rename = reinterpret_cast<uint32_t (*)(void)>(proof_bundle_symbol("op_stub_changes_rename"));
    auto takes = reinterpret_cast<void (*)(uint32_t*, uint32_t*)>(proof_bundle_symbol("op_stub_changes_takes"));
    CHECK(rename && takes);

    Heard heard;
    AudioUnit unit = proof_make_unit(component, 2);
    for (AudioUnitPropertyID id : {kAudioUnitProperty_Latency, kAudioUnitProperty_ParameterList, kAudioUnitProperty_ParameterInfo})
        CHECK(AudioUnitAddPropertyListener(unit, id, listener, &heard) == noErr);
    // Not initialized: no engine, nothing to say.
    pump_for(100);
    CHECK(heard.latency == 0 && heard.list == 0 && heard.info == 0);

    CHECK(AudioUnitInitialize(unit) == noErr);
    CHECK(latency_samples(unit) == 0);
    double clock = 0.0;
    render_on_audio_thread(unit, 4, clock);
    pump_for(120);
    CHECK(heard.latency == 0);

    // 1. The parameter moves the latency mid-session.
    CHECK(AudioUnitSetParameter(unit, 0, kAudioUnitScope_Global, 0, 0.5f, 0) == noErr);
    render_on_audio_thread(unit, 8, clock);
    CHECK(pump_until([&] { return heard.latency > 0; }));
    pump_for(120);
    CHECK(heard.latency == 1);
    CHECK(heard.offMain == 0);
    CHECK(latency_samples(unit) == 128);
    std::printf("AU changes: lookahead 0.5 -> one kAudioUnitProperty_Latency notification on the main thread, latency read back 128 samples\n");
    render_on_audio_thread(unit, 4, clock);
    pump_for(120);
    CHECK(heard.latency == 1);

    // 2. New parameter info, and the renamed table the host then reads (ABI 9).
    CHECK(name0(unit) == "Lookahead");
    const std::string before = text0(unit, 0.5f);
    CHECK(before.rfind("0.50 ", 0) == 0);  // the stand-ins' first-table text
    CHECK(rename() == 1);
    CHECK(pump_until([&] { return heard.list > 0; }));
    pump_for(120);
    CHECK(heard.list == 1 && heard.info == 1 && heard.infoElement == 0);
    CHECK(heard.latency == 1 && heard.offMain == 0);
    std::printf("AU changes: new parameter info -> kAudioUnitProperty_ParameterList once, kAudioUnitProperty_ParameterInfo for parameter 0\n");
    CHECK(name0(unit) == "Lookahead (renamed)");
    CHECK(text0(unit, 0.5f) == "128 smp (renamed)");
    CHECK(value0(unit, CFSTR("64 smp")) == 0.25f);
    {
        auto typeName = reinterpret_cast<uint32_t (*)(char*, uint32_t)>(proof_bundle_symbol("op_stub_changes_type_name"));
        char name[64] = {};
        CHECK(typeName && typeName(name, sizeof(name)) && std::strcmp(name, "Lookahead") == 0);  // negative control
    }
    std::printf("AU changes: renamed table -> ParameterInfo \"%s\", ParameterStringFromValue(0.5) \"%s\" (was \"Lookahead\", \"%s\"); the call without an instance keeps the first table\n",
                name0(unit).c_str(), text0(unit, 0.5f).c_str(), before.c_str());

    // 3. Another latency, read back as soon as the host hears.
    CHECK(AudioUnitSetParameter(unit, 0, kAudioUnitScope_Global, 0, 0.25f, 0) == noErr);
    render_on_audio_thread(unit, 2, clock);
    CHECK(pump_until([&] { return heard.latency == 2; }));
    CHECK(latency_samples(unit) == 64);
    pump_for(120);
    CHECK(heard.latency == 2 && heard.offMain == 0);

    uint32_t mainTakes = 0, otherTakes = 0;
    takes(&mainTakes, &otherTakes);
    CHECK(mainTakes > 0 && otherTakes == 0);
    std::printf("AU changes: %u polls, all on the main thread, none on the render thread\n", mainTakes);

    for (AudioUnitPropertyID id : {kAudioUnitProperty_Latency, kAudioUnitProperty_ParameterList, kAudioUnitProperty_ParameterInfo})
        CHECK(AudioUnitRemovePropertyListenerWithUserData(unit, id, listener, &heard) == noErr);
    CHECK(AudioUnitUninitialize(unit) == noErr);
    CHECK(AudioComponentInstanceDispose(unit) == noErr);

    // 4. ABI 9: the bypass follows the latency. Bypassed from the start, the latency moved by
    // the parameter between renders; the render that applies a value is the switch, the two
    // after it are compared.
    auto declareMax = reinterpret_cast<void (*)(uint32_t)>(proof_bundle_symbol("op_stub_changes_declare_max"));
    CHECK(declareMax);
    struct Step { float value; UInt32 latency; };
    auto bypassRun = [&](const std::vector<Step>& steps, int& block, std::vector<bool>& delayed) {
        AudioUnit bypassed = proof_make_unit(component, 2);
        CHECK(AudioUnitInitialize(bypassed) == noErr);
        proof_set_bypass(bypassed, true);
        const auto warm = proof_render_run(bypassed, 2, block, 2);
        block += 2;
        delayed.push_back(delayed_by(warm, block - 2, block - 1, 0));
        for (const Step& step : steps) {
            CHECK(AudioUnitSetParameter(bypassed, 0, kAudioUnitScope_Global, 0, step.value, 0) == noErr);
            const auto out = proof_render_run(bypassed, 2, block, 3);
            delayed.push_back(delayed_by(out, block, block + 1, step.latency) && delayed_by(out, block, block + 2, step.latency));
            block += 3;
        }
        CHECK(AudioUnitUninitialize(bypassed) == noErr);
        CHECK(AudioComponentInstanceDispose(bypassed) == noErr);
    };
    int block = 0;
    std::vector<bool> follows;
    declareMax(1);
    bypassRun({{0.5f, 128}, {0.25f, 64}, {0.75f, 192}}, block, follows);
    CHECK(follows == std::vector<bool>({true, true, true, true}));
    // Controls of the check itself: it tells these latencies apart (the tone repeats every 48).
    {
        std::vector<bool> wrong;
        block = 0;
        bypassRun({{0.5f, 64}, {0.25f, 128}}, block, wrong);
        CHECK(wrong == std::vector<bool>({true, false, false}));
    }
    std::printf("AU changes: bypassed output follows the latency 0 -> 128 -> 64 -> 192, delayed exactly by each\n");
    // Negative control: a plugin that declares no largest latency keeps the latency of Initialize (0).
    declareMax(0);
    {
        std::vector<bool> stays, old;
        block = 0;
        bypassRun({{0.5f, 128}}, block, stays);
        block = 0;
        bypassRun({{0.5f, 0}}, block, old);
        CHECK(stays == std::vector<bool>({true, false}) && old == std::vector<bool>({true, true}));
    }
    declareMax(1);
    std::printf("AU changes: negative control: without a declared largest latency the bypass keeps 0 after a change to 128\n");
    std::printf("AU changes proof passed\n");
    return 0;
}
