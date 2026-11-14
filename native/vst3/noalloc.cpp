// SPDX-License-Identifier: MIT
// Proves that IAudioProcessor::process of the VST3 shell does not allocate.
//
// The shell is a bundle loaded into this process, and its C++ runtime and the
// Rust engine both end up in malloc, so a replacement operator new in this
// executable would not see them. Two counters cover both routes:
//   - global operator new / delete in this executable (code compiled here);
//   - libmalloc's malloc_logger hook, which sees every malloc, calloc, realloc,
//     valloc and aligned allocation in the process, including the bundle's C++
//     runtime and the Rust engine inside it. (A malloc zone registered ahead of
//     the default one was tried and does not take over malloc() on current macOS.)
// Counting is armed only around process() calls. A self-check allocates through
// each route while armed, so the proof cannot pass because the counters are blind.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R vst3_noalloc_proof --output-on-failure
#include "proof_shell.h"
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <unistd.h>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;
// libmalloc calls malloc_logger for every allocation and free in every zone of
// the process, whichever image asked: the bundle's C++ runtime, the Rust engine,
// libSystem. It is the hook the system's own allocation tools use.
extern "C" {
typedef void (malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t num_hot_frames_to_skip);
extern malloc_logger_t* malloc_logger;
}
static std::atomic<bool> g_armed{false};
constexpr uint32_t kLogAlloc = 2;  // stack_logging_type_alloc; a realloc carries it too
static std::atomic<long> g_malloc_calls{0};
static void malloc_hook(uint32_t type, uintptr_t, uintptr_t, uintptr_t, uintptr_t, uint32_t) {
    if (g_armed.load(std::memory_order_relaxed) && (type & kLogAlloc)) g_malloc_calls.fetch_add(1, std::memory_order_relaxed);
}
#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 no-alloc proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

static std::atomic<long> g_new_calls{0};

// ---- operator new / delete in this executable ------------------------------
static void* counted_new(std::size_t size) {
    if (g_armed.load(std::memory_order_relaxed)) g_new_calls.fetch_add(1, std::memory_order_relaxed);
    void* p = std::malloc(size ? size : 1);
    if (!p) throw std::bad_alloc();
    return p;
}
void* operator new(std::size_t size) { return counted_new(size); }
void* operator new[](std::size_t size) { return counted_new(size); }
void* operator new(std::size_t size, const std::nothrow_t&) noexcept { try { return counted_new(size); } catch (...) { return nullptr; } }
void* operator new[](std::size_t size, const std::nothrow_t&) noexcept { try { return counted_new(size); } catch (...) { return nullptr; } }
void operator delete(void* p) noexcept { std::free(p); }
void operator delete[](void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }
void operator delete[](void* p, std::size_t) noexcept { std::free(p); }

struct Counts {
    long news;
    long mallocs;
};
static Counts counts() { return {g_new_calls.load(), g_malloc_calls.load()}; }
static void arm() {
    g_new_calls = 0;
    g_malloc_calls = 0;
    g_armed = true;
}
static Counts disarm() {
    g_armed = false;
    return counts();
}

// volatile sinks keep the optimizer from deleting the allocations.
static void* volatile g_sink;
static void self_check() {
    arm();
    void* a = ::operator new(8);
    Counts c = disarm();
    CHECK(c.news >= 1);
    ::operator delete(a);

    // Every route into malloc, with results kept alive in a volatile so the
    // optimizer cannot drop the call. libSystem's own mallocs count too.
    arm();
    g_sink = std::malloc(64);
    c = disarm();
    CHECK(c.mallocs == 1);
    std::free(g_sink);

    arm();
    g_sink = std::calloc(1, 8);
    g_sink = std::realloc(g_sink, 4096);
    c = disarm();
    CHECK(c.mallocs == 2);
    std::free(g_sink);

    arm();
    g_sink = ::strdup("self-check");
    c = disarm();
    CHECK(c.mallocs >= 1);
    std::free(g_sink);

    arm();
    g_sink = ::getcwd(nullptr, 0);
    c = disarm();
    CHECK(c.mallocs >= 1);
    std::free(g_sink);

    // A container allocation goes through operator new and on into malloc.
    arm();
    {
        std::vector<char> v(1024);
        g_sink = v.data();
    }
    c = disarm();
    CHECK(c.news >= 1);
    CHECK(c.mallocs >= 1);

    // Nothing armed, nothing counted.
    g_sink = ::strdup("unarmed");
    std::free(g_sink);
    CHECK(counts().mallocs == c.mallocs);
    std::puts("VST3 no-alloc self-check: operator new, malloc, calloc, realloc, strdup and getcwd are counted while armed");
}

static void change(ParameterChanges& changes, ParamID id, double value) {
    int32 index = 0;
    auto* queue = changes.addParameterData(id, index);
    queue->addPoint(0, value, index);
}

// One process() call. All buffers and parameter queues exist before arming.
static long process_counted(IAudioProcessor* processor, int channels, bool in_place, ParameterChanges* changes, int blocks) {
    constexpr int kFrames = 512;
    std::vector<float> inL(kFrames), inR(kFrames), outL(kFrames), outR(kFrames);
    long total = 0;
    for (int block = 0; block < blocks; ++block) {
        fill_sine(inL.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        fill_sine(inR.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        float* inPtrs[] = {inL.data(), inR.data()};
        float* outPtrs[] = {in_place ? inL.data() : outL.data(), in_place ? inR.data() : outR.data()};
        AudioBusBuffers input{};
        input.numChannels = channels;
        input.channelBuffers32 = inPtrs;
        AudioBusBuffers output{};
        output.numChannels = channels;
        output.channelBuffers32 = outPtrs;
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kFrames;
        data.numInputs = data.numOutputs = 1;
        data.inputs = &input;
        data.outputs = &output;
        data.inputParameterChanges = changes;
        arm();
        const tresult result = processor->process(data);
        const Counts c = disarm();
        CHECK(result == kResultOk);
        total += c.news + c.mallocs;
        if (c.news + c.mallocs != 0)
            std::fprintf(stderr, "VST3 no-alloc proof: process allocated (operator new %ld, malloc %ld)\n", c.news, c.mallocs);
    }
    return total;
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    malloc_logger = malloc_hook;
    self_check();

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
    // Allocation inside the bundle itself is seen: creating an instance runs the
    // bundle's own new (libc++ in the bundle) and the Rust engine's setup.
    {
        TUID id{};
        bool seen = false;
        for (int32 i = 0; i < factory->countClasses(); ++i) {
            PClassInfo classInfo{};
            CHECK(factory->getClassInfo(i, &classInfo) == kResultOk);
            if (std::strcmp(classInfo.category, kVstAudioEffectClass) == 0) {
                std::memcpy(id, classInfo.cid, 16);
                seen = true;
            }
        }
        CHECK(seen);
        IComponent* probe = nullptr;
        arm();
        const tresult created = factory->createInstance(id, IComponent::iid, reinterpret_cast<void**>(&probe));
        const Counts c = disarm();
        CHECK(created == kResultOk && probe);
        std::printf("VST3 no-alloc self-check: creating an instance inside the bundle allocated %ld times (operator new %ld)\n", c.mallocs, c.news);
        CHECK(c.mallocs >= 1);
        probe->release();
    }
    // The effect class comes from the factory, so the same proof runs over any
    // shell bundle (the example, and the latency stub that makes the bypass delay line real).
    TUID processorId{};
    bool foundEffect = false;
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo classInfo{};
        CHECK(factory->getClassInfo(i, &classInfo) == kResultOk);
        if (std::strcmp(classInfo.category, kVstAudioEffectClass) == 0) {
            std::memcpy(processorId, classInfo.cid, 16);
            foundEffect = true;
        }
    }
    CHECK(foundEffect);

    long grand_total = 0;
    int cases = 0;
    for (int channels : {1, 2}) {
        IComponent* component = nullptr;
        CHECK(factory->createInstance(processorId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
        CHECK(component->initialize(nullptr) == kResultOk);
        IAudioProcessor* processor = nullptr;
        CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
        SpeakerArrangement arrangement = channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
        CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
        CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, 4096, 48000.0};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);

        // The bypass parameter id, from the controller's flag.
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        IEditController* controller = nullptr;
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
        ParamID bypass = 0;
        bool haveBypass = false;
        for (int32 i = 0; i < controller->getParameterCount(); ++i) {
            ParameterInfo host{};
            CHECK(controller->getParameterInfo(i, host) == kResultOk);
            if (host.flags & ParameterInfo::kIsBypass) {
                bypass = host.id;
                haveBypass = true;
            }
        }
        CHECK(haveBypass);

        // Parameter queues are prepared before arming, as a host does.
        ParameterChanges gainChanges;
        for (int32 i = 0; i < controller->getParameterCount(); ++i) {
            ParameterInfo host{};
            CHECK(controller->getParameterInfo(i, host) == kResultOk);
            if (!(host.flags & ParameterInfo::kIsBypass)) change(gainChanges, host.id, i % 2 ? 0.25 : 0.75);
        }
        ParameterChanges bypassOn;
        change(bypassOn, bypass, 1.0);
        ParameterChanges bypassOff;
        change(bypassOff, bypass, 0.0);

        struct Step {
            const char* name;
            bool in_place;
            ParameterChanges* changes;
        };
        const Step steps[] = {
            {"separate buffers", false, nullptr},
            {"in-place buffers", true, nullptr},
            {"parameter changes", false, &gainChanges},
            {"parameter changes in place", true, &gainChanges},
            {"bypass on (switch block, fading)", false, &bypassOn},
            {"bypassed", false, nullptr},
            {"bypassed in place", true, nullptr},
            {"bypass off (switch block, fading)", false, &bypassOff},
            {"processed again", true, nullptr},
        };
        for (const Step& step : steps) {
            const long allocations = process_counted(processor, channels, step.in_place, step.changes, 6);
            std::printf("VST3 no-alloc %s, %s: %ld allocations in process\n", channels == 1 ? "mono" : "stereo", step.name, allocations);
            grand_total += allocations;
            ++cases;
        }
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
    CHECK(grand_total == 0);
    std::printf("VST3 no-alloc proof (%s): %d cases (mono and stereo, separate and in-place buffers, parameter changes, bypass on and off), 0 allocations in process: PASS\n", argv[1], cases);
    return 0;
}
