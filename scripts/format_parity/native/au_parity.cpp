// SPDX-License-Identifier: MIT
// Format parity, Audio Unit: renders the shared scenario through a built
// .component, registered in this process only (nothing is installed), with
// AudioToolbox only (no op_shell_* calls).
//
// Contract: the Audio Unit shell reads its parameters once at the start of a
// render call (AUEffectBase scheduled parameter ramps are not used), so a
// change takes effect on a render-call boundary. The host therefore cuts a
// render call at every event frame and sets the parameters before it.
//
//   ParityAU <bundle> <scenario> <output>
#include "scenario.h"
#include "proof_au.h"

static const ParityScenario* gScenario = nullptr;

// The unit pulls its input here: the scenario input at the stamp's sample time.
static OSStatus parity_input(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp* time, UInt32, UInt32 frames, AudioBufferList* data) {
    const size_t start = static_cast<size_t>(time->mSampleTime);
    for (UInt32 ch = 0; ch < data->mNumberBuffers && ch < 2; ++ch) {
        auto* samples = static_cast<float*>(data->mBuffers[ch].mData);
        if (!samples) return kAudio_ParamError;
        std::memcpy(samples, gScenario->input[ch].data() + start, frames * sizeof(float));
        data->mBuffers[ch].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}

int main(int argc, char** argv) {
    PARITY_CHECK(argc == 4);
    const ParityScenario s = parity_read(argv[2]);
    gScenario = &s;
    AudioComponent component = proof_register_component(argv[1]);
    AudioUnit unit = nullptr;
    PARITY_CHECK(AudioComponentInstanceNew(component, &unit) == noErr && unit);
    AudioStreamBasicDescription format{};
    format.mSampleRate = s.rate;
    format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags = static_cast<UInt32>(kAudioFormatFlagsNativeFloatPacked) | static_cast<UInt32>(kAudioFormatFlagIsNonInterleaved);
    format.mBytesPerPacket = sizeof(float);
    format.mFramesPerPacket = 1;
    format.mBytesPerFrame = sizeof(float);
    format.mChannelsPerFrame = 2;
    format.mBitsPerChannel = 32;
    PARITY_CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &format, sizeof(format)) == noErr);
    PARITY_CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &format, sizeof(format)) == noErr);
    UInt32 maxFrames = s.block;
    PARITY_CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &maxFrames, sizeof(maxFrames)) == noErr);
    AURenderCallbackStruct callback{parity_input, nullptr};
    PARITY_CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback, sizeof(callback)) == noErr);

    // Parameter ids by name.
    AudioUnitParameterID ids[2] = {~0u, ~0u};
    UInt32 size = 0;
    PARITY_CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &size, nullptr) == noErr);
    std::vector<AudioUnitParameterID> list(size / sizeof(AudioUnitParameterID));
    PARITY_CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, list.data(), &size) == noErr);
    for (AudioUnitParameterID id : list) {
        AudioUnitParameterInfo info{};
        UInt32 infoSize = sizeof(info);
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &info, &infoSize) != noErr) continue;
        for (uint32_t p = 0; p < 2; ++p) if (std::strcmp(info.name, parity_param_name(p)) == 0) ids[p] = id;
    }
    PARITY_CHECK(ids[0] != ~0u && ids[1] != ~0u);
    PARITY_CHECK(AudioUnitInitialize(unit) == noErr);

    std::vector<float> out[2] = {std::vector<float>(s.frames), std::vector<float>(s.frames)};
    const auto cuts = parity_cuts(s);
    ProofBuffers buffers(2, s.block);
    for (size_t k = 0; k + 1 < cuts.size(); ++k) {
        const uint32_t a = cuts[k], b = cuts[k + 1];
        for (const auto& e : s.events) {
            if (e.frame == a) PARITY_CHECK(AudioUnitSetParameter(unit, ids[e.param], kAudioUnitScope_Global, 0, e.value, 0) == noErr);
        }
        PARITY_CHECK(proof_render(unit, buffers, b - a, static_cast<double>(a)) == noErr);
        for (int ch = 0; ch < 2; ++ch) std::copy(buffers.data[ch].begin(), buffers.data[ch].begin() + (b - a), out[ch].begin() + a);
    }
    AudioUnitUninitialize(unit);
    AudioComponentInstanceDispose(unit);
    parity_write(argv[3], out);
    return 0;
}
