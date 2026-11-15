// SPDX-License-Identifier: MIT
// The channel order proof (0.6 task 8) through the VST3 shell. The plugin is Role
// Tags (examples/role_tags), which declares every SDK layout from mono to 9.1.6 and
// checks every input channel against the role it believes the channel has (see
// native/include/role_tags.h). This host speaks VST3 only: for each layout it offers
// the SDK's named speaker arrangement with setBusArrangements, and writes on host
// channel j the tag of the speaker VST3 puts there (SpeakerArr::getSpeaker, ascending
// speaker bits), read through its own table from VST3 speakers to SDK roles. Every
// output channel must come back with the plugin's tag of the same speaker. Not a DAW
// test.
//
//   VST3RolesProof <Role Tags.vst3>
#include "role_tags.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
using namespace roletags;

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 roles proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
constexpr int kBlock = 512;
constexpr int kFrames = 64;
using Audio = std::vector<std::vector<float>>;

// What each VST3 speaker is in the SDK's role ids (vstspeaker.h). VST3's Ls and Rs are
// the surround pair of 5.1 and the back pair of 7.1 Music (whose sides are Sl and Sr),
// which is the SDK's BL and BR; Trl Trc Trr are the top back (rear) row.
static uint8_t roleOfSpeaker(Speaker s) {
    struct Entry { Speaker speaker; uint8_t code; };
    static const Entry table[] = {
        {kSpeakerL, FL},     {kSpeakerR, FR},     {kSpeakerC, FC},     {kSpeakerLfe, LFE},  {kSpeakerLs, BL},   {kSpeakerRs, BR},
        {kSpeakerLc, FLC},   {kSpeakerRc, FRC},   {kSpeakerCs, BC},    {kSpeakerSl, SL},    {kSpeakerSr, SR},   {kSpeakerTc, TC},
        {kSpeakerTfl, TFL},  {kSpeakerTfc, TFC},  {kSpeakerTfr, TFR},  {kSpeakerTrl, TBL},  {kSpeakerTrc, TBC}, {kSpeakerTrr, TBR},
        {kSpeakerTsl, TSL},  {kSpeakerTsr, TSR},  {kSpeakerLw, FLW},   {kSpeakerRw, FRW},   {kSpeakerM, kMonoCode},
    };
    for (const Entry& e : table)
        if (e.speaker == s) return e.code;
    return kUnknown;
}

static std::vector<uint8_t> speakersOf(SpeakerArrangement arr) {
    std::vector<uint8_t> codes;
    for (int32 j = 0; j < SpeakerArr::getChannelCount(arr); ++j) {
        const uint8_t code = roleOfSpeaker(SpeakerArr::getSpeaker(arr, j));
        CHECK(code != kUnknown);
        codes.push_back(code);
    }
    return codes;
}

struct Plug {
    CFBundleRef bundle = nullptr;
    bool (*exitFn)() = nullptr;
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    bool active = false;

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
        CHECK(factory);
        bool found = false;
        for (int32 i = 0; i < factory->countClasses() && !found; ++i) {
            PClassInfo info{};
            CHECK(factory->getClassInfo(i, &info) == kResultOk);
            if (std::strcmp(info.category, kVstAudioEffectClass) != 0) continue;
            CHECK(factory->createInstance(info.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
            found = true;
        }
        CHECK(found);
        CHECK(component->initialize(nullptr) == kResultOk);
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
    }
    ~Plug() {
        if (active) stop();
        processor->release();
        component->terminate();
        component->release();
        factory->release();
        exitFn();
        CFRelease(bundle);
    }
    tresult arrange(SpeakerArrangement in, SpeakerArrangement out) { return processor->setBusArrangements(&in, 1, &out, 1); }
    SpeakerArrangement arrangementOf(BusDirection dir) {
        SpeakerArrangement arr = ~0ull;
        CHECK(processor->getBusArrangement(dir, 0, arr) == kResultTrue);
        return arr;
    }
    void start() {
        if (active) stop();
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, kBlock, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        active = true;
    }
    void stop() {
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        active = false;
    }
    // One block, host-order channels in, host-order channels out.
    Audio process(Audio in) {
        Audio out(in.size(), std::vector<float>(kFrames, 7.0f));
        std::vector<float*> inPtrs, outPtrs;
        for (auto& c : in) inPtrs.push_back(c.data());
        for (auto& c : out) outPtrs.push_back(c.data());
        AudioBusBuffers input{}, output{};
        input.numChannels = static_cast<int32>(in.size());
        input.channelBuffers32 = inPtrs.data();
        output.numChannels = static_cast<int32>(out.size());
        output.channelBuffers32 = outPtrs.data();
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kFrames;
        data.numInputs = 1;
        data.inputs = &input;
        data.numOutputs = 1;
        data.outputs = &output;
        CHECK(processor->process(data) == kResultOk);
        return out;
    }
};

static Audio tags(const std::vector<uint8_t>& speakers) {
    Audio a;
    for (uint8_t c : speakers) a.push_back(std::vector<float>(kFrames, inTag(c)));
    return a;
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    Plug p(argv[1]);
    using namespace SpeakerArr;
    struct Case { const char* name; SpeakerArrangement arrangement; };
    // Every layout Role Tags declares, as the VST3 SDK names it. All of them can be
    // expressed in VST3, so none is skipped.
    const Case cases[] = {
        {"mono", kMono},     {"stereo", kStereo}, {"LCR", k30Cine},    {"quad", k40Music}, {"5.0", k50},
        {"5.1", k51},        {"6.1", k61Cine},    {"7.1", k71Music},   {"7.1.4", k71_4},   {"5.1.2", k51_2_TS},
        {"5.1.4", k51_4},    {"7.1.2", k71_2},    {"9.1.6", k91_6_W},
    };
    int ran = 0;
    for (const Case& c : cases) {
        CHECK(p.arrange(c.arrangement, c.arrangement) == kResultTrue);
        CHECK(p.arrangementOf(kInput) == c.arrangement && p.arrangementOf(kOutput) == c.arrangement);
        const std::vector<uint8_t> speakers = speakersOf(c.arrangement);
        p.start();
        const Audio out = p.process(tags(speakers));
        for (size_t j = 0; j < out.size(); ++j) CHECK(checkChannel("VST3", c.name, static_cast<uint32_t>(j), speakers[j], out[j].data(), kFrames));
        p.stop();
        std::printf("VST3 roles: %-6s %2zu channels, host order %s: every role at its index in and out\n", c.name, speakers.size(), codes(speakers).c_str());
        ++ran;
    }
    CHECK(ran == 13);

    // The negative control: a host whose tags ignore VST3's channel order is caught.
    {
        CHECK(p.arrange(k91_6_W, k91_6_W) == kResultTrue);
        std::vector<uint8_t> speakers = speakersOf(k91_6_W);
        std::rotate(speakers.begin(), speakers.begin() + 1, speakers.end());
        p.start();
        CHECK(allFlagged(p.process(tags(speakers))));
        p.stop();
        std::puts("VST3 roles: 9.1.6 with the tags one channel off: every channel flagged (negative control)");
    }
    // Arrangements with the same channel counts but other speakers are not the plugin's layouts.
    CHECK(p.arrange(k51_2, k51_2) == kResultFalse);    // front heights Tfl Tfr, not the top side pair
    CHECK(p.arrange(k91_6, k91_6) == kResultFalse);    // Lc Rc, not the wides
    CHECK(p.arrange(k71Cine, k71Cine) == kResultFalse);
    CHECK(p.arrange(k71_2_TF, k71_2_TF) == kResultFalse);
    std::puts("VST3 roles: k51_2 (front heights), k91_6 (Lc Rc), k71Cine and k71_2_TF are refused");
    std::puts("VST3 roles proof: mono, stereo, LCR, quad, 5.0, 5.1, 6.1, 7.1, 7.1.4, 5.1.2, 5.1.4, 7.1.2, 9.1.6; skipped: none: PASS");
    return 0;
}
