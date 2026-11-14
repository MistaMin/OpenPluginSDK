// SPDX-License-Identifier: MIT
// In-process VST3 host for the SDK example shell. Does not install or scan a
// plug-in folder. The expected parameter metadata, latency, tail and audio come
// from the op_shell_* C ABI, not from one plugin's constants.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R vst3_proof --output-on-failure
#include "op_shell.h"
#include "proof_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ustring.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
static void change(ParameterChanges& changes, ParamID id, double value) {
    int32 index = 0;
    auto* queue = changes.addParameterData(id, index);
    queue->addPoint(0, value, index);
}
static std::string ascii_of(const TChar* text) {
    char buffer[128] = {};
    UString(const_cast<TChar*>(text), 128).toAscii(buffer, 128);
    return buffer;
}
static uint32_t find_shell_param(const char* name) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        if (std::strcmp(desc.name, name) == 0) return i;
    }
    std::fprintf(stderr, "VST3 proof: the shell has no parameter named %s\n", name);
    std::exit(1);
}
static std::vector<float> shell_defaults() {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    std::vector<float> values(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        values[i] = static_cast<float>(desc.default_plain);
    }
    return values;
}
// Every parameter the host exposes must describe the same parameter the shell
// declares: same id, name, unit, default, step count and automation flag, and
// the same plain <-> normalized mapping at the ends and at the default.
static void check_parameters(IEditController* controller) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    CHECK(info.abi == OP_SHELL_ABI);
    // The plugin's parameters, then the host bypass parameter.
    CHECK(controller->getParameterCount() == static_cast<int32>(info.param_count) + 1);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        ParameterInfo host{};
        CHECK(controller->getParameterInfo(static_cast<int32>(i), host) == kResultOk);
        CHECK(host.id == static_cast<ParamID>(i));
        CHECK(ascii_of(host.title) == desc.name);
        CHECK(ascii_of(host.units) == desc.unit);
        CHECK(std::fabs(host.defaultNormalizedValue - desc.default_normalized) < 1e-9);
        const int32 steps = desc.steps > 1 ? static_cast<int32>(desc.steps - 1) : 0;
        CHECK(host.stepCount == steps);
        CHECK(((host.flags & ParameterInfo::kCanAutomate) != 0) == ((desc.flags & OP_PARAM_AUTOMATABLE) != 0));
        const ParamID id = static_cast<ParamID>(i);
        CHECK(std::fabs(controller->normalizedParamToPlain(id, 0.0) - desc.min) < 1e-9);
        CHECK(std::fabs(controller->normalizedParamToPlain(id, 1.0) - desc.max) < 1e-9);
        CHECK(std::fabs(controller->plainParamToNormalized(id, desc.default_plain) - desc.default_normalized) < 1e-9);
        CHECK(std::fabs(controller->normalizedParamToPlain(id, desc.default_normalized) - desc.default_plain) < 1e-9);
    }
}
// The bypass parameter: found by its flag, not by an assumed id. It must be
// automatable, a two-state switch off by default, and outside 0..n-1 so the
// plugin's own ids stay the ABI indices.
static ParamID check_bypass_parameter(IEditController* controller) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    int found = 0;
    ParamID id = 0;
    for (int32 i = 0; i < controller->getParameterCount(); ++i) {
        ParameterInfo host{};
        CHECK(controller->getParameterInfo(i, host) == kResultOk);
        if ((host.flags & ParameterInfo::kIsBypass) == 0) continue;
        ++found;
        id = host.id;
        CHECK((host.flags & ParameterInfo::kCanAutomate) != 0);
        CHECK(host.stepCount == 1);
        CHECK(host.defaultNormalizedValue == 0.0);
        CHECK(host.id >= info.param_count);
        CHECK(ascii_of(host.title) == "Bypass");
    }
    CHECK(found == 1);
    CHECK(controller->getParamNormalized(id) == 0.0);
    return id;
}
static std::vector<std::pair<ParamID, double>> normalized_values(const std::vector<float>& plain) {
    std::vector<std::pair<ParamID, double>> result;
    for (uint32_t i = 0; i < plain.size(); ++i)
        result.push_back({static_cast<ParamID>(i), op_shell_to_normalized(i, plain[i])});
    return result;
}
// Render through the VST3 shell. An empty change list leaves the processor on
// the values it already holds, which is what a host does after setState.
static std::vector<float> render_host(IAudioProcessor* processor, int channels, int blocks,
                                      const std::vector<std::pair<ParamID, double>>& normalized,
                                      bool in_place) {
    constexpr int kFrames = 512;
    std::vector<float> inL(kFrames), inR(kFrames), outL(kFrames), outR(kFrames);
    ParameterChanges changes;
    for (auto [id, value] : normalized) change(changes, id, value);
    for (int block = 0; block < blocks; ++block) {
        fill_sine(inL.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (channels > 1) fill_sine(inR.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        float* inPtrs[] = {inL.data(), inR.data()};
        float* outPtrs[] = {in_place ? inL.data() : outL.data(), in_place ? inR.data() : outR.data()};
        AudioBusBuffers input{};
        input.numChannels = channels;
        input.channelBuffers32 = inPtrs;
        AudioBusBuffers output{};
        output.numChannels = channels;
        output.channelBuffers32 = outPtrs;
        ProcessData data{};
        data.processMode = kOffline;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kFrames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input;
        data.outputs = &output;
        data.inputParameterChanges = normalized.empty() ? nullptr : &changes;
        CHECK(processor->process(data) == kResultOk);
        const std::vector<float>& produced = in_place ? inL : outL;
        for (float sample : produced) CHECK(std::isfinite(sample));
        if (channels > 1) for (float sample : (in_place ? inR : outR)) CHECK(std::isfinite(sample));
        if (block + 1 == blocks) return produced;
    }
    return {};
}
#if OP_HAS_TAIL
// Mirror the contract the VST3 shell documents: zero seconds is kNoTail, a
// finite value is truncated to samples, and anything that is not below the
// kInfiniteTail sample bound is infinite.
static uint32 expected_tail(void* instance, double rate) {
    const double samples = op_shell_tail_seconds(instance) * rate;
    if (!(samples < 4.0e9)) return kInfiniteTail;
    return samples > 0.0 ? static_cast<uint32>(samples) : kNoTail;
}
#endif
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
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    PClassInfo effect{};
    PClassInfo other{};
    bool found = false;
    bool foundOther = false;
    char ids[2][33] = {};
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo classInfo{};
        CHECK(factory->getClassInfo(i, &classInfo) == kResultOk);
        FUID uid = FUID::fromTUID(classInfo.cid);
        uid.toString(ids[i]);
        CHECK(std::strlen(ids[i]) == 32);
        for (int c = 0; c < 32; ++c) {
            const char hex = ids[i][c];
            CHECK((hex >= '0' && hex <= '9') || (hex >= 'A' && hex <= 'F') || (hex >= 'a' && hex <= 'f'));
        }
        if (std::strcmp(classInfo.category, kVstAudioEffectClass) == 0) {
            effect = classInfo;
            found = true;
        } else {
            other = classInfo;
            foundOther = true;
        }
    }
    CHECK(found && foundOther);
    CHECK(std::strcmp(ids[0], ids[1]) != 0);
    CHECK(std::memcmp(effect.cid, info.processor_cid, 16) == 0);
    CHECK(std::memcmp(other.cid, info.controller_cid, 16) == 0);
    const uint32_t gain = find_shell_param("Gain");
    const uint32_t warm = find_shell_param("Warm");
    OpParamDesc gainDesc{};
    OpParamDesc warmDesc{};
    CHECK(op_shell_param(gain, &gainDesc) != 0);
    CHECK(op_shell_param(warm, &warmDesc) != 0);
    for (int channels : {1, 2}) {
        ReferenceShell reference(static_cast<uint32_t>(channels));
        CHECK(reference.instance);
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
        check_parameters(controller);
        CHECK(processor->canProcessSampleSize(kSample64) == kResultFalse);
        SpeakerArrangement arrangement = channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kOffline, kSample32, 4096, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        const char* label = channels == 1 ? "mono" : "stereo";
        CHECK(processor->getLatencySamples() == op_shell_latency(reference.instance));
        std::printf("VST3 %s latency: host %u samples, ABI %u\n", label, processor->getLatencySamples(), op_shell_latency(reference.instance));
#if OP_HAS_TAIL
        CHECK(processor->getTailSamples() == expected_tail(reference.instance, 48000.0));
        std::printf("VST3 %s tail: host %u samples, ABI %.6f s\n", label, processor->getTailSamples(), op_shell_tail_seconds(reference.instance));
#else
        std::printf("VST3 %s tail: not built (OP_TAIL=OFF)\n", label);
#endif
        const std::vector<float> defaults = shell_defaults();
        op_shell_reset(reference.instance);
        const auto hostDefault = render_host(processor, channels, 4, {}, false);
        const auto shellDefault = render_shell(reference.instance, channels, 4, defaults);
        std::printf("VST3 %s default output against the shell: max difference %.3g\n", label, max_difference(shellDefault, hostDefault));
        CHECK(max_difference(shellDefault, hostDefault) <= 1e-6);
        const double testGain = std::clamp(6.0, gainDesc.min, gainDesc.max);
        std::vector<float> plain = defaults;
        plain[gain] = static_cast<float>(testGain);
        plain[warm] = 0.0f;
        op_shell_reset(reference.instance);
        const auto shellGain = render_shell(reference.instance, channels, 8, plain);
        const auto hostGain = render_host(processor, channels, 8, normalized_values(plain), false);
        std::printf("VST3 %s gain %.2f dB against the shell: max difference %.3g\n", label, testGain, max_difference(shellGain, hostGain));
        CHECK(max_difference(shellGain, hostGain) <= 1e-6);
        const auto hostGainInPlace = render_host(processor, channels, 8, normalized_values(plain), true);
        std::printf("VST3 %s in-place against out-of-place: max difference %.3g\n", label, max_difference(hostGain, hostGainInPlace));
        CHECK(max_difference(hostGain, hostGainInPlace) <= 1e-6);
        std::vector<float> dry(512);
        fill_sine(dry.data(), 512, 48000.0, 1000.0, 0.25, 7 * 512);
        const double measured = gain_db(dry.data(), hostGain.data(), 128, 256);
        std::printf("VST3 %s measured gain: %.3f dB\n", label, measured);
        CHECK(std::fabs(measured - testGain) < 0.05);
        MemoryStream saved;
        CHECK(component->getState(&saved) == kResultOk);
        std::vector<float> changedPlain = plain;
        changedPlain[gain] = static_cast<float>(std::fabs(gainDesc.min - testGain) > 0.5 ? gainDesc.min : gainDesc.max);
        changedPlain[warm] = plain[warm] > 0.5f ? 0.0f : 1.0f;
        const auto hostChanged = render_host(processor, channels, 8, normalized_values(changedPlain), false);
        std::printf("VST3 %s changed state against saved state: max difference %.3g\n", label, max_difference(hostGain, hostChanged));
        CHECK(max_difference(hostGain, hostChanged) > 0.01);
        saved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&saved) == kResultOk);
        saved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(controller->setComponentState(&saved) == kResultOk);
        CHECK(std::fabs(controller->getParamNormalized(gain) - op_shell_to_normalized(gain, testGain)) < 1e-9);
        CHECK(std::fabs(controller->getParamNormalized(warm) - 0.0) < 1e-9);
        const auto hostRestored = render_host(processor, channels, 8, {}, false);
        std::printf("VST3 %s restored against saved state: max difference %.3g\n", label, max_difference(hostGain, hostRestored));
        CHECK(max_difference(hostGain, hostRestored) <= 1e-6);
        // Bypass. The id is found by kIsBypass; the output while bypassed is
        // the input delayed by the reported latency, and leaving bypass
        // restores the processed signal.
        const ParamID bypass = check_bypass_parameter(controller);
        const uint32_t latency = processor->getLatencySamples();
        CHECK(latency < 7 * 512);
        std::vector<float> delayed(512);
        fill_sine(delayed.data(), 512, 48000.0, 1000.0, 0.25, 7 * 512 - static_cast<double>(latency));
        const auto hostBypassed = render_host(processor, channels, 8, {{bypass, 1.0}}, false);
        std::printf("VST3 %s bypassed output against the input delayed by %u samples: max difference %.3g\n", label, latency, max_difference(delayed, hostBypassed));
        CHECK(max_difference(delayed, hostBypassed) == 0.0);
        const auto hostBypassedInPlace = render_host(processor, channels, 8, {}, true);
        CHECK(max_difference(delayed, hostBypassedInPlace) == 0.0);
        MemoryStream bypassSaved;
        CHECK(component->getState(&bypassSaved) == kResultOk);
        const auto hostUnbypassed = render_host(processor, channels, 8, {{bypass, 0.0}}, false);
        std::printf("VST3 %s processed again after bypass: max difference %.3g\n", label, max_difference(hostGain, hostUnbypassed));
        CHECK(max_difference(hostGain, hostUnbypassed) <= 1e-6);
        // The saved bypassed state brings bypass back.
        bypassSaved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&bypassSaved) == kResultOk);
        bypassSaved.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(controller->setComponentState(&bypassSaved) == kResultOk);
        CHECK(controller->getParamNormalized(bypass) == 1.0);
        const auto hostBypassRestored = render_host(processor, channels, 8, {}, false);
        CHECK(max_difference(delayed, hostBypassRestored) == 0.0);
        MemoryStream bypassAgain;
        CHECK(component->getState(&bypassAgain) == kResultOk);
        CHECK(bypassAgain.getSize() == bypassSaved.getSize());
        // A state written before bypass existed (no trailing flag) loads, and
        // means not bypassed.
        MemoryStream legacy;
        {
            int32_t header[2] = {1, static_cast<int32_t>(info.param_count)};
            CHECK(legacy.write(header, sizeof(header), nullptr) == kResultOk);
            for (uint32_t i = 0; i < info.param_count; ++i) {
                double value = op_shell_to_normalized(i, plain[i]);
                CHECK(legacy.write(&value, sizeof(value), nullptr) == kResultOk);
            }
        }
        legacy.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&legacy) == kResultOk);
        legacy.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(controller->setComponentState(&legacy) == kResultOk);
        CHECK(controller->getParamNormalized(bypass) == 0.0);
        const auto hostLegacy = render_host(processor, channels, 8, {}, false);
        std::printf("VST3 %s state without a bypass flag: max difference %.3g\n", label, max_difference(hostGain, hostLegacy));
        CHECK(max_difference(hostGain, hostLegacy) <= 1e-6);
        MemoryStream truncated;
        char bad[4] = {};
        CHECK(truncated.write(bad, 4, nullptr) == kResultOk);
        truncated.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&truncated) != kResultOk);
        MemoryStream wrongCount;
        int32_t words[2] = {1, static_cast<int32_t>(info.param_count) + 1};
        CHECK(wrongCount.write(words, sizeof(words), nullptr) == kResultOk);
        wrongCount.seek(0, IBStream::kIBSeekSet, nullptr);
        CHECK(component->setState(&wrongCount) != kResultOk);
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
#if OP_HAS_TAIL
    std::puts("VST3 proof: factory and ABI class IDs, parameter metadata, latency, tail, shell-equal audio, in-place buffers, state round-trip, malformed and wrong-count state, bypass (flag, delayed dry output, restore, state, legacy state): PASS");
#else
    std::puts("VST3 proof: factory and ABI class IDs, parameter metadata, latency, shell-equal audio, in-place buffers, state round-trip, malformed and wrong-count state, bypass, tail not built (OP_TAIL=OFF): PASS");
#endif
    return 0;
}
