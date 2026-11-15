// SPDX-License-Identifier: MIT
// In-process Audio Unit host for the SDK example shell. Registers the factory
// in this process only and does not install the bundle. Expected parameter
// metadata, latency, tail and audio come from the op_shell_* C ABI.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R au_proof --output-on-failure
#include "op_shell.h"
#include "proof_au.h"
#include <limits>
#include <string>
static std::vector<AudioUnitParameterID> parameter_ids(AudioUnit unit) {
    UInt32 size = 0;
    CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &size, nullptr) == noErr);
    const UInt32 count = size / sizeof(AudioUnitParameterID);
    std::vector<AudioUnitParameterID> ids(count);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, ids.data(), &size) == noErr);
    CHECK(size == count * sizeof(AudioUnitParameterID));
    return ids;
}
static int find_param(AudioUnit unit, const char* name) {
    for (AudioUnitParameterID id : parameter_ids(unit)) {
        AudioUnitParameterInfo info{};
        UInt32 infoSize = sizeof(info);
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &info, &infoSize) != noErr) continue;
        if (std::strcmp(info.name, name) == 0) return static_cast<int>(id);
    }
    return -1;
}
static AudioUnitParameterUnit expected_unit(const OpParamDesc& desc) {
    if (std::strcmp(desc.unit, "dB") == 0) return kAudioUnitParameterUnit_Decibels;
    if (std::strcmp(desc.unit, "Hz") == 0) return kAudioUnitParameterUnit_Hertz;
    if (std::strcmp(desc.unit, "ms") == 0) return kAudioUnitParameterUnit_Milliseconds;
    if (std::strcmp(desc.unit, "%") == 0) return kAudioUnitParameterUnit_Percent;
    if (std::strcmp(desc.unit, ":1") == 0) return kAudioUnitParameterUnit_Ratio;
    if ((desc.flags & OP_PARAM_DISCRETE) && desc.max == 1.0 && desc.steps == 2) return kAudioUnitParameterUnit_Boolean;
    return kAudioUnitParameterUnit_Generic;
}
// The Audio Unit must expose the same parameter list the shell declares: id
// order, name, range, default, unit, log display flag and current default.
static void check_parameters(AudioUnit unit) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    CHECK(info.abi == OP_SHELL_ABI);
    const auto ids = parameter_ids(unit);
    CHECK(ids.size() == info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        CHECK(ids[i] == i);
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        AudioUnitParameterInfo host{};
        UInt32 hostSize = sizeof(host);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, i, &host, &hostSize) == noErr);
        CHECK(std::strcmp(host.name, desc.name) == 0);
        CHECK(std::fabs(host.minValue - desc.min) < 1e-5);
        CHECK(std::fabs(host.maxValue - desc.max) < 1e-5);
        CHECK(std::fabs(host.defaultValue - desc.default_plain) < 1e-5);
        CHECK((host.flags & kAudioUnitParameterFlag_IsReadable) != 0);
        CHECK((host.flags & kAudioUnitParameterFlag_IsWritable) != 0);
        CHECK(((host.flags & kAudioUnitParameterFlag_DisplayLogarithmic) != 0) == ((desc.flags & OP_PARAM_LOG) != 0));
        CHECK(host.unit == expected_unit(desc));
        CHECK(find_param(unit, desc.name) == static_cast<int>(i));
        AudioUnitParameterValue value = -123.0f;
        CHECK(AudioUnitGetParameter(unit, i, kAudioUnitScope_Global, 0, &value) == noErr);
        CHECK(std::fabs(value - desc.default_plain) < 1e-5);
    }
}
static std::string abi_group(uint32_t index) {
    char buf[512];
    const size_t length = op_shell_param_group(index, buf, sizeof(buf));
    CHECK(length < sizeof(buf));
    return std::string(buf, length);
}
static std::string utf8_of(CFStringRef text) {
    char buf[1024] = {};
    CHECK(text && CFStringGetCString(text, buf, sizeof(buf), kCFStringEncodingUTF8));
    return buf;
}
// What the host reads for clump `id` asking for at most `length` characters (0: the full name).
static OSStatus clump_name(AudioUnit unit, UInt32 id, SInt32 length, std::string& out) {
    AudioUnitParameterNameInfo request{};
    request.inID = id;
    request.inDesiredLength = length;
    UInt32 size = sizeof(request);
    const OSStatus status = AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterClumpName, kAudioUnitScope_Global, 0, &request, &size);
    if (status == noErr) {
        out = utf8_of(request.outName);
        CFRelease(request.outName);
    }
    return status;
}
// Parameter groups (ABI 5) as clumps: one clump per distinct group path, numbered from 1 in the
// order the groups first appear, named by the whole path; a top-level parameter has no clump.
// A plugin without groups has no clump flags and no clump names, as before groups existed.
static void check_clumps(AudioUnit unit) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    std::vector<std::string> expected;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        const std::string group = abi_group(i);
        AudioUnitParameterInfo host{};
        UInt32 hostSize = sizeof(host);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, i, &host, &hostSize) == noErr);
        if (group.empty()) {
            CHECK((host.flags & kAudioUnitParameterFlag_HasClump) == 0);
            CHECK(host.clumpID == 0);
            continue;
        }
        auto known = std::find(expected.begin(), expected.end(), group);
        if (known == expected.end()) known = expected.insert(expected.end(), group);
        CHECK((host.flags & kAudioUnitParameterFlag_HasClump) != 0);
        CHECK(host.clumpID == static_cast<UInt32>(known - expected.begin()) + 1);
        std::printf("AU parameter %u \"%s\": clump %u\n", i, host.name, static_cast<unsigned>(host.clumpID));
    }
    for (size_t n = 0; n < expected.size(); ++n) {
        const UInt32 id = static_cast<UInt32>(n) + 1;
        std::string name;
        CHECK(clump_name(unit, id, kAudioUnitParameterName_Full, name) == noErr);
        CHECK(name == expected[n]);
        // A short name is the last name of the path, cut to the length asked for.
        const size_t slash = expected[n].rfind('/');
        const std::string last = slash == std::string::npos ? expected[n] : expected[n].substr(slash + 1);
        std::string shortName;
        CHECK(clump_name(unit, id, 3, shortName) == noErr);
        CHECK(shortName == (expected[n].size() <= 3 ? expected[n] : last.substr(0, 3)));
        std::printf("AU clump %u: \"%s\" (3 characters: \"%s\")\n", static_cast<unsigned>(id), name.c_str(), shortName.c_str());
    }
    std::string none;
    CHECK(clump_name(unit, static_cast<UInt32>(expected.size()) + 1, kAudioUnitParameterName_Full, none) != noErr);
    CHECK(clump_name(unit, kAudioUnitClumpID_System, kAudioUnitParameterName_Full, none) != noErr);
    std::printf("AU clumps: %zu, read back by ID and name\n", expected.size());
}
// The Audio Unit shows the plug-in's own parameter text (op_shell_param_format / op_shell_param_parse),
// not the number: the example's Gain at +6 dB reads what ParamInfo::format gives, a typed "6 dB"
// reads 6.0, and the toggle reads its labels.
static std::string abi_text(uint32_t id, double plain) {
    char buf[256] = {};
    const size_t length = op_shell_param_format(id, plain, buf, sizeof(buf));
    CHECK(length > 0 && length < sizeof(buf));
    return buf;
}
static void check_text(AudioUnit unit, int gainId, int warmId) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        AudioUnitParameterInfo host{};
        UInt32 hostSize = sizeof(host);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, i, &host, &hostSize) == noErr);
        // The flag that tells a host to ask the plug-in for the text.
        CHECK((host.flags & kAudioUnitParameterFlag_ValuesHaveStrings) != 0);
        const double middle = 0.5 * (desc.min + desc.max);
        for (double plain : {desc.min, desc.default_plain, middle, desc.max}) {
            const auto value = static_cast<AudioUnitParameterValue>(plain);
            const std::string text = proof_text_of(unit, i, &value);
            CHECK(text == abi_text(i, value));
            double back = -777.0;
            CHECK(proof_value_of(unit, i, text.c_str(), &back));
            CHECK(op_shell_param_format(i, back, nullptr, 0) == text.size() && abi_text(i, back) == text);
        }
        std::vector<std::string> labels;
        const bool indexed = host.unit == kAudioUnitParameterUnit_Indexed;
        CHECK(proof_value_strings(unit, i, &labels) == indexed);
    }
    // Gain +6 dB is the plug-in's text (the example's -24..24 dB), not "6.0000".
    OpParamDesc gain{};
    CHECK(op_shell_param(static_cast<uint32_t>(gainId), &gain) != 0);
    CHECK(gain.min <= 6.0 && gain.max >= 6.0);
    const AudioUnitParameterValue six = 6.0f;
    const std::string sixText = proof_text_of(unit, gainId, &six);
    std::printf("AU Gain +6 dB reads '%s'\n", sixText.c_str());
    CHECK(sixText == abi_text(static_cast<uint32_t>(gainId), 6.0));
    CHECK(sixText.find("dB") != std::string::npos && sixText.find("6") != std::string::npos);
    // A null value asks for the current one.
    CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, six, 0) == noErr);
    CHECK(proof_text_of(unit, gainId, nullptr) == sixText);
    double plain = -777.0;
    CHECK(proof_value_of(unit, gainId, "6 dB", &plain) && plain == 6.0);
    CHECK(proof_value_of(unit, gainId, "-3", &plain) && plain == -3.0);
    CHECK(proof_value_of(unit, gainId, sixText.c_str(), &plain) && plain == 6.0);
    CHECK(proof_value_of(unit, gainId, "99 dB", &plain) && plain == gain.max);  // clamped by the plug-in
    plain = -777.0;
    CHECK(!proof_value_of(unit, gainId, "loud", &plain) && plain == -777.0);
    CHECK(!proof_value_of(unit, gainId, "3 parsecs", &plain));
    CHECK(!proof_value_of(unit, gainId, "", &plain));
    // Unknown parameters and bad values are refused.
    bool ok = true;
    CHECK(proof_text_of(unit, 9999, &six, &ok).empty() && !ok);
    const AudioUnitParameterValue nan = std::numeric_limits<float>::quiet_NaN();
    proof_text_of(unit, gainId, &nan, &ok);
    CHECK(!ok);
    CHECK(!proof_value_of(unit, 9999, "6 dB", &plain));
    // The toggle reads its labels.
    const AudioUnitParameterValue off = 0.0f, on = 1.0f;
    const std::string offText = proof_text_of(unit, warmId, &off), onText = proof_text_of(unit, warmId, &on);
    std::printf("AU Warm reads '%s' and '%s'\n", offText.c_str(), onText.c_str());
    CHECK(offText == abi_text(static_cast<uint32_t>(warmId), 0.0) && onText == abi_text(static_cast<uint32_t>(warmId), 1.0));
    CHECK(offText != onText && !offText.empty() && !onText.empty());
    CHECK(offText != "0" && onText != "1" && offText.find("0.") == std::string::npos);
    CHECK(proof_value_of(unit, warmId, onText.c_str(), &plain) && plain == 1.0);
    CHECK(proof_value_of(unit, warmId, offText.c_str(), &plain) && plain == 0.0);
    CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, static_cast<AudioUnitParameterValue>(gain.default_plain), 0) == noErr);
}
static std::vector<float> render_au(AudioUnit unit, UInt32 channels, int blocks) {
    constexpr UInt32 kFrames = 512;
    std::vector<std::vector<float>> output(channels, std::vector<float>(kFrames));
    struct List { UInt32 count; AudioBuffer data[2]; } list{};
    list.count = channels;
    for (UInt32 channel = 0; channel < channels; ++channel) list.data[channel] = {1, kFrames * sizeof(float), output[channel].data()};
    AudioTimeStamp stamp{};
    stamp.mFlags = kAudioTimeStampSampleTimeValid;
    for (int block = 0; block < blocks; ++block) {
        stamp.mSampleTime = block * kFrames;
        AudioUnitRenderActionFlags flags = 0;
        CHECK(AudioUnitRender(unit, &flags, &stamp, 0, kFrames, reinterpret_cast<AudioBufferList*>(&list)) == noErr);
        for (float sample : output[0]) CHECK(std::isfinite(sample));
        if (block + 1 == blocks) return output[0];
    }
    return {};
}
int main(int argc, char** argv) {
    CHECK(argc == 2);
    AudioComponent component = proof_register_component(argv[1]);
    OpShellInfo shellInfo{};
    CHECK(op_shell_info(&shellInfo) != 0);
    for (UInt32 channels : {1u, 2u}) {
        ReferenceShell reference(channels);
        CHECK(reference.instance);
        AudioUnit unit = proof_make_unit(component, channels);
        check_parameters(unit);
        check_clumps(unit);
        const int gainId = find_param(unit, "Gain");
        const int warmId = find_param(unit, "Warm");
        CHECK(gainId >= 0 && warmId >= 0);
        OpParamDesc gainDesc{};
        OpParamDesc warmDesc{};
        CHECK(op_shell_param(static_cast<uint32_t>(gainId), &gainDesc) != 0);
        CHECK(op_shell_param(static_cast<uint32_t>(warmId), &warmDesc) != 0);
        check_text(unit, gainId, warmId);
        CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, std::numeric_limits<float>::quiet_NaN(), 0) != noErr);
        CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, static_cast<AudioUnitParameterValue>(gainDesc.max + 10.0), 0) != noErr);
        CHECK(AudioUnitInitialize(unit) == noErr);
        const char* label = channels == 1 ? "mono" : "stereo";
        Float64 latency = -1.0;
        UInt32 latencySize = sizeof(latency);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0, &latency, &latencySize) == noErr);
        const double expectedLatency = static_cast<double>(op_shell_latency(reference.instance)) / 48000.0;
        std::printf("AU %s latency: host %.6f s, ABI %.6f s\n", label, latency, expectedLatency);
        CHECK(latency == expectedLatency);
#if OP_HAS_TAIL
        UInt32 tailSize = 0;
        Boolean writable = false;
        CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_TailTime, kAudioUnitScope_Global, 0, &tailSize, &writable) == noErr);
        CHECK(tailSize == sizeof(Float64));
        Float64 tail = -1.0;
        UInt32 tailOut = sizeof(tail);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_TailTime, kAudioUnitScope_Global, 0, &tail, &tailOut) == noErr);
        const double seconds = op_shell_tail_seconds(reference.instance);
        const double expectedTail = seconds < 600.0 ? std::max(seconds, 0.0) : 600.0;
        std::printf("AU %s tail: host %.6f s, ABI %.6f s\n", label, tail, seconds);
        CHECK(tail == expectedTail);
#else
        std::printf("AU %s tail: not built (OP_TAIL=OFF)\n", label);
#endif
        std::vector<float> defaults(shellInfo.param_count);
        for (uint32_t i = 0; i < shellInfo.param_count; ++i) {
            OpParamDesc param{};
            CHECK(op_shell_param(i, &param) != 0);
            defaults[i] = static_cast<float>(param.default_plain);
        }
        op_shell_reset(reference.instance);
        const auto auDefault = render_au(unit, channels, 4);
        const auto shellDefault = render_shell(reference.instance, channels, 4, defaults);
        std::printf("AU %s default output against the shell: max difference %.3g\n", label, max_difference(shellDefault, auDefault));
        CHECK(max_difference(shellDefault, auDefault) <= 1e-6);
        const double testGain = std::clamp(6.0, gainDesc.min, gainDesc.max);
        CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, static_cast<AudioUnitParameterValue>(testGain), 0) == noErr);
        CHECK(AudioUnitSetParameter(unit, warmId, kAudioUnitScope_Global, 0, 0.0f, 0) == noErr);
        std::vector<float> plain = defaults;
        plain[static_cast<uint32_t>(gainId)] = static_cast<float>(testGain);
        plain[static_cast<uint32_t>(warmId)] = 0.0f;
        op_shell_reset(reference.instance);
        const auto shellGain = render_shell(reference.instance, channels, 8, plain);
        const auto auGain = render_au(unit, channels, 8);
        std::printf("AU %s gain %.2f dB against the shell: max difference %.3g\n", label, testGain, max_difference(shellGain, auGain));
        CHECK(max_difference(shellGain, auGain) <= 1e-6);
        std::vector<float> dry(512);
        fill_sine(dry.data(), 512, 48000.0, 1000.0, 0.25, 7 * 512);
        const double measured = gain_db(dry.data(), auGain.data(), 128, 256);
        std::printf("AU %s measured gain: %.3f dB\n", label, measured);
        CHECK(std::fabs(measured - testGain) < 0.05);
        CFPropertyListRef saved = nullptr;
        UInt32 savedSize = sizeof(saved);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &saved, &savedSize) == noErr && saved);
        // The shell's state blob rides in the dictionary beside the base keys.
        CHECK(CFDictionaryGetValue(static_cast<CFDictionaryRef>(saved), CFSTR("OpenPluginState")) != nullptr);
        CHECK(CFGetTypeID(CFDictionaryGetValue(static_cast<CFDictionaryRef>(saved), CFSTR("OpenPluginState"))) == CFDataGetTypeID());
        const double changedGain = std::fabs(gainDesc.min - testGain) > 0.5 ? gainDesc.min : gainDesc.max;
        CHECK(AudioUnitSetParameter(unit, gainId, kAudioUnitScope_Global, 0, static_cast<AudioUnitParameterValue>(changedGain), 0) == noErr);
        CHECK(AudioUnitSetParameter(unit, warmId, kAudioUnitScope_Global, 0, 1.0f, 0) == noErr);
        const auto auChanged = render_au(unit, channels, 8);
        std::printf("AU %s changed state against saved state: max difference %.3g\n", label, max_difference(auGain, auChanged));
        CHECK(max_difference(auGain, auChanged) > 0.01);
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &saved, sizeof(saved)) == noErr);
        CFRelease(saved);
        AudioUnitParameterValue restoredGain = -123.0f;
        AudioUnitParameterValue restoredWarm = -123.0f;
        CHECK(AudioUnitGetParameter(unit, gainId, kAudioUnitScope_Global, 0, &restoredGain) == noErr);
        CHECK(AudioUnitGetParameter(unit, warmId, kAudioUnitScope_Global, 0, &restoredWarm) == noErr);
        CHECK(std::fabs(restoredGain - testGain) < 1e-5);
        CHECK(std::fabs(restoredWarm) < 1e-5);
        const auto auRestored = render_au(unit, channels, 8);
        std::printf("AU %s restored against saved state: max difference %.3g\n", label, max_difference(auGain, auRestored));
        CHECK(max_difference(auGain, auRestored) <= 1e-6);
        op_shell_reset(reference.instance);
        const auto shellRestored = render_shell(reference.instance, channels, 8, plain);
        CHECK(max_difference(shellRestored, auRestored) <= 1e-6);
        proof_check_bypass(unit, channels, label, static_cast<UInt32>(op_shell_latency(reference.instance)));
        CHECK(AudioUnitReset(unit, kAudioUnitScope_Global, 0) == noErr);
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
    }
#if OP_HAS_TAIL
    std::puts("AU proof: factory from the bundle plist, parameter metadata, latency, tail, shell-equal audio, state round-trip, bypass, rejected NaN and out-of-range values: PASS");
#else
    std::puts("AU proof: factory from the bundle plist, parameter metadata, latency, shell-equal audio, state round-trip, bypass, rejected NaN and out-of-range values, tail not built (OP_TAIL=OFF): PASS");
#endif
    return 0;
}
