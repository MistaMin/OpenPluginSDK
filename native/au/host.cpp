// SPDX-License-Identifier: MIT
// In-process Audio Unit host. Registers the factory in this process only.
#include "proof_eq.h"
#include <AudioToolbox/AudioToolbox.h>
#include <CoreFoundation/CoreFoundation.h>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <string>
#include <vector>
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "AU proof failed line %d: %s\n", __LINE__, #cond); return 1; } } while (0)
static OSStatus render_input(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp* time, UInt32, UInt32 frames, AudioBufferList* data) {
    for (UInt32 channel = 0; channel < data->mNumberBuffers; ++channel) {
        auto* samples = static_cast<float*>(data->mBuffers[channel].mData);
        if (!samples) return kAudio_ParamError;
        fill_sine(samples, static_cast<int>(frames), 48000.0, 1000.0, 0.25, time->mSampleTime);
        data->mBuffers[channel].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}
static int find_param(AudioUnit unit, const char* name) {
    UInt32 size = 0;
    if (AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &size, nullptr) != noErr) return -1;
    const UInt32 count = size / sizeof(AudioUnitParameterID);
    std::vector<AudioUnitParameterID> ids(count);
    if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, ids.data(), &size) != noErr) return -1;
    for (AudioUnitParameterID id : ids) {
        AudioUnitParameterInfo info{};
        UInt32 infoSize = sizeof(info);
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &info, &infoSize) != noErr) continue;
        if (std::strcmp(info.name, name) == 0) return static_cast<int>(id);
    }
    return -1;
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
        CFStringGetCString(value, text, 5, kCFStringEncodingUTF8);
        return OSType(text[0] << 24 | text[1] << 16 | text[2] << 8 | text[3]);
    };
    const OSType type = fourcc(componentInfo, CFSTR("type"));
    const OSType subtype = fourcc(componentInfo, CFSTR("subtype"));
    const OSType manufacturer = fourcc(componentInfo, CFSTR("manufacturer"));
    auto factoryName = static_cast<CFStringRef>(CFDictionaryGetValue(componentInfo, CFSTR("factoryFunction")));
    auto factory = reinterpret_cast<AudioComponentFactoryFunction>(CFBundleGetFunctionPointerForName(bundle, factoryName));
    CHECK(factory);
    AudioComponentDescription desc{type, subtype, manufacturer, 0, 0};
    AudioComponent component = AudioComponentRegister(&desc, CFSTR("FreEQ shell proof"), 0, factory);
    CHECK(component);
    for (UInt32 channels : {1u, 2u}) {
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
        auto id = [&](const char* name) {
            const int found = find_param(unit, name);
            if (found < 0) {
                std::fprintf(stderr, "AU proof missing parameter %s\n", name);
                std::exit(1);
            }
            return static_cast<AudioUnitParameterID>(found);
        };
        CHECK(AudioUnitSetParameter(unit, id("Trim"), kAudioUnitScope_Global, 0, std::numeric_limits<float>::quiet_NaN(), 0) != noErr);
        CHECK(AudioUnitInitialize(unit) == noErr);
        Float64 latency = 1.0;
        UInt32 latencySize = sizeof(latency);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_Latency, kAudioUnitScope_Global, 0, &latency, &latencySize) == noErr);
        CHECK(latency == 0.0);
        for (int band = 1; band <= 6; ++band) {
            char typeName[32];
            char gainName[32];
            std::snprintf(typeName, sizeof(typeName), "Band %d Type", band);
            std::snprintf(gainName, sizeof(gainName), "Band %d Gain", band);
            CHECK(AudioUnitSetParameter(unit, id(typeName), kAudioUnitScope_Global, 0, 0.f, 0) == noErr);
            CHECK(AudioUnitSetParameter(unit, id(gainName), kAudioUnitScope_Global, 0, 0.f, 0) == noErr);
        }
        CHECK(AudioUnitSetParameter(unit, id("Phase"), kAudioUnitScope_Global, 0, 0.f, 0) == noErr);
        CHECK(AudioUnitSetParameter(unit, id("Trim"), kAudioUnitScope_Global, 0, 0.f, 0) == noErr);
        constexpr UInt32 kFrames = 512;
        std::vector<std::vector<float>> output(channels, std::vector<float>(kFrames));
        struct List { UInt32 count; AudioBuffer data[2]; } list{};
        list.count = channels;
        for (UInt32 channel = 0; channel < channels; ++channel) list.data[channel] = {1, kFrames * sizeof(float), output[channel].data()};
        AudioTimeStamp stamp{};
        stamp.mFlags = kAudioTimeStampSampleTimeValid;
        auto render_blocks = [&](int blocks, double* measured) {
            std::vector<float> dry(kFrames);
            for (int block = 0; block < blocks; ++block) {
                stamp.mSampleTime = block * kFrames;
                AudioUnitRenderActionFlags flags = 0;
                if (AudioUnitRender(unit, &flags, &stamp, 0, kFrames, reinterpret_cast<AudioBufferList*>(&list)) != noErr) return 1;
                for (float sample : output[0]) if (!std::isfinite(sample)) return 1;
                if (block + 1 == blocks) {
                    fill_sine(dry.data(), kFrames, 48000.0, 1000.0, 0.25, stamp.mSampleTime);
                    *measured = gain_db(dry.data(), output[0].data(), 256, 256);
                }
            }
            return 0;
        };
        double unity = 99.0;
        CHECK(render_blocks(8, &unity) == 0);
        CHECK(std::fabs(unity) < 0.05);
        CHECK(AudioUnitSetParameter(unit, id("Band 4 Freq"), kAudioUnitScope_Global, 0, 1000.f, 0) == noErr);
        CHECK(AudioUnitSetParameter(unit, id("Band 4 Gain"), kAudioUnitScope_Global, 0, 6.f, 0) == noErr);
        CHECK(AudioUnitSetParameter(unit, id("Band 4 Q"), kAudioUnitScope_Global, 0, 1.f, 0) == noErr);
        double bell = 0.0;
        CHECK(render_blocks(12, &bell) == 0);
        std::printf("AU %s Band 4 at 1 kHz: %.2f dB\n", channels == 1 ? "mono" : "stereo", bell);
        CHECK(std::fabs(bell - 6.0) < 0.2);
        CHECK(AudioUnitReset(unit, kAudioUnitScope_Global, 0) == noErr);
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
    }
    std::puts("AU proof: factory from the bundle plist, mono/stereo, NaN rejected, unity, +6 dB, latency 0: PASS");
    return 0;
}
