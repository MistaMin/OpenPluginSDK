// SPDX-License-Identifier: MIT
// Parameter text over a stand-in with a choice parameter (text_shell.cpp). Loads the test bundle in
// this process only and checks what the SDK example cannot: a choice parameter is indexed and lists
// the plug-in's labels (one per step, in order, with a non-ASCII letter), its text and its typed
// value go through the plug-in, a stepped number is not mistaken for a choice, and parameters
// without strings (the unit's other properties) are untouched.
//
//   ctest --test-dir build-shell -R au_text_proof --output-on-failure
#include "proof_au.h"
#include <limits>

int main(int argc, char** argv) {
    CHECK(argc == 2);
    AudioComponent component = proof_register_component(argv[1]);
    AudioUnit unit = proof_make_unit(component, 2);
    enum { kLevel, kSwitch, kMode, kSteps };
    auto info = [&](UInt32 id) {
        AudioUnitParameterInfo host{};
        UInt32 size = sizeof(host);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &host, &size) == noErr);
        CHECK((host.flags & kAudioUnitParameterFlag_ValuesHaveStrings) != 0);
        return host;
    };
    // Level: continuous dB.
    CHECK(info(kLevel).unit == kAudioUnitParameterUnit_Decibels);
    const AudioUnitParameterValue six = 6.0f, low = -24.0f;
    CHECK(proof_text_of(unit, kLevel, &six) == "6.0 dB");
    CHECK(proof_text_of(unit, kLevel, &low) == "-24.0 dB");
    double plain = 0.0;
    CHECK(proof_value_of(unit, kLevel, "6 dB", &plain) && plain == 6.0);
    CHECK(!proof_value_of(unit, kLevel, "loud", &plain));
    std::vector<std::string> labels;
    CHECK(!proof_value_strings(unit, kLevel, &labels));
    // Switch: a toggle keeps the boolean unit and reads its labels.
    CHECK(info(kSwitch).unit == kAudioUnitParameterUnit_Boolean);
    const AudioUnitParameterValue off = 0.0f, on = 1.0f;
    CHECK(proof_text_of(unit, kSwitch, &off) == "Off" && proof_text_of(unit, kSwitch, &on) == "On");
    CHECK(proof_value_of(unit, kSwitch, "On", &plain) && plain == 1.0);
    CHECK(!proof_value_strings(unit, kSwitch, &labels));
    // Mode: a choice is indexed and lists the plug-in's labels.
    const AudioUnitParameterInfo mode = info(kMode);
    CHECK(mode.unit == kAudioUnitParameterUnit_Indexed);
    CHECK(mode.minValue == 0.0f && mode.maxValue == 2.0f);
    CHECK(proof_value_strings(unit, kMode, &labels));
    const std::vector<std::string> expected = {"Clean", "Soft", "Gr\xC3\xB6\xC3\x9F" "e"};
    CHECK(labels == expected);
    for (size_t i = 0; i < expected.size(); ++i) {
        const auto value = static_cast<AudioUnitParameterValue>(i);
        CHECK(proof_text_of(unit, kMode, &value) == expected[i]);
        CHECK(proof_value_of(unit, kMode, expected[i].c_str(), &plain) && plain == static_cast<double>(i));
    }
    CHECK(!proof_value_of(unit, kMode, "Loud", &plain));
    // Steps: five whole numbers are a stepped parameter, not a choice.
    CHECK(info(kSteps).unit == kAudioUnitParameterUnit_Generic);
    CHECK(!proof_value_strings(unit, kSteps, &labels));
    const AudioUnitParameterValue three = 3.0f;
    CHECK(proof_text_of(unit, kSteps, &three) == "3");
    // The unit's text follows the current value when none is given, and refuses bad requests.
    CHECK(AudioUnitSetParameter(unit, kMode, kAudioUnitScope_Global, 0, 1.0f, 0) == noErr);
    CHECK(proof_text_of(unit, kMode, nullptr) == "Soft");
    bool ok = true;
    CHECK(proof_text_of(unit, 77, &three, &ok).empty() && !ok);
    const AudioUnitParameterValue nan = std::numeric_limits<float>::quiet_NaN();
    proof_text_of(unit, kMode, &nan, &ok);
    CHECK(!ok);
    CHECK(!proof_value_strings(unit, 77, &labels));
    // The stand-in has no parameter groups: no parameter is in a clump and no clump has a name,
    // exactly as before groups existed.
    for (UInt32 id : {kLevel, kSwitch, kMode, kSteps}) CHECK((info(id).flags & kAudioUnitParameterFlag_HasClump) == 0 && info(id).clumpID == 0);
    AudioUnitParameterNameInfo clump{};
    clump.inID = 1;
    clump.inDesiredLength = kAudioUnitParameterName_Full;
    UInt32 clumpSize = sizeof(clump);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterClumpName, kAudioUnitScope_Global, 0, &clump, &clumpSize) != noErr);
    CHECK(AudioComponentInstanceDispose(unit) == noErr);
    std::puts("AU text proof: choice labels (indexed), toggle labels, dB text and typed values through the plug-in, stepped numbers not indexed, bad requests refused, no clumps without groups: PASS");
    return 0;
}
