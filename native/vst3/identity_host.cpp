// SPDX-License-Identifier: MIT
// The identity pinning proof (porting toolkit task 2) through the VST3 shell. The
// plugin is Pinned (examples/pinned) in four builds, each behind the real shell:
//
//   Pinned A   pinned ids, parameters level, offset, invert
//   Pinned B   pinned ids, parameters invert, drive (new), offset, level
//   Plain A    the first table, no pins
//   Plain B    the second table, no pins
//
// This host does what a VST3 host does with a project. It keeps the processor's
// class id, the component state the plugin wrote, and an automation lane whose
// points name a parameter by its ParamID. Saved with Pinned A and reopened with
// Pinned B, the project must find the same class, put every saved value on its
// own parameter in the processor and in the controller, and play the lane to
// the same audio, sample for sample; and the same from B back to A.
//
// The negative control is the same project through Plain A and Plain B, where a
// ParamID is the parameter's position, as it always was: the reordered build
// refuses the state and the lane moves other parameters. Not a DAW test.
//
//   VST3IdentityProof <Pinned A.vst3> <Pinned B.vst3> <Plain A.vst3> <Plain B.vst3>
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 identity proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

namespace {
constexpr int kFrames = 64;
// The pins of examples/pinned/src/lib.rs.
constexpr ParamID kLevel = 0x6C65766C;
constexpr ParamID kOffset = 0x6F666673;
constexpr ParamID kInvert = 0x696E7672;
constexpr ParamID kDrive = 0x64726976;
constexpr ParamID kBypass = 0x42595053;
const char* const kProcessorId = "A1B2C3D4E5F60718293A4B5C6D7E8F90";
const char* const kControllerId = "0F1E2D3C4B5A69788796A5B4C3D2E1F0";

using Bytes = std::vector<uint8_t>;
using Audio = std::vector<float>;

// One automation point: the parameter as the host stored it, the sample and the normalized value.
struct LanePoint {
    ParamID id;
    int32 offset;
    double value;
};

// What the host keeps of a plugin in its project file.
struct Project {
    std::string classId;
    Bytes state;
    std::vector<LanePoint> lane;
};

struct Param {
    ParamID id;
    std::string title;
    int32 flags;
};

// The class id as the 32 hex digits a host stores: the same text on every system.
std::string textOf(const TUID cid) {
    char text[33] = {};
    FUID::fromTUID(cid).toString(text);
    for (char& c : text)
        if (c >= 'a' && c <= 'f') c = static_cast<char>(c - 'a' + 'A');
    return text;
}

std::string asciiOf(const TChar* text) {
    std::string out;
    for (int i = 0; i < 128 && text[i]; ++i) out.push_back(static_cast<char>(text[i]));
    return out;
}

struct Plug {
    CFBundleRef bundle = nullptr;
    bool (*exitFn)() = nullptr;
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    std::string processorId;
    std::string controllerId;

    explicit Plug(const char* path) {
        CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(path), static_cast<CFIndex>(std::strlen(path)), true);
        CHECK(url);
        bundle = CFBundleCreate(nullptr, url);
        CFRelease(url);
        CHECK(bundle && CFBundleLoadExecutable(bundle));
        auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
        exitFn = reinterpret_cast<bool (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")));
        auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
        CHECK(entry && exitFn && getFactory && entry(bundle));
        factory = getFactory();
        CHECK(factory && factory->countClasses() == 2);
        TUID effectId{};
        for (int32 i = 0; i < factory->countClasses(); ++i) {
            PClassInfo info{};
            CHECK(factory->getClassInfo(i, &info) == kResultOk);
            if (std::strcmp(info.category, kVstAudioEffectClass) == 0) {
                std::memcpy(effectId, info.cid, 16);
                processorId = textOf(info.cid);
            } else {
                controllerId = textOf(info.cid);
            }
        }
        CHECK(processorId.size() == 32 && controllerId.size() == 32);
        CHECK(factory->createInstance(effectId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        TUID linked{};
        CHECK(component->getControllerClassId(linked) == kResultOk);
        CHECK(textOf(linked) == controllerId);
        CHECK(factory->createInstance(linked, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        SpeakerArrangement stereo = SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&stereo, 1, &stereo, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, kFrames, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
    }
    ~Plug() {
        processor->setProcessing(false);
        component->setActive(false);
        controller->terminate();
        controller->release();
        processor->release();
        component->terminate();
        component->release();
        factory->release();
        exitFn();
        CFRelease(bundle);
    }
    Plug(const Plug&) = delete;
    Plug& operator=(const Plug&) = delete;

    // The plugin's own parameters, in the order the controller lists them (the
    // bypass parameter the shell adds is checked apart).
    std::vector<Param> params() const {
        std::vector<Param> list;
        bool bypass = false;
        for (int32 i = 0; i < controller->getParameterCount(); ++i) {
            ParameterInfo info{};
            CHECK(controller->getParameterInfo(i, info) == kResultOk);
            if (info.flags & ParameterInfo::kIsBypass) {
                CHECK(info.id == kBypass && !bypass);
                bypass = true;
                continue;
            }
            list.push_back({info.id, asciiOf(info.title), info.flags});
        }
        CHECK(bypass);
        return list;
    }
    ParamID idOf(const char* title) const {
        for (const Param& p : params())
            if (p.title == title) return p.id;
        std::fprintf(stderr, "no parameter titled %s\n", title);
        std::exit(1);
    }
    bool has(ParamID id) const {
        const auto list = params();
        return std::any_of(list.begin(), list.end(), [id](const Param& p) { return p.id == id; });
    }
    // One block of a constant input with the points of a lane; the left channel.
    Audio render(float input, const std::vector<LanePoint>& points) {
        Audio inL(kFrames, input), inR(kFrames, input), outL(kFrames, 9.0f), outR(kFrames, 9.0f);
        ParameterChanges changes;
        for (const LanePoint& point : points) {
            int32 index = 0;
            IParamValueQueue* queue = changes.addParameterData(point.id, index);
            CHECK(queue && queue->addPoint(point.offset, point.value, index) == kResultOk);
        }
        float* ins[] = {inL.data(), inR.data()};
        float* outs[] = {outL.data(), outR.data()};
        AudioBusBuffers input32{}, output32{};
        input32.numChannels = output32.numChannels = 2;
        input32.channelBuffers32 = ins;
        output32.channelBuffers32 = outs;
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kFrames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input32;
        data.outputs = &output32;
        data.inputParameterChanges = points.empty() ? nullptr : &changes;
        CHECK(processor->process(data) == kResultOk);
        CHECK(outL == outR);
        return outL;
    }
    // Sets values as a host does while the user moves controls: points at the block start.
    void set(const std::vector<std::pair<ParamID, double>>& values) {
        std::vector<LanePoint> points;
        for (const auto& [id, value] : values) {
            points.push_back({id, 0, value});
            CHECK(controller->setParamNormalized(id, value) == kResultOk);
        }
        render(0.0f, points);
    }
    Bytes save() {
        MemoryStream stream;
        CHECK(component->getState(&stream) == kResultOk);
        const char* data = stream.getData();
        return Bytes(data, data + stream.getSize());
    }
    // The component state goes to the processor and then, when the processor took it, to
    // the controller, as a host does on load. The controller must take what the processor took.
    bool load(const Bytes& state) {
        for (int half = 0; half < 2; ++half) {
            MemoryStream stream;
            if (!state.empty()) CHECK(stream.write(const_cast<uint8_t*>(state.data()), static_cast<int32>(state.size()), nullptr) == kResultOk);
            stream.seek(0, IBStream::kIBSeekSet, nullptr);
            if (half == 0 && component->setState(&stream) != kResultOk) return false;
            if (half == 1) CHECK(controller->setComponentState(&stream) == kResultOk);
        }
        return true;
    }
    double shown(ParamID id) const { return controller->getParamNormalized(id); }
    std::string text(ParamID id, double normalized) const {
        String128 text{};
        if (controller->getParamStringByValue(id, normalized, text) != kResultOk) return "<refused>";
        return asciiOf(text);
    }
    Project project(std::vector<LanePoint> lane) { return {processorId, save(), std::move(lane)}; }
};

int32_t headVersion(const Bytes& state) {
    CHECK(state.size() >= 8);
    int32_t version = 0;
    std::memcpy(&version, state.data(), 4);
    return version;
}

// The lane of the proof: level and offset move in the middle of the block.
// Normalized 0.75 of level (0..2) is 1.5, 0.125 is 0.25; 0.25 of offset (-1..1) is -0.5.
std::vector<LanePoint> lane(ParamID level, ParamID offset) { return {{level, 16, 0.75}, {offset, 32, 0.25}, {level, 48, 0.125}}; }

// The audio the lane must give after the saved values (level 0.5, offset 0.25,
// inverted) with an input of 0.5, from the plugin's own formula. Every value is
// exact in a float.
Audio expected() {
    Audio out(kFrames);
    for (int n = 0; n < kFrames; ++n) {
        const float level = n < 16 ? 0.5f : n < 48 ? 1.5f : 0.25f;
        const float offset = n < 32 ? 0.25f : -0.5f;
        out[n] = -(0.5f * level + offset);
    }
    return out;
}

std::vector<ParamID> idsOf(const Plug& plug) {
    std::vector<ParamID> ids;
    for (const Param& p : plug.params()) ids.push_back(p.id);
    return ids;
}
}

int main(int argc, char** argv) {
    CHECK(argc == 5);
    // ---- what each build shows the host ------------------------------------------------
    {
        Plug a(argv[1]), b(argv[2]), plainA(argv[3]), plainB(argv[4]);
        CHECK(a.processorId == kProcessorId && a.controllerId == kControllerId);
        CHECK(b.processorId == kProcessorId && b.controllerId == kControllerId);
        CHECK(idsOf(a) == (std::vector<ParamID>{kLevel, kOffset, kInvert}));
        CHECK(idsOf(b) == (std::vector<ParamID>{kInvert, kDrive, kOffset, kLevel}));
        CHECK(a.idOf("Level") == kLevel && b.idOf("Level") == kLevel && b.idOf("Drive") == kDrive);
        // Text and values go by the host's id to the right parameter in either order.
        CHECK(a.text(kInvert, 1.0) == "On" && b.text(kInvert, 1.0) == "On");
        CHECK(a.shown(kLevel) == 0.5 && b.shown(kLevel) == 0.5 && a.shown(kOffset) == 0.5 && b.shown(kOffset) == 0.5);
        for (ParamID stale : {ParamID(0), ParamID(1), ParamID(2), ParamID(3), kDrive + 1}) {
            CHECK(a.text(stale, 0.0) == "<refused>" && b.text(stale, 0.0) == "<refused>");
            CHECK(a.controller->setParamNormalized(stale, 1.0) != kResultOk);
        }
        CHECK(!a.has(kDrive) && b.has(kDrive));
        // Without pins a ParamID is the position, and the class ids are the derived ones.
        CHECK(idsOf(plainA) == (std::vector<ParamID>{0, 1, 2}));
        CHECK(idsOf(plainB) == (std::vector<ParamID>{0, 1, 2, 3}));
        CHECK(plainA.processorId == plainB.processorId && plainA.processorId != kProcessorId && plainA.controllerId != kControllerId);
        // The head of the state: by ParamID (version 2) when pinned, by position (version 1) as always otherwise.
        CHECK(headVersion(a.save()) == 2 && headVersion(b.save()) == 2);
        CHECK(headVersion(plainA.save()) == 1 && headVersion(plainB.save()) == 1);
        std::printf("VST3 identity: Pinned A and B both show class %s / %s and ParamIDs %08X %08X %08X (B also %08X) in their own orders; Plain A and B show positions 0..n-1 and class %s\n",
                    a.processorId.c_str(), a.controllerId.c_str(), kLevel, kOffset, kInvert, kDrive, plainA.processorId.c_str());
    }
    // ---- a project saved with Pinned A reopens in Pinned B ------------------------------
    {
        Plug first(argv[1]);
        first.set({{kLevel, 0.25}, {kOffset, 0.625}, {kInvert, 1.0}});
        const Project project = first.project(lane(first.idOf("Level"), first.idOf("Offset")));
        const Audio heard = first.render(0.5f, project.lane);
        CHECK(heard == expected());

        Plug second(argv[2]);
        CHECK(second.processorId == project.classId);
        for (const LanePoint& point : project.lane) CHECK(second.has(point.id));
        CHECK(second.load(project.state));
        CHECK(second.shown(kLevel) == 0.25 && second.shown(kOffset) == 0.625 && second.shown(kInvert) == 1.0 && second.shown(kDrive) == 0.0);
        CHECK(second.render(0.5f, project.lane) == heard);
        // The values are in the processor, not only in the controller: no lane, same start.
        Plug again(argv[2]);
        CHECK(again.load(project.state));
        const Audio still = again.render(0.5f, {});
        for (float sample : still) CHECK(sample == -(0.5f * 0.5f + 0.25f));
        std::printf("VST3 identity: a project of Pinned A (level 0.5, offset 0.25, inverted, a 3-point lane) reopens in Pinned B: same class, every value on its own parameter in processor and controller, drive at its default, the lane's audio identical over %d samples\n", kFrames);

        // And back: a project of B, drive included, in A.
        second.set({{kLevel, 1.0}, {kOffset, 0.375}, {kInvert, 0.0}, {kDrive, 0.5}});
        const Project newer = second.project(lane(second.idOf("Level"), second.idOf("Offset")));
        const Audio driven = second.render(0.5f, newer.lane);
        Plug back(argv[1]);
        CHECK(back.processorId == newer.classId);
        CHECK(back.load(newer.state));
        CHECK(back.shown(kLevel) == 1.0 && back.shown(kOffset) == 0.375 && back.shown(kInvert) == 0.0);
        const Audio plain = back.render(0.5f, newer.lane);
        // A has no drive (1.5 in B): the same lane without it.
        for (int n = 0; n < kFrames; ++n) CHECK(plain[n] * 1.5f == driven[n]);
        std::printf("VST3 identity: a project of Pinned B reopens in Pinned A: the three shared parameters and the lane land as saved; the value of drive, which A does not have, is dropped\n");
    }
    // ---- points and states the pinned build must not take --------------------------------
    {
        Plug a(argv[1]);
        a.set({{kLevel, 0.25}});
        const Audio quiet = a.render(0.5f, {});
        // The drive of the other build, and positions: none is a parameter here.
        for (ParamID stale : {kDrive, ParamID(0), ParamID(1), ParamID(2)}) CHECK(a.render(0.5f, {{stale, 0, 1.0}}) == quiet);
        // A positional head (version 1: three values) names ParamIDs 0, 1 and 2, which this build does not have.
        Plug plain(argv[3]);
        plain.set({{0, 0.75}, {1, 0.125}, {2, 1.0}});
        const Bytes positional = plain.save();
        CHECK(headVersion(positional) == 1);
        CHECK(!a.load(positional));
        // Cut and corrupt keyed states are refused whole.
        const Bytes good = a.save();
        for (size_t cut : {size_t(0), size_t(4), size_t(8), size_t(12), size_t(19), good.size() - 1}) {
            const Bytes shorter(good.begin(), good.begin() + static_cast<long>(cut));
            CHECK(!a.load(shorter));
        }
        Bytes unknownVersion = good;
        unknownVersion[0] = 3;
        CHECK(!a.load(unknownVersion));
        CHECK(a.shown(kLevel) == 0.25 && a.shown(kOffset) == 0.5 && a.shown(kInvert) == 0.0);
        CHECK(a.render(0.5f, {}) == quiet);
        CHECK(a.load(good));
        std::printf("VST3 identity: in Pinned A, points for ParamIDs it does not have move nothing; a positional state, cut states and an unknown head version are refused and change nothing\n");
    }
    // ---- the negative control: the same project without pins ----------------------------
    {
        Plug first(argv[3]);
        const ParamID level = first.idOf("Level"), offset = first.idOf("Offset"), invert = first.idOf("Invert");
        first.set({{level, 0.25}, {offset, 0.625}, {invert, 1.0}});
        const Project project = first.project(lane(level, offset));
        const Audio heard = first.render(0.5f, project.lane);
        CHECK(heard == expected());

        Plug second(argv[4]);
        CHECK(second.processorId == project.classId);
        // Three positional values load as the first three of four, the rule for a table that
        // grew at its end (docs/SHELL.md, VST3 state). This table was reordered too, so each
        // value lands on the parameter now at its position: offset's on Drive, invert's on Offset.
        CHECK(second.load(project.state));
        CHECK(second.shown(second.idOf("Drive")) == 0.625 && second.shown(second.idOf("Offset")) == 1.0);
        // The lane's ids are positions: 0 was level and is invert now, 1 was offset and is drive.
        CHECK(second.idOf("Invert") == level && second.idOf("Drive") == offset);
        CHECK(second.render(0.5f, project.lane) != heard);
        // With the values put right by hand the lane still lands on the wrong parameters...
        const std::vector<std::pair<ParamID, double>> byName = {{second.idOf("Level"), 0.25}, {second.idOf("Offset"), 0.625}, {second.idOf("Invert"), 1.0}, {second.idOf("Drive"), 0.0}};
        second.set(byName);
        CHECK(second.render(0.5f, project.lane) != heard);
        // ...and addressed by name it is right: only the ids were wrong.
        second.set(byName);
        CHECK(second.render(0.5f, lane(second.idOf("Level"), second.idOf("Offset"))) == heard);
        std::printf("VST3 identity, negative control: without pins the reordered build takes the saved values by position, on other parameters, and plays the lane on other parameters (the audio differs); addressed by name the same lane is right\n");
    }
    std::printf("VST3 identity proof passed\n");
    return 0;
}
