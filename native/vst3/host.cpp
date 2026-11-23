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
#include "pluginterfaces/vst/ivstmessage.h"
#include "pluginterfaces/vst/ivstunits.h"
#include "base/source/fstreamer.h"
#include "public.sdk/source/common/memorystream.h"
#include "public.sdk/source/vst/hosting/hostclasses.h"
#include "public.sdk/source/vst/hosting/parameterchanges.h"
#include <algorithm>
#include <chrono>
#include <thread>
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
    CHECK(info.abi >= OP_SHELL_ABI && op_shell_abi_version() >= OP_SHELL_ABI);
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
// Parameter groups (ABI 5) as units: the root unit, one unit per group path
// and per parent of a path, nested by path and named by the last name; every
// parameter's unitId is the unit of its group, the bypass is in the root. A
// plugin without groups offers no IUnitInfo and keeps every parameter in the
// root unit, as before groups existed. The expected tree comes from the ABI.
static std::string abi_group(uint32_t index) {
    char buf[512];
    const size_t length = op_shell_param_group(index, buf, sizeof(buf));
    CHECK(length < sizeof(buf));
    return std::string(buf, length);
}
static void check_units(IEditController* controller) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    IUnitInfo* units = nullptr;
    const bool offered = controller->queryInterface(IUnitInfo::iid, reinterpret_cast<void**>(&units)) == kResultOk && units;
    bool grouped = false;
    for (uint32_t i = 0; i < info.param_count; ++i) grouped = grouped || !abi_group(i).empty();
    if (!grouped) {
        CHECK(!offered);
        for (int32 i = 0; i < controller->getParameterCount(); ++i) {
            ParameterInfo host{};
            CHECK(controller->getParameterInfo(i, host) == kResultOk);
            CHECK(host.unitId == kRootUnitId);
        }
        std::printf("VST3 units: the plugin has no groups; no IUnitInfo, every parameter in the root unit\n");
        return;
    }
    CHECK(offered);
    // Units by id: path, parent, name. Id 0 is the root.
    struct Expected {
        UnitID parent;
        std::string path;
        std::string name;
    };
    const int32 count = units->getUnitCount();
    CHECK(count >= 2);
    std::vector<Expected> seen(static_cast<size_t>(count));
    bool root = false;
    for (int32 n = 0; n < count; ++n) {
        UnitInfo unit{};
        CHECK(units->getUnitInfo(n, unit) == kResultOk);
        CHECK(unit.id >= 0 && unit.id < count);
        CHECK(unit.programListId == kNoProgramListId);
        const std::string name = ascii_of(unit.name);
        CHECK(!name.empty());
        if (unit.id == kRootUnitId) {
            CHECK(unit.parentUnitId == kNoParentUnitId);
            root = true;
            continue;
        }
        CHECK(unit.parentUnitId >= 0 && unit.parentUnitId < count && unit.parentUnitId != unit.id);
        seen[static_cast<size_t>(unit.id)] = {unit.parentUnitId, "", name};
    }
    CHECK(root);
    // A unit's path is its parents' names and its own, from the root.
    for (int32 id = 1; id < count; ++id) {
        std::string path = seen[static_cast<size_t>(id)].name;
        for (UnitID up = seen[static_cast<size_t>(id)].parent, hops = 0; up != kRootUnitId; up = seen[static_cast<size_t>(up)].parent, ++hops) {
            CHECK(hops < count);
            path = seen[static_cast<size_t>(up)].name + "/" + path;
        }
        seen[static_cast<size_t>(id)].path = path;
        std::printf("VST3 unit %d: \"%s\" (parent %d), path \"%s\"\n", id, seen[static_cast<size_t>(id)].name.c_str(),
                    seen[static_cast<size_t>(id)].parent, path.c_str());
    }
    // Every path and every parent of a path has exactly one unit, and nothing else does.
    std::vector<std::string> paths;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        const std::string group = abi_group(i);
        for (size_t at = 0; !group.empty();) {
            const size_t slash = group.find('/', at);
            const std::string prefix = slash == std::string::npos ? group : group.substr(0, slash);
            if (std::find(paths.begin(), paths.end(), prefix) == paths.end()) paths.push_back(prefix);
            if (slash == std::string::npos) break;
            at = slash + 1;
        }
    }
    CHECK(paths.size() == static_cast<size_t>(count - 1));
    auto unitOf = [&](const std::string& path) -> UnitID {
        if (path.empty()) return kRootUnitId;
        UnitID found = -1;
        for (int32 id = 1; id < count; ++id)
            if (seen[static_cast<size_t>(id)].path == path) {
                CHECK(found < 0);
                found = id;
            }
        CHECK(found > 0);
        return found;
    };
    for (const std::string& path : paths) unitOf(path);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        ParameterInfo host{};
        CHECK(controller->getParameterInfo(static_cast<int32>(i), host) == kResultOk);
        CHECK(host.unitId == unitOf(abi_group(i)));
    }
    for (int32 i = static_cast<int32>(info.param_count); i < controller->getParameterCount(); ++i) {
        ParameterInfo host{};
        CHECK(controller->getParameterInfo(i, host) == kResultOk);
        CHECK(host.unitId == kRootUnitId);
    }
    CHECK(units->getSelectedUnit() == kRootUnitId);
    CHECK(units->selectUnit(1) == kResultTrue && units->getSelectedUnit() == 1);
    CHECK(units->selectUnit(count) == kResultFalse && units->getSelectedUnit() == 1);
    CHECK(units->selectUnit(kRootUnitId) == kResultTrue);
    UnitInfo outside{};
    CHECK(units->getUnitInfo(count, outside) != kResultOk);
    CHECK(units->getProgramListCount() == 0);
    units->release();
    std::printf("VST3 units: %d, nested by path; every parameter in its group's unit\n", count);
}

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

// Parameter text: the host asks the controller for the text of a normalized
// value and for the value of a typed text. The text is the plugin's own
// (op_shell_param_format), not the normalized number; typed text is read as
// the plugin reads it (op_shell_param_parse).
static std::string ascii_of_units(IEditController* controller, ParamID id) {
    ParameterInfo info{};
    if (controller->getParameterInfo(static_cast<int32>(id), info) != kResultOk) return {};
    return ascii_of(info.units);
}

static std::string text_of(IEditController* controller, ParamID id, double normalized) {
    String128 text{};
    CHECK(controller->getParamStringByValue(id, normalized, text) == kResultOk);
    return ascii_of(text);
}
static bool value_of(IEditController* controller, ParamID id, const char* ascii, double& normalized) {
    String128 text{};
    UString(text, 128).fromAscii(ascii);
    return controller->getParamValueByString(id, text, normalized) == kResultOk;
}
static std::string abi_text(uint32_t index, double plain) {
    char buf[128];
    const size_t length = op_shell_param_format(index, plain, buf, sizeof(buf));
    CHECK(length < sizeof(buf));
    return std::string(buf, length);
}
static void check_parameter_text(IEditController* controller, ParamID bypass, const char* label) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0);
    uint32_t discreteSeen = 0;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        // Every parameter: the controller shows the plugin's text for the plain
        // value at the ends, the default and the middle, and reads it back.
        for (double normalized : {0.0, desc.default_normalized, 0.5, 1.0}) {
            const double plain = op_shell_to_plain(i, normalized);
            const std::string shown = text_of(controller, i, normalized);
            // The units are VST3's own field, so the text leaves out a trailing unit.
            std::string expected = abi_text(i, plain);
            const std::string unit(desc.unit);
            if (!unit.empty() && expected.size() > unit.size() &&
                expected.compare(expected.size() - unit.size(), unit.size(), unit) == 0) {
                expected.erase(expected.size() - unit.size());
                while (!expected.empty() && expected.back() == ' ') expected.pop_back();
            }
            CHECK(shown == expected);
            CHECK(!shown.empty());
            double back = -1.0;
            CHECK(value_of(controller, i, shown.c_str(), back));
            // The text is rounded for display, so a continuous parameter comes
            // back within its display precision.
            // A discrete parameter snaps to a step, so only its steps are compared (below).
            if (desc.steps < 2) CHECK(std::fabs(back - normalized) <= 0.02);
        }
        if (desc.steps > 1) ++discreteSeen;
    }
    // The example's Gain (-24 to 24 dB, the first parameter): 0.625 is +6 dB.
    OpParamDesc gain{};
    CHECK(op_shell_param(0, &gain) != 0);
    if (gain.min == -24.0 && gain.max == 24.0 && std::string(gain.unit) == "dB") {
        const std::string shown = text_of(controller, 0, 0.625);
        std::printf("VST3 %s Gain at normalized 0.625 shows \"%s\"\n", label, shown.c_str());
        CHECK(shown == "6.0");  // the host adds the units field, "dB"
        CHECK(ascii_of_units(controller, 0) == "dB");
        CHECK(shown.find("0.6250") == std::string::npos);
        double typed = -1.0;
        CHECK(value_of(controller, 0, "6 dB", typed));
        std::printf("VST3 %s \"6 dB\" reads as normalized %.6f\n", label, typed);
        CHECK(std::fabs(typed - 0.625) < 1e-6);
        CHECK(value_of(controller, 0, "-24", typed) && std::fabs(typed) < 1e-6);
        CHECK(value_of(controller, 0, "+24 dB", typed) && std::fabs(typed - 1.0) < 1e-6);
        CHECK(value_of(controller, 0, "99 dB", typed) && std::fabs(typed - 1.0) < 1e-6);  // clamped
    }
    // Text that is not a value is refused and leaves the value alone.
    for (const char* bad : {"", "loud", "3 parsecs"}) {
        double value = 0.123;
        CHECK(!value_of(controller, 0, bad, value));
        CHECK(value == 0.123);
    }
    // A toggle or choice round-trips its label.
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        if (desc.steps < 2 || desc.steps > 64) continue;
        for (uint32_t step = 0; step < desc.steps; ++step) {
            const double normalized = static_cast<double>(step) / (desc.steps - 1);
            const std::string label_text = text_of(controller, i, normalized);
            double back = -1.0;
            CHECK(value_of(controller, i, label_text.c_str(), back));
            CHECK(std::fabs(back - normalized) < 1e-9);
        }
        if (std::string(desc.unit) != "dB" && desc.steps == 2) {
            std::printf("VST3 %s %s: Off \"%s\", On \"%s\"\n", label, desc.name, text_of(controller, i, 0.0).c_str(), text_of(controller, i, 1.0).c_str());
        }
    }
    CHECK(discreteSeen >= 1);
    // Bypass reads as a switch.
    CHECK(text_of(controller, bypass, 0.0) == "Off");
    CHECK(text_of(controller, bypass, 1.0) == "On");
    double value = -1.0;
    CHECK(value_of(controller, bypass, "on", value) && value == 1.0);
    CHECK(value_of(controller, bypass, " Off ", value) && value == 0.0);
    CHECK(!value_of(controller, bypass, "maybe", value));
    // Hostile calls reach no crash: an unknown id and a null string.
    String128 sink{};
    CHECK(controller->getParamStringByValue(0x7FFFFFF0u, 0.5, sink) != kResultOk);
    double ignored = 0.0;
    CHECK(controller->getParamValueByString(0x7FFFFFF0u, sink, ignored) != kResultOk);
    std::printf("VST3 %s parameter text: plugin text shown and read back for %u parameters\n", label, info.param_count);
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

// ---- Sample-accurate parameter changes and bypass --------------------------
// A change point: a parameter (or the bypass, index < 0) takes `value`
// (normalized) from sample `offset` of the block.
struct ChangePoint {
    int offset;
    int index;
    double value;
};
struct Stereo {
    std::vector<float> left;
    std::vector<float> right;
};
static void make_input(Stereo& in, int frames) {
    in.left.resize(frames);
    in.right.resize(frames);
    fill_sine(in.left.data(), frames, 48000.0, 1000.0, 0.25, 0);
    fill_sine(in.right.data(), frames, 48000.0, 1300.0, 0.2, 0);
}
// Put the processor and the reference on the same values, not bypassed, with a
// fresh engine: values go in through a zero-sample block, as a host may send.
static void prime(IAudioProcessor* processor, void* reference, const std::vector<double>& normalized, ParamID bypass) {
    ParameterChanges changes;
    for (uint32_t i = 0; i < normalized.size(); ++i) change(changes, static_cast<ParamID>(i), normalized[i]);
    change(changes, bypass, 0.0);
    ProcessData data{};
    data.processMode = kRealtime;
    data.symbolicSampleSize = kSample32;
    data.numSamples = 0;
    data.inputParameterChanges = &changes;
    CHECK(processor->process(data) == kResultOk);
    CHECK(processor->setProcessing(false) == kResultOk);
    CHECK(processor->setProcessing(true) == kResultOk);
    for (uint32_t i = 0; i < normalized.size(); ++i) CHECK(op_shell_set_normalized(reference, i, static_cast<float>(normalized[i])) != 0);
    op_shell_reset(reference);
}
// One block through the VST3 shell with the points as parameter queues.
static Stereo host_block(IAudioProcessor* processor, int channels, const Stereo& in, const std::vector<ChangePoint>& points, ParamID bypass, bool in_place) {
    const int frames = static_cast<int>(in.left.size());
    Stereo work = in;
    Stereo out;
    out.left.assign(frames, 0.0f);
    out.right.assign(frames, 0.0f);
    float* inPtrs[] = {work.left.data(), work.right.data()};
    float* outPtrs[] = {in_place ? work.left.data() : out.left.data(), in_place ? work.right.data() : out.right.data()};
    AudioBusBuffers input{};
    input.numChannels = channels;
    input.channelBuffers32 = inPtrs;
    AudioBusBuffers output{};
    output.numChannels = channels;
    output.channelBuffers32 = outPtrs;
    ParameterChanges changes;
    for (const ChangePoint& point : points) {
        int32 index = 0;
        auto* queue = changes.addParameterData(point.index < 0 ? bypass : static_cast<ParamID>(point.index), index);
        queue->addPoint(point.offset, point.value, index);
    }
    ProcessData data{};
    data.processMode = kRealtime;
    data.symbolicSampleSize = kSample32;
    data.numSamples = frames;
    data.numInputs = data.numOutputs = 1;
    data.inputs = &input;
    data.outputs = &output;
    data.inputParameterChanges = points.empty() ? nullptr : &changes;
    CHECK(processor->process(data) == kResultOk);
    return in_place ? work : out;
}
// The same block straight through the C ABI: split at the change offsets (and at
// `limit` frames), set the values that change at each start, then process.
// Bypass points are skipped here; bypass_model applies them.
static Stereo abi_block(void* reference, int channels, const Stereo& in, std::vector<ChangePoint> points, int limit) {
    const int frames = static_cast<int>(in.left.size());
    Stereo out;
    out.left.assign(frames, 0.0f);
    out.right.assign(frames, 0.0f);
    points.erase(std::remove_if(points.begin(), points.end(), [](const ChangePoint& p) { return p.index < 0; }), points.end());
    std::stable_sort(points.begin(), points.end(), [](const ChangePoint& a, const ChangePoint& b) { return a.offset < b.offset; });
    size_t next = 0;
    int pos = 0;
    while (pos < frames) {
        while (next < points.size() && points[next].offset <= pos) {
            CHECK(op_shell_set_normalized(reference, static_cast<uint32_t>(points[next].index), static_cast<float>(points[next].value)) != 0);
            ++next;
        }
        int end = frames;
        if (next < points.size()) end = std::min(end, points[next].offset);
        const int n = std::min(limit, end - pos);
        CHECK(op_shell_process(reference, in.left.data() + pos, channels > 1 ? in.right.data() + pos : nullptr,
                               out.left.data() + pos, channels > 1 ? out.right.data() + pos : nullptr, static_cast<uint32_t>(n)) != 0);
        pos += n;
    }
    return out;
}
static double stereo_difference(const Stereo& a, const Stereo& b, int channels) {
    double worst = max_difference(a.left, b.left);
    if (channels > 1) worst = std::max(worst, max_difference(a.right, b.right));
    return worst;
}
// First sample at which two blocks differ, or -1.
static int first_difference(const Stereo& a, const Stereo& b) {
    for (size_t i = 0; i < a.left.size(); ++i)
        if (a.left[i] != b.left[i]) return static_cast<int>(i);
    return -1;
}
// The shell's bypass, written out: the mix moves towards its target by one step
// per sample, starting at the sample where the target changes. dry is the
// input delayed by `latency`.
static Stereo bypass_model(const Stereo& processed, const Stereo& in, int channels, int latency, const std::vector<ChangePoint>& points) {
    const int frames = static_cast<int>(in.left.size());
    const float step = static_cast<float>(1.0 / std::max(1.0, 0.010 * 48000.0));
    Stereo out = processed;
    float mix = 0.0f;
    float target = 0.0f;
    size_t next = 0;
    for (int i = 0; i < frames; ++i) {
        for (; next < points.size(); ++next) {
            if (points[next].offset > i) break;
            if (points[next].index < 0) target = points[next].value >= 0.5 ? 1.0f : 0.0f;
        }
        if (mix < target) mix = std::min(target, mix + step);
        else if (mix > target) mix = std::max(target, mix - step);
        for (int c = 0; c < channels; ++c) {
            const std::vector<float>& src = c ? in.right : in.left;
            std::vector<float>& dst = c ? out.right : out.left;
            const float dry = i - latency >= 0 ? src[i - latency] : 0.0f;
            dst[i] = mix >= 1.0f ? dry : dst[i] + (dry - dst[i]) * mix;
        }
    }
    return out;
}
static void check_sample_accurate(IComponent* component, IAudioProcessor* processor, void* reference, int channels, uint32_t gain, uint32_t warm, ParamID bypass, int maxBlock) {
    const char* label = channels == 1 ? "mono" : "stereo";
    std::vector<double> initial;
    {
        const std::vector<float> defaults = shell_defaults();
        for (uint32_t i = 0; i < defaults.size(); ++i) initial.push_back(op_shell_to_normalized(i, defaults[i]));
    }
    initial[gain] = 0.5;
    initial[warm] = 0.0;
    const int latency = static_cast<int>(processor->getLatencySamples());
    struct Case {
        const char* name;
        int frames;
        std::vector<ChangePoint> points;
    };
    const int g = static_cast<int>(gain);
    const int w = static_cast<int>(warm);
    std::vector<ChangePoint> ramp;
    for (int k = 0; k < 24; ++k) ramp.push_back({100 + k * 37, g, 0.55 + 0.35 * k / 23.0});
    const Case cases[] = {
        {"one change at offset 300", 1024, {{300, g, 0.9}}},
        {"change at the last sample", 1024, {{1023, g, 0.9}}},
        {"24-point ramp in one queue", 1024, ramp},
        {"two queues, different offsets", 1024, {{200, g, 0.8}, {640, w, 1.0}, {900, g, 0.3}}},
        {"two queues, same offset", 1024, {{500, g, 0.2}, {500, w, 1.0}}},
        {"block larger than the prepared size", 3 * maxBlock + 1234, {{maxBlock - 1, g, 0.9}, {maxBlock + 5, w, 1.0}, {2 * maxBlock + 77, g, 0.1}}},
    };
    for (const Case& c : cases) {
        Stereo in;
        make_input(in, c.frames);
        for (bool in_place : {false, true}) {
            prime(processor, reference, initial, bypass);
            const Stereo hostOut = host_block(processor, channels, in, c.points, bypass, in_place);
            prime(processor, reference, initial, bypass);
            const Stereo abiOut = abi_block(reference, channels, in, c.points, maxBlock);
            const double diff = stereo_difference(hostOut, abiOut, channels);
            std::printf("VST3 %s%s, %s: max difference from the ABI split at the same offsets %.3g\n", label, in_place ? " in place" : "", c.name, diff);
            CHECK(diff == 0.0);
        }
        // The change lands at its sample: the output is the unchanged render up
        // to the first change point and differs from it at that very sample.
        int firstPoint = c.frames;
        for (const ChangePoint& p : c.points) firstPoint = std::min(firstPoint, p.offset);
        prime(processor, reference, initial, bypass);
        const Stereo unchanged = abi_block(reference, channels, in, {}, maxBlock);
        prime(processor, reference, initial, bypass);
        const Stereo hostOut = host_block(processor, channels, in, c.points, bypass, false);
        const int firstChange = first_difference(unchanged, hostOut);
        std::printf("VST3 %s, %s: first sample that differs from the unchanged render: %d (first change point %d)\n", label, c.name, firstChange, firstPoint);
        CHECK(firstChange == firstPoint);
        // And it is not a block-level change: every value at sample 0 gives another block.
        std::vector<ChangePoint> early = c.points;
        for (ChangePoint& p : early) p.offset = 0;
        prime(processor, reference, initial, bypass);
        const Stereo blockLevel = abi_block(reference, channels, in, early, maxBlock);
        CHECK(stereo_difference(hostOut, blockLevel, channels) > 1e-3);
    }
    // Bypass mid-block: the crossfade starts at the sample of the change.
    struct BypassCase {
        const char* name;
        std::vector<ChangePoint> points;
    };
    const BypassCase bypassCases[] = {
        {"bypass on at 300", {{300, -1, 1.0}}},
        {"bypass on at 100, off at 700", {{100, -1, 1.0}, {700, -1, 0.0}}},
        {"bypass on at 250 with a gain change at 600", {{250, -1, 1.0}, {600, g, 0.2}}},
    };
    // The default gain is near unity, which would make the processed and the dry
    // signal alike; start the bypass cases at a clearly audible gain.
    initial[gain] = 0.85;
    for (const BypassCase& c : bypassCases) {
        Stereo in;
        make_input(in, 1536);
        for (bool in_place : {false, true}) {
            prime(processor, reference, initial, bypass);
            const Stereo hostOut = host_block(processor, channels, in, c.points, bypass, in_place);
            prime(processor, reference, initial, bypass);
            const Stereo processed = abi_block(reference, channels, in, c.points, maxBlock);
            const Stereo expected = bypass_model(processed, in, channels, latency, c.points);
            const double diff = stereo_difference(hostOut, expected, channels);
            std::printf("VST3 %s%s, %s: max difference from the crossfade model %.3g\n", label, in_place ? " in place" : "", c.name, diff);
            CHECK(diff <= 1e-6);
        }
        // Before the first bypass point the output is the processed signal exactly,
        // and at that sample it starts to move.
        prime(processor, reference, initial, bypass);
        const Stereo hostOut = host_block(processor, channels, in, c.points, bypass, false);
        prime(processor, reference, initial, bypass);
        const Stereo processed = abi_block(reference, channels, in, c.points, maxBlock);
        const int first = c.points[0].offset;
        for (int i = 0; i < first; ++i) CHECK(hostOut.left[i] == processed.left[i]);
        CHECK(hostOut.left[first] != processed.left[first]);
    }
    // More points than the shell's scratch holds (3 queues x 4096 points in one
    // 4096-sample block): the queues are thinned, but the last value of every
    // queue is kept, so the state after the block is the last point of each.
    {
        Stereo in;
        make_input(in, 4096);
        prime(processor, reference, initial, bypass);
        std::vector<ChangePoint> dense;
        for (int i = 0; i < 4096; ++i) {
            dense.push_back({i, g, 0.2 + 0.6 * (i % 11) / 10.0});
            dense.push_back({i, w, (i / 3) % 2 ? 1.0 : 0.0});
            dense.push_back({i, -1, (i / 5) % 2 ? 1.0 : 0.0});
        }
        dense[3 * 4095] = {4095, g, 0.3125};
        dense[3 * 4095 + 1] = {4095, w, 1.0};
        dense[3 * 4095 + 2] = {4095, -1, 1.0};
        const Stereo out = host_block(processor, channels, in, dense, bypass, false);
        for (float sample : out.left) CHECK(std::isfinite(sample));
        MemoryStream state;
        CHECK(component->getState(&state) == kResultOk);
        const char* bytes = state.getData();
        const int64 size = state.getSize();
        CHECK(size >= static_cast<int64>(8 + 8 * initial.size() + 4));
        double held[8] = {};
        CHECK(initial.size() <= 8);
        std::memcpy(held, bytes + 8, 8 * initial.size());
        int32_t bypassed = 0;
        std::memcpy(&bypassed, bytes + 8 + 8 * initial.size(), 4);
        std::printf("VST3 %s, 3 x 4096 points in one block: the last values are kept (gain %.4f, warm %.0f, bypass %d)\n", label, held[gain], held[warm], bypassed);
        CHECK(held[gain] == static_cast<double>(0.3125));
        CHECK(held[warm] == 1.0);
        CHECK(bypassed == 1);
    }
}
// ---- Edits while no audio runs ---------------------------------------------
// The value of parameter `id` (or the bypass flag) in the component's saved state.
static double saved_value(IComponent* component, ParamID id, ParamID bypass) {
    MemoryStream state;
    CHECK(component->getState(&state) == kResultOk);
    state.seek(0, IBStream::kIBSeekSet, nullptr);
    IBStreamer reader(&state, kLittleEndian);
    int32 version = 0, count = 0;
    CHECK(reader.readInt32(version) && reader.readInt32(count) && count > 0);
    double found = -1.0;
    for (int32 i = 0; i < count; ++i) {
        int32 key = i;
        double value = 0.0;
        if (version == 2) CHECK(reader.readInt32(key));
        CHECK(reader.readDouble(value));
        if (static_cast<ParamID>(key) == id) found = value;
    }
    int32 flag = 0;
    CHECK(reader.readInt32(flag));
    return id == bypass ? (flag ? 1.0 : 0.0) : found;
}

// A host that bypasses an effect stops calling process but still edits its
// parameters, through the controller; the edits must be in the project it saves.
// While blocks run they are the host's parameter changes' job, and the
// controller's copy is left alone.
static void check_idle_edits(IPluginFactory* factory, const TUID effectId, ParamID gain) {
    IPtr<HostApplication> host = owned(new HostApplication);
    IComponent* component = nullptr;
    CHECK(factory->createInstance(effectId, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
    CHECK(component->initialize(host) == kResultOk);
    IAudioProcessor* processor = nullptr;
    CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk && processor);
    TUID controllerId{};
    CHECK(component->getControllerClassId(controllerId) == kResultOk);
    IEditController* controller = nullptr;
    CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
    CHECK(controller->initialize(host) == kResultOk);
    IConnectionPoint* fromComponent = nullptr;
    IConnectionPoint* fromController = nullptr;
    CHECK(component->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromComponent)) == kResultOk);
    CHECK(controller->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromController)) == kResultOk);
    CHECK(fromComponent->connect(fromController) == kResultOk && fromController->connect(fromComponent) == kResultOk);
    const ParamID bypass = check_bypass_parameter(controller);
    SpeakerArrangement arrangement = SpeakerArr::kStereo;
    CHECK(processor->setBusArrangements(&arrangement, 1, &arrangement, 1) == kResultOk);
    CHECK(component->activateBus(kAudio, kInput, 0, true) == kResultOk);
    CHECK(component->activateBus(kAudio, kOutput, 0, true) == kResultOk);
    ProcessSetup setup{kRealtime, kSample32, 512, 48000.0};
    CHECK(processor->setupProcessing(setup) == kResultOk);
    CHECK(component->setActive(true) == kResultOk);
    CHECK(processor->setProcessing(true) == kResultOk);
    render_host(processor, 2, 1, {}, false);

    const double before = saved_value(component, gain, bypass);
    const double other = before > 0.5 ? 0.125 : 0.875;
    // Audio runs: left to the host's parameter changes.
    CHECK(controller->setParamNormalized(gain, other) == kResultOk);
    CHECK(saved_value(component, gain, bypass) == before);
    // Processing off: the processor takes the edit.
    CHECK(processor->setProcessing(false) == kResultOk);
    CHECK(controller->setParamNormalized(gain, other) == kResultOk);
    CHECK(std::fabs(saved_value(component, gain, bypass) - other) < 1e-12);
    // Processing on but no block for longer than the idle time: the same, bypass too.
    CHECK(processor->setProcessing(true) == kResultOk);
    render_host(processor, 2, 1, {}, false);
    std::this_thread::sleep_for(std::chrono::milliseconds(700));
    CHECK(controller->setParamNormalized(gain, before) == kResultOk);
    CHECK(controller->setParamNormalized(bypass, 1.0) == kResultOk);
    CHECK(std::fabs(saved_value(component, gain, bypass) - before) < 1e-12);
    CHECK(saved_value(component, bypass, bypass) == 1.0);
    CHECK(controller->setParamNormalized(bypass, 0.0) == kResultOk);
    CHECK(saved_value(component, bypass, bypass) == 0.0);
    std::puts("VST3 edits while no audio runs: left to the host while blocks run, saved while processing is off or no block came for the idle time (bypass too)");

    CHECK(processor->setProcessing(false) == kResultOk);
    CHECK(component->setActive(false) == kResultOk);
    fromComponent->disconnect(fromController);
    fromController->disconnect(fromComponent);
    fromComponent->release();
    fromController->release();
    processor->release();
    CHECK(controller->terminate() == kResultOk);
    controller->release();
    CHECK(component->terminate() == kResultOk);
    component->release();
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
    // The class ids are the ABI's 16 bytes, read as the FUID string a host stores in its
    // projects: the same string on every system. (On macOS the TUID holds those bytes in
    // order; on Windows it is a COM GUID, whose first three fields are little-endian.)
    const auto same_id = [](const TUID cid, const uint8_t* abi) {
        char got[33] = {};
        char want[33] = {};
        FUID uid = FUID::fromTUID(cid);
        uid.toString(got);
        for (int b = 0; b < 16; ++b) std::snprintf(want + 2 * b, 3, "%02X", static_cast<unsigned>(abi[b]));
        for (int c = 0; c < 32; ++c) {
            if (got[c] >= 'a' && got[c] <= 'f') got[c] = static_cast<char>(got[c] - 'a' + 'A');
        }
        if (std::strcmp(got, want) != 0) std::fprintf(stderr, "class id %s, the ABI says %s\n", got, want);
        return std::strcmp(got, want) == 0;
    };
    CHECK(same_id(effect.cid, info.processor_cid));
    CHECK(same_id(other.cid, info.controller_cid));
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
        check_units(controller);
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
        check_parameter_text(controller, bypass, label);
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
        // The plugin's own state, the blob of op_shell_save_state, rides after the
        // bypass flag: 'OPSB', trailer version 1, length, bytes. A damaged blob is
        // refused and the held state stays as it was. (The chunk round trip is in
        // state.cpp; the example has no chunk of its own.)
        {
            MemoryStream current;
            CHECK(component->getState(&current) == kResultOk);
            const size_t headSize = 8 + 8 * info.param_count + 4;
            const std::string held(current.getData(), static_cast<size_t>(current.getSize()));
            CHECK(held.size() > headSize + 12);
            uint32_t tag = 0;
            std::memcpy(&tag, held.data() + headSize, 4);
            CHECK(tag == 0x4F505342);
            uint32_t blobLength = 0;
            std::memcpy(&blobLength, held.data() + headSize + 8, 4);
            CHECK(held.size() == headSize + 12 + blobLength);
            std::printf("VST3 %s state: %zu bytes, head %zu, plugin blob %u\n", label, held.size(), headSize, blobLength);
            // Each damage is applied to a state with other parameter values.
            std::string different = held;
            const double otherValue = 0.123;
            std::memcpy(&different[8], &otherValue, sizeof(otherValue));
            auto refused = [&](std::string damaged) {
                MemoryStream stream;
                CHECK(stream.write(damaged.data(), static_cast<int32>(damaged.size()), nullptr) == kResultOk);
                stream.seek(0, IBStream::kIBSeekSet, nullptr);
                CHECK(component->setState(&stream) != kResultOk);
                MemoryStream after;
                CHECK(component->getState(&after) == kResultOk);
                CHECK(std::string(after.getData(), static_cast<size_t>(after.getSize())) == held);
            };
            std::string flipped = different;
            flipped[headSize + 12] ^= 0x5A;
            refused(flipped);
            std::string cut = different;
            cut.resize(cut.size() - 1);
            refused(cut);
            std::string extended = different;
            extended.push_back('x');
            refused(extended);
            std::string foreign = different;
            foreign[headSize] ^= 0x01;
            refused(foreign);
            std::string version = different;
            version[headSize + 4] = 2;
            refused(version);
            std::string empty = different.substr(0, headSize + 12);
            std::memset(&empty[headSize + 8], 0, 4);
            refused(empty);
            std::string huge = different;
            huge[headSize + 11] = 0x7F;
            refused(huge);
            std::string nan = different;
            const double notANumber = std::nan("");
            std::memcpy(&nan[8], &notANumber, sizeof(notANumber));
            refused(nan);
            std::printf("VST3 %s state: 8 damaged plugin blobs refused, the held state unchanged\n", label);
            // The state the shell wrote still loads, unchanged.
            MemoryStream again;
            CHECK(again.write(const_cast<char*>(held.data()), static_cast<int32>(held.size()), nullptr) == kResultOk);
            again.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(component->setState(&again) == kResultOk);
        }
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
        // A state from an older build of a plugin without pinned IDs, whose table ended one
        // parameter earlier: the head (version 1) and the plugin's blob (OPST version 1) both
        // hold one value fewer. It loads, and the parameter it does not have takes its default.
        if (info.param_count > 1 && !(op_shell_identity() & OP_IDENTITY_KEYED_STATE)) {
            const uint32_t fewer = info.param_count - 1;
            std::string older;
            const auto put = [&older](const void* data, size_t size) { older.append(static_cast<const char*>(data), size); };
            const int32_t header[2] = {1, static_cast<int32_t>(fewer)};
            put(header, sizeof(header));
            for (uint32_t i = 0; i < fewer; ++i) {
                const double value = op_shell_to_normalized(i, plain[i]);
                put(&value, sizeof(value));
            }
            const int32_t notBypassed = 0;
            put(&notBypassed, sizeof(notBypassed));
            const uint16_t plane[2] = {1, static_cast<uint16_t>(fewer)};
            const uint32_t trailer[3] = {0x4F505342, 1, static_cast<uint32_t>(4 + sizeof(plane) + 4 * fewer)};
            put(trailer, sizeof(trailer));
            put("OPST", 4);
            put(plane, sizeof(plane));
            for (uint32_t i = 0; i < fewer; ++i) put(&plain[i], sizeof(float));
            // First move the last parameter off its default, with a head of this build.
            OpParamDesc last{};
            CHECK(op_shell_param(fewer, &last));
            const double moved = last.default_normalized < 0.5 ? 1.0 : 0.0;
            std::string full = older.substr(0, 8 + 8 * fewer);
            full.append(reinterpret_cast<const char*>(&moved), sizeof(moved));
            const int32_t count_now = static_cast<int32_t>(info.param_count);
            std::memcpy(&full[4], &count_now, 4);
            MemoryStream fullStream;
            CHECK(fullStream.write(full.data(), static_cast<int32>(full.size()), nullptr) == kResultOk);
            fullStream.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(component->setState(&fullStream) == kResultOk);
            fullStream.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(controller->setComponentState(&fullStream) == kResultOk);
            CHECK(controller->getParamNormalized(fewer) == moved);
            MemoryStream stream;
            CHECK(stream.write(older.data(), static_cast<int32>(older.size()), nullptr) == kResultOk);
            stream.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(component->setState(&stream) == kResultOk);
            stream.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(controller->setComponentState(&stream) == kResultOk);
            CHECK(controller->getParamNormalized(fewer) == last.default_normalized);
            MemoryStream now;
            CHECK(component->getState(&now) == kResultOk);
            int32_t count = 0;
            std::memcpy(&count, now.getData() + 4, 4);
            CHECK(count == static_cast<int32_t>(info.param_count));
            for (uint32_t i = 0; i < info.param_count; ++i) {
                double value = -1.0;
                std::memcpy(&value, now.getData() + 8 + 8 * i, 8);
                CHECK(std::fabs(value - (i < fewer ? op_shell_to_normalized(i, plain[i]) : last.default_normalized)) < 1e-6);
            }
            std::printf("VST3 %s state of an older build with %u of %u parameters: loads, the missing one at its default\n", label, fewer, info.param_count);
            // Back to the values the checks below start from.
            legacy.seek(0, IBStream::kIBSeekSet, nullptr);
            CHECK(component->setState(&legacy) == kResultOk);
        }
        // Parameter changes and bypass at their sample offsets.
        check_sample_accurate(component, processor, reference.instance, channels, gain, warm, bypass, 4096);
        CHECK(processor->setProcessing(false) == kResultOk);
        CHECK(component->setActive(false) == kResultOk);
        processor->release();
        CHECK(controller->terminate() == kResultOk);
        controller->release();
        CHECK(component->terminate() == kResultOk);
        component->release();
    }
    check_idle_edits(factory, effect.cid, find_shell_param("Gain"));
    factory->release();
    CHECK(exitFn());
    CFRelease(bundle);
#if OP_HAS_TAIL
    std::puts("VST3 proof: factory and ABI class IDs, parameter metadata, latency, tail, shell-equal audio, in-place buffers, state round-trip, malformed and wrong-count state, bypass (flag, delayed dry output, restore, state, legacy state), sample-accurate parameter changes and bypass, edits while no audio runs: PASS");
#else
    std::puts("VST3 proof: factory and ABI class IDs, parameter metadata, latency, shell-equal audio, in-place buffers, state round-trip, malformed and wrong-count state, bypass, sample-accurate parameter changes and bypass, edits while no audio runs, tail not built (OP_TAIL=OFF): PASS");
#endif
    return 0;
}
