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
//    engine) says nothing.
//
//   ctest --test-dir build-shell -R au_changes_proof --output-on-failure
#include "proof_au.h"
#include <chrono>
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

    // 2. New parameter info.
    CHECK(rename() == 1);
    CHECK(pump_until([&] { return heard.list > 0; }));
    pump_for(120);
    CHECK(heard.list == 1 && heard.info == 1 && heard.infoElement == 0);
    CHECK(heard.latency == 1 && heard.offMain == 0);
    std::printf("AU changes: new parameter info -> kAudioUnitProperty_ParameterList once, kAudioUnitProperty_ParameterInfo for parameter 0\n");

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
    std::printf("AU changes proof passed\n");
    return 0;
}
