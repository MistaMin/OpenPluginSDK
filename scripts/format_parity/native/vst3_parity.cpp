// SPDX-License-Identifier: MIT
// Format parity, VST3: renders the shared scenario through a built .vst3 with
// the VST3 hosting interfaces only (no op_shell_* calls). Parameters are
// found by title and converted with the controller. Every event is a point of
// an IParameterChanges queue at its sample offset in the block: the VST3 shell
// is sample-accurate, so no sub-blocks are cut.
//
//   ParityVST3 <bundle> <scenario> <output>
#include "scenario.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ustring.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <string>
using namespace Steinberg;
using namespace Steinberg::Vst;

static std::string ascii_of(const TChar* text) {
    char buffer[128] = {};
    UString(const_cast<TChar*>(text), 128).toAscii(buffer, 128);
    return buffer;
}

int main(int argc, char** argv) {
    PARITY_CHECK(argc == 4);
    const ParityScenario s = parity_read(argv[2]);
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    PARITY_CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    PARITY_CHECK(bundle && CFBundleLoadExecutable(bundle));
    auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
    auto exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
    PARITY_CHECK(entry && exitFn && getFactory && entry(bundle));
    IPluginFactory* factory = getFactory();
    PARITY_CHECK(factory);
    PClassInfo effect{};
    bool found = false;
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo info{};
        PARITY_CHECK(factory->getClassInfo(i, &info) == kResultOk);
        if (std::strcmp(info.category, kVstAudioEffectClass) == 0) { effect = info; found = true; }
    }
    PARITY_CHECK(found);
    IComponent* component = nullptr;
    PARITY_CHECK(factory->createInstance(effect.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
    PARITY_CHECK(component->initialize(nullptr) == kResultOk);
    IAudioProcessor* processor = nullptr;
    PARITY_CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
    TUID controllerId{};
    PARITY_CHECK(component->getControllerClassId(controllerId) == kResultOk);
    IEditController* controller = nullptr;
    PARITY_CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
    PARITY_CHECK(controller->initialize(nullptr) == kResultOk);

    ParamID ids[2] = {~0u, ~0u};
    for (int32 i = 0; i < controller->getParameterCount(); ++i) {
        ParameterInfo info{};
        PARITY_CHECK(controller->getParameterInfo(i, info) == kResultOk);
        for (uint32_t p = 0; p < 2; ++p) if (ascii_of(info.title) == parity_param_name(p)) ids[p] = info.id;
    }
    PARITY_CHECK(ids[0] != ~0u && ids[1] != ~0u);

    SpeakerArrangement arrangement = SpeakerArr::kStereo;
    PARITY_CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
    PARITY_CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
    PARITY_CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    ProcessSetup setup{kRealtime, kSample32, static_cast<int32>(s.block), s.rate};
    PARITY_CHECK(processor->setupProcessing(setup) == kResultOk);
    PARITY_CHECK(component->setActive(true) == kResultOk);
    PARITY_CHECK(processor->setProcessing(true) == kResultOk);

    std::vector<float> out[2] = {std::vector<float>(s.frames), std::vector<float>(s.frames)};
    for (uint32_t start = 0; start < s.frames; start += s.block) {
        const uint32_t end = std::min(start + s.block, s.frames);
        const uint32_t n = end - start;
        // The host owns the input; the plugin writes a separate output.
        std::vector<float> inL(s.input[0].begin() + start, s.input[0].begin() + end);
        std::vector<float> inR(s.input[1].begin() + start, s.input[1].begin() + end);
        std::vector<float> outL(n), outR(n);
        float* inPtrs[] = {inL.data(), inR.data()};
        float* outPtrs[] = {outL.data(), outR.data()};
        AudioBusBuffers input{};
        input.numChannels = 2;
        input.channelBuffers32 = inPtrs;
        AudioBusBuffers output{};
        output.numChannels = 2;
        output.channelBuffers32 = outPtrs;
        ParameterChanges changes;
        bool any = false;
        for (uint32_t p = 0; p < 2; ++p) {
            int32 index = 0;
            IParamValueQueue* queue = nullptr;
            for (const auto& e : s.events) {
                if (e.param != p || e.frame < start || e.frame >= end) continue;
                if (!queue) queue = changes.addParameterData(ids[p], index);
                int32 pointIndex = 0;
                queue->addPoint(static_cast<int32>(e.frame - start), controller->plainParamToNormalized(ids[p], e.value), pointIndex);
                any = true;
            }
        }
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = static_cast<int32>(n);
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input;
        data.outputs = &output;
        data.inputParameterChanges = any ? &changes : nullptr;
        PARITY_CHECK(processor->process(data) == kResultOk);
        std::copy(outL.begin(), outL.end(), out[0].begin() + start);
        std::copy(outR.begin(), outR.end(), out[1].begin() + start);
    }
    PARITY_CHECK(processor->setProcessing(false) == kResultOk);
    PARITY_CHECK(component->setActive(false) == kResultOk);
    parity_write(argv[3], out);
    return 0;
}
