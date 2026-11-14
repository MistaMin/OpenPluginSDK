// SPDX-License-Identifier: MIT
// In-process Audio Unit host for the SDK example shell. Registers the factory
// in this process only and does not install the bundle. Expected parameter
// metadata, latency, tail and audio come from the op_shell_* C ABI.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R au_proof --output-on-failure
#include "op_shell.h"
#include "proof_shell.h"
#include <AudioToolbox/AudioToolbox.h>
#include <CoreFoundation/CoreFoundation.h>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <vector>
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "AU proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
static OSStatus render_input(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp* time, UInt32, UInt32 frames, AudioBufferList* data) {
    for (UInt32 channel = 0; channel < data->mNumberBuffers; ++channel) {
        auto* samples = static_cast<float*>(data->mBuffers[channel].mData);
        if (!samples) return kAudio_ParamError;
        fill_sine(samples, static_cast<int>(frames), 48000.0, 1000.0, 0.25, time->mSampleTime);
        data->mBuffers[channel].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}
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
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    CHECK(bundle && CFBundleLoadExecutable(bundle));
    CFDictionaryRef info = CFBundleGetInfoDictionary(bundle);
    CFArrayRef components = static_cast<CFArrayRef>(CFDictionaryGetValue(info, CFSTR("AudioComponents")));
    CHECK(components && CFArrayGetCount(components) == 1);
    CFDictionaryRef componentInfo = static_cast<CFDictionaryRef>(CFArrayGetValueAtIndex(components, 0));
    auto fourcc = [](CFDictionaryRef dict, CFStringRef key) {
        char text[5] = {};
        auto value = static_cast<CFStringRef>(CFDictionaryGetValue(dict, key));
        CHECK(value && CFStringGetCString(value, text, 5, kCFStringEncodingUTF8));
        return OSType(text[0] << 24 | text[1] << 16 | text[2] << 8 | text[3]);
    };
    const OSType type = fourcc(componentInfo, CFSTR("type"));
    const OSType subtype = fourcc(componentInfo, CFSTR("subtype"));
    const OSType manufacturer = fourcc(componentInfo, CFSTR("manufacturer"));
    auto factoryName = static_cast<CFStringRef>(CFDictionaryGetValue(componentInfo, CFSTR("factoryFunction")));
    CHECK(factoryName);
    auto factory = reinterpret_cast<AudioComponentFactoryFunction>(CFBundleGetFunctionPointerForName(bundle, factoryName));
    CHECK(factory);
    AudioComponentDescription desc{type, subtype, manufacturer, 0, 0};
    AudioComponent component = AudioComponentRegister(&desc, CFSTR("OpenPlugin shell proof"), 0, factory);
    CHECK(component);
    OpShellInfo shellInfo{};
    CHECK(op_shell_info(&shellInfo) != 0);
    for (UInt32 channels : {1u, 2u}) {
        ReferenceShell reference(channels);
        CHECK(reference.instance);
        AudioUnit unit = nullptr;
        CHECK(AudioComponentInstanceNew(component, &unit) == noErr && unit);
        AudioStreamBasicDescription format{};
        format.mSampleRate = 48000;
        format.mFormatID = kAudioFormatLinearPCM;
        format.mFormatFlags = static_cast<UInt32>(kAudioFormatFlagsNativeFloatPacked) | static_cast<UInt32>(kAudioFormatFlagIsNonInterleaved);
        format.mBytesPerPacket = sizeof(float);
        format.mFramesPerPacket = 1;
        format.mBytesPerFrame = sizeof(float);
        format.mChannelsPerFrame = channels;
        format.mBitsPerChannel = 32;
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &format, sizeof(format)) == noErr);
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &format, sizeof(format)) == noErr);
        UInt32 maximum = 4096;
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &maximum, sizeof(maximum)) == noErr);
        AURenderCallbackStruct callback{render_input, nullptr};
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback, sizeof(callback)) == noErr);
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
        CHECK(AudioUnitReset(unit, kAudioUnitScope_Global, 0) == noErr);
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
    }
#if OP_HAS_TAIL
    std::puts("AU proof: factory from the bundle plist, parameter metadata, latency, tail, shell-equal audio, state round-trip, rejected NaN and out-of-range values: PASS");
#else
    std::puts("AU proof: factory from the bundle plist, parameter metadata, latency, shell-equal audio, state round-trip, rejected NaN and out-of-range values, tail not built (OP_TAIL=OFF): PASS");
#endif
    return 0;
}
