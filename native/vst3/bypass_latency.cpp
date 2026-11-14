// SPDX-License-Identifier: MIT
// Bypass against a plugin that reports latency. The shell is built over
// latency_stub.cpp (37 samples, halves the signal). While bypassed the output
// must be the input delayed by exactly the reported latency, in odd block sizes
// that wrap the delay line, in place or not. The crossfade into and out of
// bypass must stay between the processed and the delayed dry signal.
//
//   ctest --test-dir build-shell -R vst3_bypass_latency_proof --output-on-failure
#include "proof_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 bypass latency proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

// One block of `frames` starting at global sample `start`. A null `bypass` leaves
// bypass as it is. Returns the output (the input buffer when in place).
static std::vector<float> run_block(IAudioProcessor* processor, int channels, int start, int frames, bool in_place, const double* bypass, ParamID bypassId, int bypassAt = 0, const double* second = nullptr, int secondAt = 0) {
    std::vector<float> inL(frames), inR(frames), outL(frames), outR(frames);
    fill_sine(inL.data(), frames, 48000.0, 1000.0, 0.25, start);
    fill_sine(inR.data(), frames, 48000.0, 1000.0, 0.25, start);
    float* inPtrs[] = {inL.data(), inR.data()};
    float* outPtrs[] = {in_place ? inL.data() : outL.data(), in_place ? inR.data() : outR.data()};
    AudioBusBuffers input{};
    input.numChannels = channels;
    input.channelBuffers32 = inPtrs;
    AudioBusBuffers output{};
    output.numChannels = channels;
    output.channelBuffers32 = outPtrs;
    ParameterChanges changes;
    if (bypass) {
        int32 index = 0;
        auto* queue = changes.addParameterData(bypassId, index);
        queue->addPoint(bypassAt, *bypass, index);
        if (second) queue->addPoint(secondAt, *second, index);
    }
    ProcessData data{};
    data.processMode = kRealtime;
    data.symbolicSampleSize = kSample32;
    data.numSamples = frames;
    data.numInputs = data.numOutputs = 1;
    data.inputs = &input;
    data.outputs = &output;
    data.inputParameterChanges = bypass ? &changes : nullptr;
    CHECK(processor->process(data) == kResultOk);
    return in_place ? inL : outL;
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    CHECK(bundle && CFBundleLoadExecutable(bundle));
    auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
    auto exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
    CHECK(entry && exitFn && getFactory && entry(bundle));
    IPluginFactory* factory = getFactory();
    CHECK(factory);
    TUID effectId{};
    bool found = false;
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo info{};
        CHECK(factory->getClassInfo(i, &info) == kResultOk);
        if (std::strcmp(info.category, kVstAudioEffectClass) == 0) {
            std::memcpy(effectId, info.cid, 16);
            found = true;
        }
    }
    CHECK(found);
    for (int channels : {1, 2}) {
        for (bool in_place : {false, true}) {
            IComponent* component = nullptr;
            CHECK(factory->createInstance(effectId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
            CHECK(component->initialize(nullptr) == kResultOk);
            IAudioProcessor* processor = nullptr;
            CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
            TUID controllerId{};
            CHECK(component->getControllerClassId(controllerId) == kResultOk);
            IEditController* controller = nullptr;
            CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
            CHECK(controller->initialize(nullptr) == kResultOk);
            ParamID bypassId = 0;
            for (int32 i = 0; i < controller->getParameterCount(); ++i) {
                ParameterInfo host{};
                CHECK(controller->getParameterInfo(i, host) == kResultOk);
                if (host.flags & ParameterInfo::kIsBypass) bypassId = host.id;
            }
            CHECK(bypassId != 0);
            SpeakerArrangement arrangement = channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
            CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
            CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
            CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
            ProcessSetup setup{kRealtime, kSample32, 1024, 48000.0};
            CHECK(processor->setupProcessing(setup) == kResultOk);
            CHECK(component->setActive(true) == kResultOk);
            CHECK(processor->setProcessing(true) == kResultOk);
            const int latency = static_cast<int>(processor->getLatencySamples());
            CHECK(latency == 37);

            const int sizes[] = {100, 61, 480, 513, 7, 1000, 33, 256};
            const double on = 1.0;
            const double off = 0.0;
            int start = 0;
            int blockIndex = 0;
            double worstBypassed = 0.0;
            double worstProcessed = 0.0;
            // `exactFrom`: samples from this index on are compared exactly against
            // the expected signal; earlier ones only against the crossfade bounds.
            auto phase = [&](const double* change, int blocks, bool expectDry, bool expectWet, int exactFrom) {
                for (int b = 0; b < blocks; ++b, ++blockIndex) {
                    const int frames = sizes[blockIndex % 8];
                    const auto out = run_block(processor, channels, start, frames, in_place, b == 0 ? change : nullptr, bypassId);
                    for (int i = 0; i < frames; ++i) {
                        const int t = start + i;
                        // Dry is the input `latency` samples earlier; before time zero, silence.
                        const double dry = t - latency >= 0 ? 0.25 * std::sin(2.0 * M_PI * 1000.0 * (t - latency) / 48000.0) : 0.0;
                        const double wet = 0.5 * 0.25 * std::sin(2.0 * M_PI * 1000.0 * t / 48000.0);
                        // Whatever the state, the output lies between the two signals.
                        CHECK(out[i] >= std::min(dry, wet) - 1e-6 && out[i] <= std::max(dry, wet) + 1e-6);
                        if (t < exactFrom) continue;
                        if (expectDry) worstBypassed = std::max(worstBypassed, std::fabs(out[i] - dry));
                        if (expectWet) worstProcessed = std::max(worstProcessed, std::fabs(out[i] - wet));
                    }
                    start += frames;
                }
            };
            phase(nullptr, 3, false, true, 0);   // processed from the start
            phase(&on, 8, false, false, 0);      // 10 ms crossfade into bypass
            const int bypassedFrom = start;
            phase(nullptr, 8, true, false, bypassedFrom);
            CHECK(worstBypassed < 1e-7);
            phase(&off, 8, false, false, 0);     // crossfade back
            const int processedFrom = start;
            phase(nullptr, 8, false, true, processedFrom);
            CHECK(worstProcessed < 1e-6);
            // Bypass switched inside a block: on at sample 300, off again at 700, so the
            // crossfade starts at 300, is cut short at 700 and runs back. The dry signal
            // is the input delayed by the stub's latency, the wet one half of the input.
            {
                const int frames = 900;
                const int onAt = 300;
                const int offAt = 700;
                const auto out = run_block(processor, channels, start, frames, in_place, &on, bypassId, onAt, &off, offAt);
                const float step = static_cast<float>(1.0 / (0.010 * 48000.0));
                float mix = 0.0f;
                double worst = 0.0;
                for (int i = 0; i < frames; ++i) {
                    const float target = i >= onAt && i < offAt ? 1.0f : 0.0f;
                    if (mix < target) mix = std::min(target, mix + step);
                    else if (mix > target) mix = std::max(target, mix - step);
                    const int t = start + i;
                    const double dry = t - latency >= 0 ? 0.25 * std::sin(2.0 * M_PI * 1000.0 * (t - latency) / 48000.0) : 0.0;
                    const double wet = 0.5 * 0.25 * std::sin(2.0 * M_PI * 1000.0 * t / 48000.0);
                    worst = std::max(worst, std::fabs(out[i] - (wet + (dry - wet) * mix)));
                    if (i < onAt) CHECK(std::fabs(out[i] - wet) < 1e-6);
                }
                CHECK(worst < 1e-6);
                CHECK(std::fabs(out[onAt] - 0.5 * 0.25 * std::sin(2.0 * M_PI * 1000.0 * (start + onAt) / 48000.0)) > 1e-7);
                std::printf("VST3 bypass latency %d, %s, %s: bypass on at sample %d and off at %d of one block: max difference from the crossfade model %.3g\n",
                            latency, channels == 1 ? "mono" : "stereo", in_place ? "in place" : "separate buffers", onAt, offAt, worst);
                start += frames;
                // The fade back finishes in the next blocks and processing resumes.
                phase(nullptr, 8, false, true, start + 600);
            }
            std::printf("VST3 bypass latency %d, %s, %s: bypassed output max difference from the delayed input %.3g, processed again %.3g\n",
                        latency, channels == 1 ? "mono" : "stereo", in_place ? "in place" : "separate buffers", worstBypassed, worstProcessed);
            CHECK(processor->setProcessing(false) == kResultOk);
            CHECK(component->setActive(false) == kResultOk);
            processor->release();
            CHECK(controller->terminate() == kResultOk);
            controller->release();
            CHECK(component->terminate() == kResultOk);
            component->release();
        }
    }
    factory->release();
    CHECK(exitFn());
    CFRelease(bundle);
    std::puts("VST3 bypass latency proof: bypassed output is the input delayed by the reported latency (odd block sizes, wrapping delay line), crossfade stays between the signals, processing resumes: PASS");
    return 0;
}
