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
        const int gainId = find_param(unit, "Gain");
        const int warmId = find_param(unit, "Warm");
        CHECK(gainId >= 0 && warmId >= 0);
        OpParamDesc gainDesc{};
        OpParamDesc warmDesc{};
        CHECK(op_shell_param(static_cast<uint32_t>(gainId), &gainDesc) != 0);
        CHECK(op_shell_param(static_cast<uint32_t>(warmId), &warmDesc) != 0);
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
