// SPDX-License-Identifier: MIT
// In-process VST3 host for the FreEQ shell. Does not install or scan a plug-in folder.
#include "op_shell.h"
#include "proof_eq.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ustring.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <cmath>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 proof failed line %d: %s\n", __LINE__, #cond); return 1; } } while (0)
static void change(ParameterChanges& changes, ParamID id, double value) {
    int32 index = 0;
    auto* queue = changes.addParameterData(id, index);
    queue->addPoint(0, value, index);
}
static std::string title_of(const ParameterInfo& info) {
    char buffer[128] = {};
    UString(const_cast<TChar*>(info.title), 128).toAscii(buffer, 128);
    return buffer;
}
static int find_param(IEditController* controller, const char* name) {
    const int32 count = controller->getParameterCount();
    for (int32 i = 0; i < count; ++i) {
        ParameterInfo info{};
        if (controller->getParameterInfo(i, info) != kResultOk) continue;
        if (title_of(info) == name) return static_cast<int>(info.id);
    }
    return -1;
}
static bool hex32(const char* text) {
    if (std::strlen(text) != 32) return false;
    for (int i = 0; i < 32; ++i) {
        const char c = text[i];
        if (!((c >= '0' && c <= '9') || (c >= 'A' && c <= 'F') || (c >= 'a' && c <= 'f'))) return false;
    }
    return true;
}
static int render_gain(IAudioProcessor* processor, IComponent* component, int channels, const std::vector<std::pair<ParamID, double>>& normalized, double* measured) {
    constexpr int kFrames = 512;
    constexpr int kBlocks = 12;
    std::vector<float> left(kFrames), right(kFrames), outL(kFrames), outR(kFrames), dry(kFrames);
    float* inPtrs[] = {left.data(), right.data()};
    float* outPtrs[] = {outL.data(), outR.data()};
    AudioBusBuffers in{};
    in.numChannels = channels;
    in.channelBuffers32 = inPtrs;
    AudioBusBuffers out{};
    out.numChannels = channels;
    out.channelBuffers32 = outPtrs;
    ParameterChanges changes;
    for (auto [id, value] : normalized) change(changes, id, value);
    for (int block = 0; block < kBlocks; ++block) {
        fill_sine(left.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (channels > 1) fill_sine(right.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        dry = left;
        ProcessData data{};
        data.processMode = kOffline;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kFrames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &in;
        data.outputs = &out;
        data.inputParameterChanges = &changes;
        if (processor->process(data) != kResultOk) return 1;
        for (int i = 0; i < kFrames; ++i) if (!std::isfinite(outL[i])) return 1;
        if (block + 1 == kBlocks) *measured = gain_db(dry.data(), outL.data(), 256, 256);
    }
    out.channelBuffers32 = inPtrs;
    fill_sine(left.data(), kFrames, 48000.0, 1000.0, 0.25, 0);
    dry = left;
    ProcessData inplace{};
    inplace.processMode = kOffline;
    inplace.symbolicSampleSize = kSample32;
    inplace.numSamples = kFrames;
    inplace.numInputs = inplace.numOutputs = 1;
    inplace.inputs = &in;
    inplace.outputs = &out;
    inplace.inputParameterChanges = &changes;
    if (processor->process(inplace) != kResultOk) return 1;
    const double inplaceDb = gain_db(dry.data(), left.data(), 256, 256);
    if (std::fabs(inplaceDb - *measured) > 0.35) return 1;
    (void)component;
    return 0;
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
    CHECK(factory && factory->countClasses() == 2);
    PClassInfo effect{};
    bool found = false;
    char ids[2][33] = {};
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo info{};
        CHECK(factory->getClassInfo(i, &info) == kResultOk);
        FUID uid = FUID::fromTUID(info.cid);
        uid.toString(ids[i]);
        CHECK(hex32(ids[i]));
        if (std::strcmp(info.category, kVstAudioEffectClass) == 0) {
            effect = info;
            found = true;
        }
    }
    CHECK(found);
    CHECK(std::strcmp(ids[0], ids[1]) != 0);
    for (int channels : {1, 2}) {
        IComponent* component = nullptr;
        CHECK(factory->createInstance(effect.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        IAudioProcessor* processor = nullptr;
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        IEditController* controller = nullptr;
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        CHECK(controller->getParameterCount() == 26);
        CHECK(processor->canProcessSampleSize(kSample64) == kResultFalse);
        SpeakerArrangement arrangement = channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kOffline, kSample32, 4096, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        CHECK(processor->getLatencySamples() == 0);
        auto id = [&](const char* name) -> ParamID {
            const int foundId = find_param(controller, name);
            if (foundId < 0) {
                std::fprintf(stderr, "VST3 proof missing parameter %s\n", name);
                std::exit(1);
            }
            return static_cast<ParamID>(foundId);
        };
        auto plain = [&](const char* name, double value) {
            const ParamID param = id(name);
            return std::pair<ParamID, double>{param, controller->plainParamToNormalized(param, value)};
        };
        std::vector<std::pair<ParamID, double>> flat;
        std::vector<std::pair<ParamID, double>> boosted;
        for (int band = 1; band <= 6; ++band) {
            char typeName[32];
            char gainName[32];
            std::snprintf(typeName, sizeof(typeName), "Band %d Type", band);
            std::snprintf(gainName, sizeof(gainName), "Band %d Gain", band);
            flat.push_back(plain(typeName, 0.0));
            flat.push_back(plain(gainName, 0.0));
            boosted.push_back(plain(typeName, 0.0));
            boosted.push_back(plain(gainName, band == 4 ? 6.0 : 0.0));
        }
        boosted.push_back(plain("Band 4 Freq", 1000.0));
        boosted.push_back(plain("Band 4 Q", 1.0));
        for (const char* name : {"Phase", "Trim"}) {
            flat.push_back(plain(name, 0.0));
            boosted.push_back(plain(name, 0.0));
        }
        double unity = 99.0;
        CHECK(render_gain(processor, component, channels, flat, &unity) == 0);
        CHECK(std::fabs(unity) < 0.05);
        double bell = 0.0;
        CHECK(render_gain(processor, component, channels, boosted, &bell) == 0);
        std::printf("VST3 %s Band 4 at 1 kHz: %.2f dB\n", channels == 1 ? "mono" : "stereo", bell);
        CHECK(std::fabs(bell - 6.0) < 0.2);
        MemoryStream saved;
        CHECK(component->getState(&saved) == kResultOk);
        double back = 99.0;
        CHECK(render_gain(processor, component, channels, flat, &back) == 0);
        CHECK(std::fabs(back) < 0.05);
        saved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&saved) == kResultOk);
        saved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(controller->setComponentState(&saved) == kResultOk);
        double restored = 0.0;
        CHECK(render_gain(processor, component, channels, {}, &restored) == 0);
        CHECK(std::fabs(restored - 6.0) < 0.2);
        MemoryStream invalid;
        char bad[4] = {};
        CHECK(invalid.write(bad, 4, nullptr) == kResultOk);
        invalid.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&invalid) != kResultOk);
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        processor->release();
        CHECK(controller->terminate() == kResultOk);
        controller->release();
        CHECK(component->terminate() == kResultOk);
        component->release();
    }
    factory->release();
    CHECK(exitFn());
    CFRelease(bundle);
    std::puts("VST3 proof: factory, 32-hex class IDs, 26 parameters, mono/stereo, unity, +6 dB, state, in-place: PASS");
    return 0;
}
