// SPDX-License-Identifier: MIT
// A host that enlarges the block size while the plugin is active. The VST3 rules
// call setupProcessing only while inactive, but a host may send it at any time.
// The engine and the shell's dry (bypass) buffers were made for the block size
// of its activation, so the shell must cut every host block to that size: a
// larger chunk overran the dry buffer (heap corruption; here a crash at the
// next deactivation). The shell is built
// over latency_stub.cpp (halves the signal).
//
//   ctest --test-dir build-shell -R vst3_block_size_proof --output-on-failure
#include "proof_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 block size proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

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
    SpeakerArrangement stereo = SpeakerArr::kStereo;
    CHECK(processor->setBusArrangements(&stereo, 1, &stereo, 1) == kResultOk);
    CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
    CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    ProcessSetup small{kRealtime, kSample32, 64, 48000.0};
    CHECK(processor->setupProcessing(small) == kResultOk);
    CHECK(component->setActive(true) == kResultOk);
    CHECK(processor->setProcessing(true) == kResultOk);

    // The host now says blocks may be 1024 long, without deactivating first, and sends them.
    ProcessSetup large{kOffline, kSample32, 1024, 48000.0};
    processor->setupProcessing(large);
    int start = 0;
    double worst = 0.0;
    for (int block = 0; block < 8; ++block) {
        const int frames = block % 2 ? 1024 : 1000;
        std::vector<float> inL(frames), inR(frames), outL(frames, 9.0f), outR(frames, 9.0f);
        fill_sine(inL.data(), frames, 48000.0, 1000.0, 0.25, start);
        fill_sine(inR.data(), frames, 48000.0, 1000.0, 0.25, start);
        float* inPtrs[] = {inL.data(), inR.data()};
        float* outPtrs[] = {outL.data(), outR.data()};
        AudioBusBuffers input{};
        input.numChannels = 2;
        input.channelBuffers32 = inPtrs;
        AudioBusBuffers output{};
        output.numChannels = 2;
        output.channelBuffers32 = outPtrs;
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = frames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input;
        data.outputs = &output;
        CHECK(processor->process(data) == kResultOk);
        for (int i = 0; i < frames; ++i) {
            // The stub halves the input (its reported latency only delays the bypass path).
            const double want = 0.5 * 0.25 * std::sin(2.0 * M_PI * 1000.0 * (start + i) / 48000.0);
            worst = std::max(worst, std::fabs(outL[i] - want));
            worst = std::max(worst, std::fabs(outR[i] - want));
        }
        start += frames;
    }
    CHECK(worst < 1e-6);
    CHECK(processor->setProcessing(false) == kResultOk);
    CHECK(component->setActive(false) == kResultOk);
    processor->release();
    CHECK(controller->terminate() == kResultOk);
    controller->release();
    CHECK(component->terminate() == kResultOk);
    component->release();
    factory->release();
    CHECK(exitFn());
    CFRelease(bundle);
    std::printf("VST3 block size proof: activated for 64-sample blocks, then setupProcessing(1024) while active and blocks of 1000 and 1024: every block processed, max difference from the stub's output %.3g: PASS\n", worst);
    return 0;
}
