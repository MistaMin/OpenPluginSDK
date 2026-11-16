// SPDX-License-Identifier: MIT
// Per-note expression through the VST3 shell (roadmap 0.7 task 4, shell ABI 7). The
// plugin is Expression Synth (examples/expression_synth): eight voices, each a sine
// whose level follows its pressure (0.25 + 0.75 p), whose second harmonic follows its
// brightness and whose pitch follows its tuning. This host speaks VST3 only:
//  - the controller offers INoteExpressionController with the seven expressions
//    (volume to brightness as the predefined types, pressure as kCustomStart), their
//    titles, units, defaults and bipolar flags, value to string and string to value,
//    and INoteExpressionPhysicalUIMapping;
//  - two notes start with note ids 1 and 2 (keys 60 and 66); pressure, brightness and
//    tuning sent as kNoteExpressionValueEvent for note id 1 change what the output holds
//    at that voice's frequencies, measured with the Goertzel algorithm, while the
//    other voice's fundamental and harmonic do not move;
//  - the negative control: the same expressions for note id 99 change nothing;
//  - hostile events (an unknown type, a NaN value, a negative note id) change nothing.
// Not a DAW test.
//
//   VST3ExpressionProof <Expression Synth.vst3>
#include <CoreFoundation/CoreFoundation.h>
#include "pluginterfaces/base/funknown.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstnoteexpression.h"
#include "pluginterfaces/vst/ivstphysicalui.h"
#include "public.sdk/source/vst/hosting/eventlist.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <string>
#include <vector>
using namespace Steinberg;
using namespace Steinberg::Vst;

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "VST3 expression proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
constexpr double kRate = 48000.0;
constexpr int kBlock = 480;
constexpr int kMeasure = 24000;  // 0.5 s: Goertzel bins 2 Hz apart, so a voice 22 Hz away leaks under 0.1%

// operator new counted while process() runs: the shell's event path is C++ and must
// not allocate (the Rust side is measured by examples/expression_synth/tests).
static thread_local bool g_armed = false;  // per thread: only the arming thread is counted
static long g_allocs = 0;
void* operator new(std::size_t size) {
    if (g_armed) ++g_allocs;
    if (void* p = std::malloc(size ? size : 1)) return p;
    throw std::bad_alloc();
}
void operator delete(void* p) noexcept { std::free(p); }
void operator delete(void* p, std::size_t) noexcept { std::free(p); }

static double hz(double key) { return 440.0 * std::pow(2.0, (key - 69.0) / 12.0); }

// Amplitude of a sinusoid at `f` in `x` (Goertzel over a Hann window, scaled so a unit
// sine reads 1).
static double magnitude(const std::vector<float>& x, double f) {
    const double w = 2.0 * M_PI * f / kRate;
    const double c = 2.0 * std::cos(w);
    double s1 = 0.0, s2 = 0.0, sum = 0.0;
    const size_t n = x.size();
    for (size_t i = 0; i < n; ++i) {
        const double hann = 0.5 - 0.5 * std::cos(2.0 * M_PI * static_cast<double>(i) / static_cast<double>(n - 1));
        sum += hann;
        const double s0 = x[i] * hann + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    const double re = s1 - s2 * std::cos(w);
    const double im = s2 * std::sin(w);
    return 2.0 * std::sqrt(re * re + im * im) / sum;
}

static std::string utf8(const TChar* text) {
    std::string out;
    for (size_t i = 0; text[i]; ++i) out += static_cast<char>(text[i] < 0x80 ? text[i] : '?');
    return out;
}

static void utf16(const char* text, String128 out) {
    size_t i = 0;
    for (; text[i] && i < 127; ++i) out[i] = static_cast<TChar>(text[i]);
    out[i] = 0;
}

struct Plug {
    CFBundleRef bundle = nullptr;
    bool (*exitFn)() = nullptr;
    IPluginFactory* factory = nullptr;
    IComponent* component = nullptr;
    IAudioProcessor* processor = nullptr;
    IEditController* controller = nullptr;
    std::vector<float> left = std::vector<float>(kBlock), right = std::vector<float>(kBlock);

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
        TUID controllerId{};
        CHECK(component->getControllerClassId(controllerId) == kResultOk);
        CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
        CHECK(controller->initialize(nullptr) == kResultOk);
    }
    ~Plug() {
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        processor->release();
        controller->terminate();
        controller->release();
        component->terminate();
        component->release();
        factory->release();
        exitFn();
        CFRelease(bundle);
    }
    void start() {
        CHECK(component->getBusCount(kAudio, kInput) == 0);
        CHECK(component->getBusCount(kAudio, kOutput) == 1 && component->getBusCount(kEvent, kInput) == 1);
        CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
        CHECK(component->activateBus(kEvent, kInput, 0, true) == kResultOk);
        ProcessSetup setup{kRealtime, kSample32, kBlock, kRate};
        CHECK(processor->setupProcessing(setup) == kResultOk);
        CHECK(component->setActive(true) == kResultOk);
        CHECK(processor->setProcessing(true) == kResultOk);
    }
    // One block with `events` at its start; appends the left channel to `into`.
    void block(EventList* events, std::vector<float>* into) {
        float* ptrs[2] = {left.data(), right.data()};
        AudioBusBuffers output{};
        output.numChannels = 2;
        output.channelBuffers32 = ptrs;
        ProcessData data{};
        data.processMode = kRealtime;
        data.symbolicSampleSize = kSample32;
        data.numSamples = kBlock;
        data.numInputs = 0;
        data.inputs = nullptr;
        data.numOutputs = 1;
        data.outputs = &output;
        data.inputEvents = events;
        g_armed = true;
        const tresult result = processor->process(data);
        g_armed = false;
        CHECK(result == kResultOk);
        for (float s : left) CHECK(std::isfinite(s));
        if (into) into->insert(into->end(), left.begin(), left.end());
    }
    // Send `events`, let the 5 ms ramps settle, then measure.
    std::vector<float> play(EventList* events) {
        block(events, nullptr);
        for (int i = 0; i < 4; ++i) block(nullptr, nullptr);
        std::vector<float> out;
        while (static_cast<int>(out.size()) < kMeasure) block(nullptr, &out);
        out.resize(kMeasure);
        return out;
    }
};

static Event noteOn(int32 id, int16 key) {
    Event e{};
    e.type = Event::kNoteOnEvent;
    e.noteOn.channel = 0;
    e.noteOn.pitch = key;
    e.noteOn.velocity = 0.8f;
    e.noteOn.noteId = id;
    return e;
}

static Event expression(int32 id, NoteExpressionTypeID type, double normalized) {
    Event e{};
    e.type = Event::kNoteExpressionValueEvent;
    e.noteExpressionValue.typeId = type;
    e.noteExpressionValue.noteId = id;
    e.noteExpressionValue.value = normalized;
    return e;
}

constexpr NoteExpressionTypeID kPressure = NoteExpressionTypeIDs::kCustomStart;
const double kKeyA = 60, kKeyB = 66;

struct Spectrum {
    double a, b, a2, b2, tuned;
};

static Spectrum spectrum(const std::vector<float>& x) {
    return {magnitude(x, hz(kKeyA)), magnitude(x, hz(kKeyB)), magnitude(x, 2 * hz(kKeyA)), magnitude(x, 2 * hz(kKeyB)), magnitude(x, hz(kKeyA + 7))};
}

static bool same(double x, double y) { return std::fabs(x - y) <= 0.01 * std::max(std::fabs(x), std::fabs(y)) + 1e-5; }

static void print(const char* what, const Spectrum& s) {
    std::printf("VST3 expression: %-34s key 60 %.4f, key 66 %.4f, 2nd harmonics %.4f / %.4f, key 67 %.4f\n", what, s.a, s.b, s.a2, s.b2, s.tuned);
}

static void controllerInfo(IEditController* controller) {
    FUnknownPtr<INoteExpressionController> notes(controller);
    CHECK(notes);
    CHECK(notes->getNoteExpressionCount(0, 0) == 7);
    CHECK(notes->getNoteExpressionCount(0, 15) == 7);
    CHECK(notes->getNoteExpressionCount(1, 0) == 0);
    CHECK(notes->getNoteExpressionCount(0, 16) == 0);
    struct Want {
        NoteExpressionTypeID type;
        const char* title;
        const char* unit;
        double def;
        bool bipolar;
    };
    const Want want[] = {
        {kVolumeTypeID, "Volume", "dB", 0.25, false}, {kPanTypeID, "Pan", "", 0.5, true},
        {kTuningTypeID, "Tuning", "st", 0.5, true},   {kVibratoTypeID, "Vibrato", "%", 0.0, false},
        {kExpressionTypeID, "Expression", "%", 0.0, false}, {kBrightnessTypeID, "Brightness", "%", 0.0, false},
        {kPressure, "Pressure", "%", 0.0, false},
    };
    for (int32 i = 0; i < 7; ++i) {
        NoteExpressionTypeInfo info{};
        CHECK(notes->getNoteExpressionInfo(0, 0, i, info) == kResultOk);
        CHECK(info.typeId == want[i].type);
        CHECK(utf8(info.title) == want[i].title);
        CHECK(utf8(info.units) == want[i].unit);
        CHECK(std::fabs(info.valueDesc.defaultValue - want[i].def) < 1e-9);
        CHECK(info.valueDesc.minimum == 0.0 && info.valueDesc.maximum == 1.0 && info.valueDesc.stepCount == 0);
        CHECK(((info.flags & NoteExpressionTypeInfo::kIsBipolar) != 0) == want[i].bipolar);
        std::printf("VST3 expression: info %d: type %u %-10s unit '%s' default %.2f%s\n", i, info.typeId, utf8(info.title).c_str(), utf8(info.units).c_str(),
                    info.valueDesc.defaultValue, want[i].bipolar ? " bipolar" : "");
    }
    NoteExpressionTypeInfo info{};
    CHECK(notes->getNoteExpressionInfo(0, 0, 7, info) == kInvalidArgument);
    CHECK(notes->getNoteExpressionInfo(0, 0, -1, info) == kInvalidArgument);

    String128 text{};
    CHECK(notes->getNoteExpressionStringByValue(0, 0, kTuningTypeID, 0.5 + 7.0 / 240.0, text) == kResultOk);
    CHECK(utf8(text) == "+7.00 st");
    CHECK(notes->getNoteExpressionStringByValue(0, 0, kVolumeTypeID, 0.25, text) == kResultOk);
    CHECK(utf8(text) == "0.0 dB");
    CHECK(notes->getNoteExpressionStringByValue(0, 0, kPressure, 0.5, text) == kResultOk);
    CHECK(utf8(text) == "50%");
    CHECK(notes->getNoteExpressionStringByValue(0, 0, kTextTypeID, 0.5, text) == kInvalidArgument);
    double value = -1.0;
    String128 typed{};
    utf16("-7 st", typed);
    CHECK(notes->getNoteExpressionValueByString(0, 0, kTuningTypeID, typed, value) == kResultOk);
    CHECK(std::fabs(value - (0.5 - 7.0 / 240.0)) < 1e-6);
    utf16("+6 dB", typed);
    CHECK(notes->getNoteExpressionValueByString(0, 0, kVolumeTypeID, typed, value) == kResultOk);
    CHECK(std::fabs(value - 0.25 * std::pow(10.0, 6.0 / 20.0)) < 1e-4);
    utf16("loud", typed);
    CHECK(notes->getNoteExpressionValueByString(0, 0, kPressure, typed, value) == kResultFalse);
    std::puts("VST3 expression: tuning +7.00 st, volume 0.0 dB, pressure 50% as text and back; unknown type and bad text refused");

    FUnknownPtr<INoteExpressionPhysicalUIMapping> physical(controller);
    CHECK(physical);
    PhysicalUIMap map[3] = {{kPUIXMovement, kInvalidTypeID}, {kPUIYMovement, kInvalidTypeID}, {kPUIPressure, kInvalidTypeID}};
    PhysicalUIMapList list{3, map};
    CHECK(physical->getPhysicalUIMapping(0, 0, list) == kResultOk);
    CHECK(map[0].noteExpressionTypeID == kTuningTypeID && map[1].noteExpressionTypeID == kBrightnessTypeID && map[2].noteExpressionTypeID == kPressure);
    std::puts("VST3 expression: physical UI X -> tuning, Y -> brightness, pressure -> pressure");
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    Plug p(argv[1]);
    controllerInfo(p.controller);
    p.start();

    EventList notes;
    Event a = noteOn(1, static_cast<int16>(kKeyA)), b = noteOn(2, static_cast<int16>(kKeyB));
    notes.addEvent(a);
    notes.addEvent(b);
    const Spectrum base = spectrum(p.play(&notes));
    print("two notes, ids 1 and 2:", base);
    CHECK(base.a > 0.005 && same(base.a, base.b));
    CHECK(base.a2 < 0.01 * base.a && base.b2 < 0.01 * base.b && base.tuned < 0.01 * base.a);

    // Negative control first: every expression for a note id no voice has.
    EventList stray;
    Event s1 = expression(99, kPressure, 1.0), s2 = expression(99, kBrightnessTypeID, 1.0), s3 = expression(99, kTuningTypeID, 0.0);
    stray.addEvent(s1);
    stray.addEvent(s2);
    stray.addEvent(s3);
    // Hostile: an unknown type, a NaN value, a negative id.
    Event h1 = expression(1, kTextTypeID, 1.0), h2 = expression(1, kPressure, std::nan("")), h3 = expression(-1, kTuningTypeID, 1.0);
    stray.addEvent(h1);
    stray.addEvent(h2);
    stray.addEvent(h3);
    const Spectrum control = spectrum(p.play(&stray));
    print("note id 99, unknown, NaN, id -1:", control);
    CHECK(same(control.a, base.a) && same(control.b, base.b) && control.a2 < 0.01 * base.a && control.tuned < 0.01 * base.a);

    // Pressure 1 on note id 1: its level goes from 0.25 to 1, four times; note 2 stays.
    EventList press;
    Event e1 = expression(1, kPressure, 1.0);
    press.addEvent(e1);
    const Spectrum pressed = spectrum(p.play(&press));
    print("pressure 1 on note id 1:", pressed);
    CHECK(std::fabs(pressed.a / base.a - 4.0) < 0.05);
    CHECK(same(pressed.b, base.b) && pressed.b2 < 0.01 * base.b);

    // Brightness 0.5 on note id 1: its second harmonic at half its fundamental.
    EventList bright;
    Event e2 = expression(1, kBrightnessTypeID, 0.5);
    bright.addEvent(e2);
    const Spectrum brightened = spectrum(p.play(&bright));
    print("brightness 0.5 on note id 1:", brightened);
    CHECK(std::fabs(brightened.a2 / brightened.a - 0.5) < 0.02);
    CHECK(same(brightened.b, base.b) && brightened.b2 < 0.01 * base.b);

    // Tuning +7 semitones on note id 1: key 60 moves to key 67; note 2 stays.
    EventList tune;
    Event e3 = expression(1, kTuningTypeID, 0.5 + 7.0 / 240.0);
    tune.addEvent(e3);
    const Spectrum tuned = spectrum(p.play(&tune));
    print("tuning +7 st on note id 1:", tuned);
    CHECK(tuned.a < 0.01 * pressed.a && std::fabs(tuned.tuned / pressed.a - 1.0) < 0.05);
    CHECK(same(tuned.b, base.b) && tuned.b2 < 0.01 * base.b);

    CHECK(g_allocs == 0);
    std::puts("VST3 expression: no operator new in process() with note and expression events");
    std::puts("VST3 expression proof: note ids, pressure, brightness and tuning reach one voice only; note id 99 changes nothing: PASS");
    return 0;
}
