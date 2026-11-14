// SPDX-License-Identifier: MIT
// Helpers shared by the in-process Audio Unit proofs: load a built bundle's
// factory in this process only (nothing is installed), make a configured
// unit, render, and the bypass checks. These are not DAW tests.
#pragma once
#include "proof_shell.h"
#include <AudioToolbox/AudioToolbox.h>
#include <CoreFoundation/CoreFoundation.h>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#ifndef CHECK
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "AU proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
#endif

constexpr UInt32 kProofBlock = 512;

inline OSStatus proof_render_input(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp* time, UInt32, UInt32 frames, AudioBufferList* data) {
    for (UInt32 channel = 0; channel < data->mNumberBuffers; ++channel) {
        auto* samples = static_cast<float*>(data->mBuffers[channel].mData);
        if (!samples) return kAudio_ParamError;
        fill_sine(samples, static_cast<int>(frames), 48000.0, 1000.0, 0.25, time->mSampleTime);
        data->mBuffers[channel].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}

/// Registers the factory named in the bundle's plist, in this process only.
inline AudioComponent proof_register_component(const char* bundle_path) {
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(bundle_path), static_cast<CFIndex>(std::strlen(bundle_path)), true);
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
    AudioComponentDescription desc{fourcc(componentInfo, CFSTR("type")), fourcc(componentInfo, CFSTR("subtype")), fourcc(componentInfo, CFSTR("manufacturer")), 0, 0};
    auto factoryName = static_cast<CFStringRef>(CFDictionaryGetValue(componentInfo, CFSTR("factoryFunction")));
    CHECK(factoryName);
    auto factory = reinterpret_cast<AudioComponentFactoryFunction>(CFBundleGetFunctionPointerForName(bundle, factoryName));
    CHECK(factory);
    AudioComponent component = AudioComponentRegister(&desc, CFSTR("OpenPlugin shell proof"), 0, factory);
    CHECK(component);
    return component;
}

/// A unit with 48 kHz non-interleaved float streams and the proof tone as input.
/// Not initialized: the caller sets parameters first if it wants to.
inline AudioUnit proof_make_unit(AudioComponent component, UInt32 channels, UInt32 max_frames = 4096) {
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
    CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &max_frames, sizeof(max_frames)) == noErr);
    AURenderCallbackStruct callback{proof_render_input, nullptr};
    CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback, sizeof(callback)) == noErr);
    return unit;
}

/// Output buffers for repeated renders, so a render call itself allocates nothing.
struct ProofBuffers {
    UInt32 channels;
    std::vector<std::vector<float>> data;
    struct List { UInt32 count; AudioBuffer buffer[2]; } list{};
    ProofBuffers(UInt32 channel_count, UInt32 frames) : channels(channel_count), data(channel_count, std::vector<float>(frames)) {}
    AudioBufferList* prepare(UInt32 frames) {
        list.count = channels;
        for (UInt32 ch = 0; ch < channels; ++ch) list.buffer[ch] = {1, frames * static_cast<UInt32>(sizeof(float)), data[ch].data()};
        return reinterpret_cast<AudioBufferList*>(&list);
    }
};

/// One render call of `frames` frames starting at sample `start`.
inline OSStatus proof_render(AudioUnit unit, ProofBuffers& buffers, UInt32 frames, double start) {
    AudioTimeStamp stamp{};
    stamp.mFlags = kAudioTimeStampSampleTimeValid;
    stamp.mSampleTime = start;
    AudioUnitRenderActionFlags flags = 0;
    return AudioUnitRender(unit, &flags, &stamp, 0, frames, buffers.prepare(frames));
}

/// `blocks` blocks of 512 frames from block `first` on; every output sample of every channel.
inline std::vector<std::vector<float>> proof_render_run(AudioUnit unit, UInt32 channels, int first, int blocks) {
    ProofBuffers buffers(channels, kProofBlock);
    std::vector<std::vector<float>> all(channels);
    for (int block = first; block < first + blocks; ++block) {
        CHECK(proof_render(unit, buffers, kProofBlock, static_cast<double>(block) * kProofBlock) == noErr);
        for (UInt32 ch = 0; ch < channels; ++ch) all[ch].insert(all[ch].end(), buffers.data[ch].begin(), buffers.data[ch].end());
    }
    return all;
}

inline float proof_tone(long index) {
    float value = 0.0f;
    fill_sine(&value, 1, 48000.0, 1000.0, 0.25, static_cast<double>(index));
    return value;
}

inline void proof_set_bypass(AudioUnit unit, bool on) {
    UInt32 flag = on ? 1 : 0;
    CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_BypassEffect, kAudioUnitScope_Global, 0, &flag, sizeof(flag)) == noErr);
    UInt32 read = 99;
    UInt32 size = sizeof(read);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_BypassEffect, kAudioUnitScope_Global, 0, &read, &size) == noErr);
    CHECK(read == flag);
}

/// Bypass contract, on an initialized unit that is processing and not bypassed, fed the
/// proof tone, with `latency` samples reported. It renders blocks 0 to 19 of the tone.
///  - the unit reports bypass support;
///  - bypassed, the output is the input delayed by `latency`, exactly;
///  - the switch crossfades: no step larger than the tone itself makes;
///  - a reset while bypassed empties the delay line;
///  - switching back restores the processed signal, which differs from the dry one.
inline void proof_check_bypass(AudioUnit unit, UInt32 channels, const char* label, UInt32 latency) {
    UInt32 size = 0;
    Boolean writable = false;
    CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_BypassEffect, kAudioUnitScope_Global, 0, &size, &writable) == noErr);
    CHECK(size == sizeof(UInt32) && writable);
    UInt32 initial = 99;
    UInt32 initialSize = sizeof(initial);
    CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_BypassEffect, kAudioUnitScope_Global, 0, &initial, &initialSize) == noErr);
    CHECK(initial == 0);

    constexpr long kB = kProofBlock;
    const auto wet = proof_render_run(unit, channels, 0, 4);  // blocks 0 to 3
    double wetVersusDry = 0.0;
    for (long n = 3 * kB; n < 4 * kB; ++n) wetVersusDry = std::max(wetVersusDry, std::fabs(static_cast<double>(wet[0][static_cast<size_t>(n)]) - proof_tone(n - latency)));
    std::printf("AU %s bypass: processed differs from delayed dry by %.3g\n", label, wetVersusDry);
    CHECK(wetVersusDry > 0.01);  // otherwise the checks below could not tell bypass from processing

    proof_set_bypass(unit, true);
    const auto bypassed = proof_render_run(unit, channels, 4, 8);  // blocks 4 to 11
    double worstStep = 0.0;
    for (UInt32 ch = 0; ch < channels; ++ch) {
        for (long i = 1; i < kB; ++i) worstStep = std::max(worstStep, std::fabs(static_cast<double>(bypassed[ch][static_cast<size_t>(i)]) - bypassed[ch][static_cast<size_t>(i - 1)]));
    }
    std::printf("AU %s bypass: largest step while crossfading %.4f\n", label, worstStep);
    CHECK(worstStep < 0.1);
    double worstDry = 0.0;
    for (UInt32 ch = 0; ch < channels; ++ch) {
        for (long i = kB; i < 8 * kB; ++i) {  // after the crossfade: 10 ms is under one block
            const long n = 4 * kB + i;
            worstDry = std::max(worstDry, std::fabs(static_cast<double>(bypassed[ch][static_cast<size_t>(i)]) - proof_tone(n - latency)));
        }
    }
    std::printf("AU %s bypass: output against input delayed by %u samples: max difference %.3g\n", label, latency, worstDry);
    CHECK(worstDry == 0.0);

    // A reset while bypassed empties the delay line: the first `latency` samples are silent.
    if (latency > 0) {
        CHECK(AudioUnitReset(unit, kAudioUnitScope_Global, 0) == noErr);
        const auto afterReset = proof_render_run(unit, channels, 12, 1);
        for (UInt32 ch = 0; ch < channels; ++ch) {
            for (long i = 0; i < kB; ++i) {
                const float expected = i < static_cast<long>(latency) ? 0.0f : proof_tone(12 * kB + i - latency);
                CHECK(afterReset[ch][static_cast<size_t>(i)] == expected);
            }
        }
        std::printf("AU %s bypass: reset empties the delay line\n", label);
    }

    proof_set_bypass(unit, false);
    const long base = 14;
    const auto back = proof_render_run(unit, channels, static_cast<int>(base), 6);  // blocks 14 to 19
    // The tone repeats every 48 samples, so block 3 holds the processed signal to expect.
    double worstWet = 0.0;
    double worstBackDry = 0.0;
    for (UInt32 ch = 0; ch < channels; ++ch) {
        for (long i = 2 * kB; i < 6 * kB; ++i) {  // after the crossfade
            const long n = base * kB + i;
            const long ref = 3 * kB + ((n - 3 * kB) % 48);
            worstWet = std::max(worstWet, std::fabs(static_cast<double>(back[ch][static_cast<size_t>(i)]) - wet[ch][static_cast<size_t>(ref)]));
            worstBackDry = std::max(worstBackDry, std::fabs(static_cast<double>(back[ch][static_cast<size_t>(i)]) - proof_tone(n - latency)));
        }
    }
    std::printf("AU %s bypass off: processed signal restored, max difference %.3g, distance from dry %.3g\n", label, worstWet, worstBackDry);
    CHECK(worstWet < 1e-5);
    CHECK(worstBackDry > 0.01);
}
