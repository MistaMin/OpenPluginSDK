// SPDX-License-Identifier: MIT
// Plugin state in the VST3 shell. The shell is built over state_stub.cpp, a
// plugin with a chunk of its own (a gain that multiplies the audio), so a
// restored chunk can be heard and read back through op_shell_save_state.
//
// Proved through the component and controller interfaces a host uses:
//  - getState writes the head (version 1, parameter values, bypass flag) and
//    the plugin-state trailer; setState on a new instance restores the
//    parameter and the chunk, with the engine active or not, and the audio of
//    the two instances is identical;
//  - the chunk survives deactivation (getState after setActive(false));
//  - a state grown between the size query and the copy still saves (the loop);
//  - version 1 states, with and without the bypass flag, load as before and
//    leave the chunk alone;
//  - corrupt trailers (bad plugin blob, foreign tag or version, truncated,
//    extended, empty, oversized, half a header) are refused and change nothing,
//    active or not;
//  - the controller's values match after setComponentState of a new state.
//
//   ctest --test-dir build-shell -R vst3_state_proof --output-on-failure
#include "proof_shell.h"
#include "state_stub.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/common/memorystream.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 state proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

namespace {
constexpr uint32_t kTag = 0x4F505342;  // 'OPSB'

using Bytes = std::vector<uint8_t>;

void put32(Bytes& b, uint32_t v) {
    const uint8_t* p = reinterpret_cast<const uint8_t*>(&v);
    b.insert(b.end(), p, p + 4);
}

// The head every shell has written: version 1, one parameter, optionally the
// bypass flag.
Bytes head(double level, bool withBypass, int32_t bypass = 0) {
    Bytes b;
    put32(b, 1);
    put32(b, 1);
    const uint8_t* p = reinterpret_cast<const uint8_t*>(&level);
    b.insert(b.end(), p, p + 8);
    if (withBypass) put32(b, static_cast<uint32_t>(bypass));
    return b;
}

void trailer(Bytes& b, const Bytes& blob, uint32_t tag = kTag, uint32_t version = 1, int64_t length = -1) {
    put32(b, tag);
    put32(b, version);
    put32(b, static_cast<uint32_t>(length < 0 ? static_cast<int64_t>(blob.size()) : length));
    b.insert(b.end(), blob.begin(), blob.end());
}

Bytes newState(double level, const state_stub::Blob& blob, int32_t bypass = 0) {
    Bytes b = head(level, true, bypass);
    trailer(b, state_stub::encode(blob));
    return b;
}

struct Stream {
    MemoryStream s;
    explicit Stream(const Bytes& b) {
        if (!b.empty()) CHECK(s.write(const_cast<uint8_t*>(b.data()), static_cast<int32>(b.size()), nullptr) == kResultOk);
        s.seek(0, IBStream::kIBSeekSet, nullptr);
    }
};

Bytes bytesOf(MemoryStream& s) {
    const char* data = s.getData();
    return Bytes(data, data + s.getSize());
}

struct Parsed {
    double level = -1.0;
    int32_t bypass = -1;
    bool hasTrailer = false;
    Bytes blob;
    bool ok = false;
};
Parsed parse(const Bytes& b) {
    Parsed p;
    if (b.size() < 8 + 8 + 4) return p;
    uint32_t version, count;
    std::memcpy(&version, b.data(), 4);
    std::memcpy(&count, b.data() + 4, 4);
    if (version != 1 || count != 1) return p;
    std::memcpy(&p.level, b.data() + 8, 8);
    std::memcpy(&p.bypass, b.data() + 16, 4);
    if (b.size() == 20) {
        p.ok = true;
        return p;
    }
    if (b.size() < 32) return p;
    uint32_t tag, tv, length;
    std::memcpy(&tag, b.data() + 20, 4);
    std::memcpy(&tv, b.data() + 24, 4);
    std::memcpy(&length, b.data() + 28, 4);
    if (tag != kTag || tv != 1 || b.size() != 32u + length) return p;
    p.hasTrailer = true;
    p.blob.assign(b.begin() + 32, b.end());
    p.ok = true;
    return p;
}

struct Instance {
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    bool active = false;

    Instance(IPluginFactory* f, const TUID effectId) : factory(f) {
        CHECK(factory->createInstance(effectId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        SpeakerArrangement arrangement = SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, 1024, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
    }
    void activate() {
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
        active = true;
    }
    void deactivate() {
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        active = false;
    }
    ~Instance() {
        if (active) deactivate();
        processor->release();
        controller->terminate();
        controller->release();
        component->terminate();
        component->release();
    }
    Bytes save() {
        MemoryStream s;
        CHECK(component->getState(&s) == kResultOk);
        return bytesOf(s);
    }
    tresult load(const Bytes& b) {
        Stream s(b);
        return component->setState(&s.s);
    }
    tresult loadController(const Bytes& b) {
        Stream s(b);
        return controller->setComponentState(&s.s);
    }
    // One block of a sine; returns the left channel.
    std::vector<float> render(int start = 0) {
        constexpr int frames = 256;
        std::vector<float> inL(frames), inR(frames), outL(frames), outR(frames);
        fill_sine(inL.data(), frames, 48000.0, 1000.0, 0.25, start);
        fill_sine(inR.data(), frames, 48000.0, 1000.0, 0.25, start);
        float* ins[] = {inL.data(), inR.data()};
        float* outs[] = {outL.data(), outR.data()};
        AudioBusBuffers input{};
        input.numChannels = 2;
        input.channelBuffers32 = ins;
        AudioBusBuffers output{};
        output.numChannels = 2;
        output.channelBuffers32 = outs;
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = frames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input;
        data.outputs = &output;
        CHECK(processor->process(data) == kResultOk);
        return outL;
    }
};

// The audio the stub must produce for a level and chunk gain, exactly.
std::vector<float> expected(float level, float gain) {
    std::vector<float> in(256), out(256);
    fill_sine(in.data(), 256, 48000.0, 1000.0, 0.25, 0);
    const float scale = level * gain;
    for (int i = 0; i < 256; ++i) out[i] = in[i] * scale;
    return out;
}

state_stub::Blob blobOf(const Bytes& saved) {
    const Parsed p = parse(saved);
    CHECK(p.ok && p.hasTrailer);
    state_stub::Blob blob;
    CHECK(state_stub::decode(p.blob.data(), p.blob.size(), blob));
    return blob;
}
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
    {
        // 1. A new state carries the parameter and the chunk, active or not.
        state_stub::Blob source;
        source.level = 0.75f;
        source.gain = 0.25f;
        const Bytes written = newState(0.75, source, 0);
        for (bool activeFirst : {false, true}) {
            Instance a(factory, effectId);
            if (activeFirst) a.activate();
            CHECK(a.load(written) == kResultOk);
            CHECK(a.loadController(written) == kResultOk);
            CHECK(std::fabs(a.controller->getParamNormalized(0) - 0.75) < 1e-12);
            if (!activeFirst) a.activate();
            const auto out = a.render();
            CHECK(out == expected(0.75f, 0.25f));
            std::printf("VST3 state, %s: setState of a new state restores level 0.75 and chunk gain 0.25: audio identical to the model, controller value %.2f\n",
                        activeFirst ? "engine active" : "engine inactive", a.controller->getParamNormalized(0));

            // 2. Save from it and restore into a fresh instance.
            const Bytes saved = a.save();
            const Parsed parsed = parse(saved);
            CHECK(parsed.ok && parsed.hasTrailer && parsed.level == 0.75 && parsed.bypass == 0);
            const state_stub::Blob inside = blobOf(saved);
            CHECK(inside.level == 0.75f && inside.gain == 0.25f && inside.pad == 0 && inside.grow == 0);
            for (bool restoreActive : {false, true}) {
                Instance b(factory, effectId);
                if (restoreActive) b.activate();
                CHECK(b.load(saved) == kResultOk);
                CHECK(b.loadController(saved) == kResultOk);
                CHECK(std::fabs(b.controller->getParamNormalized(0) - 0.75) < 1e-12);
                if (!restoreActive) {
                    // Not yet active: the state waits for the engine and is saved back unchanged.
                    CHECK(b.save() == saved);
                    b.activate();
                }
                CHECK(b.render() == out);
                CHECK(b.save() == saved);
                // The chunk survives deactivation and reactivation.
                b.deactivate();
                CHECK(b.save() == saved);
                b.activate();
                CHECK(b.render() == out);
                std::printf("VST3 state round trip (saved %s, restored into %s instance): parameter, chunk and audio identical; kept over deactivation\n",
                            activeFirst ? "active" : "inactive", restoreActive ? "an active" : "an inactive");
            }
        }
    }
    {
        // 3. Size query protocol: the state grows twice between query and copy.
        Instance a(factory, effectId);
        a.activate();
        state_stub::Blob growing;
        growing.level = 0.5f;
        growing.gain = 2.0f;
        growing.pad = 8;
        growing.grow = 2;
        CHECK(a.load(newState(0.5, growing)) == kResultOk);
        const Bytes saved = a.save();
        const state_stub::Blob inside = blobOf(saved);
        CHECK(inside.pad == 8 + 32 && inside.grow == 0 && inside.gain == 2.0f);
        const Parsed parsed = parse(saved);
        CHECK(parsed.blob.size() == state_stub::kHeader + inside.pad);
        Instance b(factory, effectId);
        CHECK(b.load(saved) == kResultOk);
        b.activate();
        CHECK(b.render() == expected(0.5f, 2.0f));
        std::printf("VST3 state grown twice between size query and copy: saved at %zu bytes, restored, audio identical\n", parsed.blob.size());
    }
    {
        // 4. States written by the shell before the trailer: version 1, with and
        // without the bypass flag. They load as before and leave the chunk alone.
        state_stub::Blob chunk;
        chunk.level = 0.5f;
        chunk.gain = 0.5f;
        const Bytes full = newState(0.5, chunk);
        for (int variant = 0; variant < 2; ++variant) {
            const Bytes old = head(0.25, variant == 1, 0);
            CHECK(old.size() == (variant == 1 ? 20u : 16u));
            Instance fresh(factory, effectId);
            CHECK(fresh.load(old) == kResultOk);
            CHECK(fresh.loadController(old) == kResultOk);
            CHECK(std::fabs(fresh.controller->getParamNormalized(0) - 0.25) < 1e-12);
            fresh.activate();
            CHECK(fresh.render() == expected(0.25f, 1.0f));
            Instance held(factory, effectId);
            held.activate();
            CHECK(held.load(full) == kResultOk);
            CHECK(held.render() == expected(0.5f, 0.5f));
            CHECK(held.load(old) == kResultOk);
            CHECK(held.render() == expected(0.25f, 0.5f));
            std::printf("VST3 state version 1 %s the bypass flag: loads, parameter restored, chunk left alone\n", variant == 1 ? "with" : "without");
        }
        // Bypass flag set in an old state still means bypassed.
        Instance bypassed(factory, effectId);
        CHECK(bypassed.load(head(0.25, true, 1)) == kResultOk);
        CHECK(parse(bypassed.save()).bypass == 1);
    }
    {
        // 5. Corrupt states are refused and change nothing. The held state has
        // level 0.75, gain 0.25 and bypass 0; every bad state says level 0.1,
        // so a partial load would show in the saved bytes or the audio.
        state_stub::Blob held;
        held.level = 0.75f;
        held.gain = 0.25f;
        const Bytes good = newState(0.75, held);
        state_stub::Blob other;
        other.level = 0.1f;
        other.gain = 3.0f;
        const Bytes otherBlob = state_stub::encode(other);
        const Bytes headBad = head(0.1, true, 1);
        struct Case {
            const char* name;
            Bytes bytes;
        };
        std::vector<Case> cases;
        auto add = [&](const char* name, Bytes tail) {
            Bytes b = headBad;
            b.insert(b.end(), tail.begin(), tail.end());
            cases.push_back({name, std::move(b)});
        };
        {
            Bytes t;
            trailer(t, otherBlob, 0x12345678);
            add("foreign trailer tag", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 2);
            add("unknown trailer version", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 0);
            add("trailer version 0", t);
        }
        {
            Bytes t;
            trailer(t, {}, kTag, 1, 0);
            add("empty plugin blob", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 1, static_cast<int64_t>(otherBlob.size()) + 1);
            add("length longer than the data (truncated)", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 1, static_cast<int64_t>(otherBlob.size()) - 1);
            add("length shorter than the data (bytes after the trailer)", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob);
            t.push_back(0);
            add("one byte after the trailer", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 1, 0x7FFFFFFF);
            add("length 2 GiB with a few bytes", t);
        }
        {
            Bytes t;
            trailer(t, otherBlob, kTag, 1, 0xFFFFFFFFll);
            add("length 4 GiB - 1", t);
        }
        {
            Bytes t;
            put32(t, kTag);
            put32(t, 1);
            add("trailer header without a length", t);
        }
        {
            Bytes t;
            t.push_back('O');
            t.push_back('P');
            add("two bytes of a tag", t);
        }
        {
            Bytes b = otherBlob;
            b[0] ^= 0xFF;
            Bytes t;
            trailer(t, b);
            add("plugin blob with a wrong magic", t);
        }
        {
            Bytes b = otherBlob;
            b.resize(b.size() - 3);
            Bytes t;
            trailer(t, b);
            add("plugin blob cut short", t);
        }
        {
            state_stub::Blob nan = other;
            nan.gain = std::numeric_limits<float>::quiet_NaN();
            Bytes t;
            trailer(t, state_stub::encode(nan));
            add("plugin blob with a NaN chunk value", t);
        }
        {
            state_stub::Blob big = other;
            big.level = 7.0f;
            Bytes t;
            trailer(t, state_stub::encode(big));
            add("plugin blob with an out-of-range parameter", t);
        }
        {
            Bytes b = otherBlob;
            b.insert(b.end(), 5, 0);
            Bytes t;
            trailer(t, b);
            add("plugin blob with trailing bytes", t);
        }
        {
            Bytes b = headBad;
            b.insert(b.end(), 40, 0xFF);
            cases.push_back({"garbage after the bypass flag", b});
        }
        {
            Bytes b = good;
            b.resize(good.size() - 1);
            cases.push_back({"the good state cut by one byte", b});
        }
        // The head fixed by the existing proofs also holds here.
        cases.push_back({"nothing", {}});
        cases.push_back({"a wrong parameter count", [] { Bytes b = head(0.1, true); b[4] = 2; return b; }()});
        cases.push_back({"a NaN parameter value", head(std::numeric_limits<double>::quiet_NaN(), true)});
        cases.push_back({"an out-of-range parameter value", head(1.5, true)});
        for (bool active : {false, true}) {
            Instance a(factory, effectId);
            CHECK(a.load(good) == kResultOk);
            CHECK(a.loadController(good) == kResultOk);
            if (active) a.activate();
            const Bytes before = a.save();
            const auto audioBefore = a.active ? a.render() : std::vector<float>();
            for (const Case& c : cases) {
                CHECK(a.load(c.bytes) != kResultOk);
                if (a.save() != before) {
                    std::fprintf(stderr, "state changed after refusing: %s\n", c.name);
                    std::exit(1);
                }
            }
            if (a.active) CHECK(a.render() == audioBefore);
            else {
                a.activate();
                CHECK(a.render() == expected(0.75f, 0.25f));
            }
            // And it still takes a good state afterwards.
            state_stub::Blob again;
            again.level = 0.5f;
            again.gain = 1.5f;
            CHECK(a.load(newState(0.5, again)) == kResultOk);
            CHECK(a.render() == expected(0.5f, 1.5f));
            std::printf("VST3 state, engine %s: %zu corrupt states refused with the saved bytes and the audio unchanged; a good state loads afterwards\n",
                        active ? "active" : "inactive", cases.size());
        }
    }
    {
        // 6. The inactive refusal also holds for a fresh instance that has never
        // held a chunk: nothing is kept for the next setActive.
        Instance a(factory, effectId);
        state_stub::Blob other;
        other.level = 0.1f;
        Bytes bad = head(0.1, true);
        Bytes blob = state_stub::encode(other);
        blob[2] ^= 0x55;
        trailer(bad, blob);
        CHECK(a.load(bad) != kResultOk);
        const Bytes saved = a.save();
        CHECK(!parse(saved).hasTrailer && parse(saved).level == 0.5);
        a.activate();
        CHECK(a.render() == expected(0.5f, 1.0f));
        std::puts("VST3 state: a refused state on a fresh inactive instance leaves the defaults and no pending chunk");
    }
    factory->release();
    CHECK(exitFn());
    CFRelease(bundle);
    std::puts("VST3 state proof: parameters and the plugin's chunk round-trip through getState/setState (engine active or not, over deactivation, size query loop); version 1 states load; corrupt blobs are refused unchanged: PASS");
    return 0;
}
