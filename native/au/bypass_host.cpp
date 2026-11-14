// SPDX-License-Identifier: MIT
// Bypass with a reported latency. The SDK example reports none, so this loads
// the test bundle built from latency_shell.cpp (37 samples) and checks that
// bypassed audio is the input delayed by exactly the latency the unit reports.
//
//   ctest --test-dir build-shell -R au_bypass_proof --output-on-failure
#include "proof_au.h"

int main(int argc, char** argv) {
    CHECK(argc == 2);
    AudioComponent component = proof_register_component(argv[1]);
    for (UInt32 channels : {1u, 2u}) {
        AudioUnit unit = proof_make_unit(component, channels);
        CHECK(AudioUnitInitialize(unit) == noErr);
        Float64 latency = -1.0;
        UInt32 latencySize = sizeof(latency);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0, &latency, &latencySize) == noErr);
        const double samples = latency * 48000.0;
        CHECK(samples > 0.0 && std::fabs(samples - std::round(samples)) < 1e-9);
        const char* label = channels == 1 ? "latency mono" : "latency stereo";
        std::printf("AU %s: reported latency %.0f samples\n", label, samples);
        proof_check_bypass(unit, channels, label, static_cast<UInt32>(std::lround(samples)));
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
    }
    std::puts("AU bypass proof with latency: delayed dry signal, crossfade, reset, restore: PASS");
    return 0;
}
