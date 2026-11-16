// SPDX-License-Identifier: MIT
// kAudioUnitProperty_SupportsMPE through the Audio Unit shell (roadmap 0.7 task 4).
// The shell reports 1 for a plugin whose Plugin::mpe_input is Some (the library sets
// OP_NOTE_PORT_MPE in op_shell_note_ports) and 0 for any other. The bundle is
// registered in this process only (AudioComponentRegister); nothing is installed.
// Not a DAW test.
//
//   AUMpeProof <bundle.component> <expected 0 or 1>
#include "proof_au.h"

int main(int argc, char** argv) {
    if (argc != 3 || (std::strcmp(argv[2], "0") != 0 && std::strcmp(argv[2], "1") != 0)) {
        std::fprintf(stderr, "usage: AUMpeProof <bundle.component> <0|1>\n");
        return 2;
    }
    const UInt32 expected = argv[2][0] == '1' ? 1 : 0;
    AudioComponent component = proof_register_component(argv[1]);
    AudioUnit unit = nullptr;
    CHECK(AudioComponentInstanceNew(component, &unit) == noErr && unit);

    // Global scope, a read-only UInt32, before and after Initialize.
    for (int pass = 0; pass < 2; ++pass) {
        UInt32 size = 0;
        Boolean writable = true;
        CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_SupportsMPE, kAudioUnitScope_Global, 0, &size, &writable) == noErr);
        CHECK(size == sizeof(UInt32));
        CHECK(!writable);
        UInt32 value = 99;
        size = sizeof(value);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_SupportsMPE, kAudioUnitScope_Global, 0, &value, &size) == noErr);
        CHECK(size == sizeof(UInt32));
        if (value != expected) {
            std::fprintf(stderr, "AU proof failed: SupportsMPE is %u, expected %u\n", value, expected);
            return 1;
        }
        // The host cannot turn it on or off.
        UInt32 flip = expected ? 0 : 1;
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_SupportsMPE, kAudioUnitScope_Global, 0, &flip, sizeof(flip)) != noErr);
        if (pass == 0) {
            const OSStatus init = AudioUnitInitialize(unit);
            CHECK(init == noErr);
        }
    }
    AudioUnitUninitialize(unit);
    AudioComponentInstanceDispose(unit);
    std::printf("AU SupportsMPE proof passed: %s reports %u\n", argv[1], expected);
    return 0;
}
