// SPDX-License-Identifier: MIT
// Proves AAX bypass is the input delayed by the reported latency, crossfaded over
// 10 ms, against a latency stub (proof_stub.cpp: 37 samples, gain 0.5 at its
// default), because the SDK example reports no latency. It calls the real process
// callback, mono and stereo, with odd block sizes (including one longer than the
// 2048-frame internal block), in place and not, switching both ways. A per-sample
// reference model, written independently of the shell's code, gives the expected
// output for every plan. Not a Pro Tools test.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_bypass_proof --output-on-failure
#include "OpenPluginAAX.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <vector>
using namespace openplugin_aax;

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX bypass proof failed: %s\n", message);
        std::exit(1);
    }
}

constexpr uint32_t kLatency = 37;
constexpr double kRate = 48000.0;
constexpr size_t kFade = 480;  // 10 ms at 48 kHz
constexpr float kGain = 0.5f;  // the stub's default

struct Step {
    unsigned frames;
    bool bypass;
};

static std::vector<float> noise(size_t count, uint32_t seed) {
    std::vector<float> out(count);
    uint32_t x = seed;
    for (auto& v : out) {
        x = x * 1664525u + 1013904223u;
        v = 0.5f * (static_cast<float>((x >> 8) & 0xFFFF) / 32768.0f - 1.0f);
    }
    return out;
}

struct Result {
    std::vector<float> out[2];
    std::vector<float> in[2];
    std::vector<bool> bypassAt;  // per sample: the bypass flag of its block
};

static Result run(ProcessState& state, uint32_t channels, const std::vector<Step>& plan, bool inPlace, bool nanPacket = false) {
    OpShellInfo info{};
    require(op_shell_info(&info) != 0, "shell info");
    PacketBuffer packet(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        require(op_shell_param(i, &desc) != 0, "param");
        packet.plain()[i] = static_cast<float>(desc.default_plain);
    }
    if (nanPacket) packet.plain()[0] = std::numeric_limits<float>::quiet_NaN();
    size_t total = 0;
    for (const Step& s : plan) total += s.frames;
    Result r;
    for (uint32_t c = 0; c < channels; ++c) r.in[c] = noise(total, 1234u + 77u * c);
    size_t at = 0;
    for (const Step& step : plan) {
        packet.get()->bypass = step.bypass ? 1u : 0u;
        std::vector<float> a[2], b[2];
        float* input[2] = {nullptr, nullptr};
        float* output[2] = {nullptr, nullptr};
        for (uint32_t c = 0; c < channels; ++c) {
            a[c].assign(r.in[c].begin() + at, r.in[c].begin() + at + step.frames);
            b[c].assign(step.frames, 0.f);
            input[c] = a[c].data();
            output[c] = inPlace ? a[c].data() : b[c].data();
        }
        int32_t frames = static_cast<int32_t>(step.frames);
        auto* statePtr = &state;
        Context context{input, output, &frames, packet.get(), &statePtr};
        Context* contexts[] = {&context};
        process(contexts, contexts + 1);
        for (uint32_t c = 0; c < channels; ++c) r.out[c].insert(r.out[c].end(), output[c], output[c] + step.frames);
        r.bypassAt.insert(r.bypassAt.end(), step.frames, step.bypass);
        at += step.frames;
    }
    return r;
}

// The expected output, sample by sample. `startMix` is the mix before sample 0.
static std::vector<float> expected(const std::vector<float>& in, const std::vector<bool>& bypassAt, float startMix) {
    std::vector<float> out(in.size());
    // The mix moves a fixed step per sample, kept in single precision as the shell keeps it,
    // so the reference follows the same ramp rather than an idealised one.
    const float step = static_cast<float>(1.0 / static_cast<double>(kFade));
    float mix = startMix;
    for (size_t n = 0; n < in.size(); ++n) {
        const float target = bypassAt[n] ? 1.f : 0.f;
        mix = mix < target ? std::min(target, mix + step) : std::max(target, mix - step);
        const float dry = n >= kLatency ? in[n - kLatency] : 0.f;
        const float engine = in[n] * kGain;
        out[n] = mix >= 1.f ? dry : engine + (dry - engine) * mix;
    }
    return out;
}

static double max_diff(const std::vector<float>& a, const std::vector<float>& b) {
    require(a.size() == b.size(), "same length");
    double worst = 0.0;
    for (size_t i = 0; i < a.size(); ++i) worst = std::max(worst, static_cast<double>(std::fabs(a[i] - b[i])));
    return worst;
}

int main() {
    ProcessState probe;
    require(probe.prepare(kRate, 2), "prepare");
    require(probe.latency == kLatency && op_shell_latency(probe.engine) == kLatency, "the stub reports its latency to the process state");
    require(probe.dry[0].size() == kMaxBlock && probe.line[0].size() == kLatency, "bypass buffers are sized in prepare");
    unsigned long samples = 0, checks = 0;

    const std::vector<Step> fromStart = {{1, true},   {7, true},    {33, true},   {100, true},  {511, true}, {512, true},
                                         {513, true}, {2047, true}, {2048, true}, {2049, true}, {3000, true}, {5, true}};
    const std::vector<Step> switching = {{700, false}, {333, true},  {1, true},     {45, true},    {2000, true}, {100, false},
                                         {60, true},   {1000, false}, {2500, false}, {37, true},    {3000, true},  {1, false},
                                         {4096, false}};
    for (uint32_t channels = 1; channels <= 2; ++channels) {
        const char* layout = channels == 1 ? "mono" : "stereo";
        for (bool inPlace : {false, true}) {
            // 1. Bypassed from the first block: the output is the input delayed by the latency, exactly.
            {
                ProcessState state;
                require(state.prepare(kRate, channels), "prepare");
                const Result r = run(state, channels, fromStart, inPlace);
                for (uint32_t c = 0; c < channels; ++c) {
                    for (size_t n = 0; n < r.in[c].size(); ++n) {
                        const float want = n >= kLatency ? r.in[c][n - kLatency] : 0.f;
                        if (r.out[c][n] != want) {
                            std::fprintf(stderr, "AAX bypass proof failed: %s%s, bypassed from the start, channel %u sample %zu is %.9g, wanted %.9g\n",
                                         layout, inPlace ? " in place" : "", c, n, r.out[c][n], want);
                            return 1;
                        }
                    }
                    samples += r.in[c].size();
                }
                ++checks;
            }
            // 2. Switching both ways across odd blocks: every sample against the reference model.
            {
                ProcessState state;
                require(state.prepare(kRate, channels), "prepare");
                const Result r = run(state, channels, switching, inPlace);
                for (uint32_t c = 0; c < channels; ++c) {
                    const auto want = expected(r.in[c], r.bypassAt, 0.f);
                    const double worst = max_diff(r.out[c], want);
                    if (worst > 1e-6) {
                        std::fprintf(stderr, "AAX bypass proof failed: %s%s switching, channel %u differs from the reference by %.3g\n", layout,
                                     inPlace ? " in place" : "", c, worst);
                        return 1;
                    }
                    samples += r.in[c].size();
                    // Facts that do not rely on the model: unbypassed audio is exactly the engine, a bypassed
                    // stretch longer than the fade is exactly the delayed input, and the fade is a ramp.
                    size_t firstOn = 0;
                    while (!r.bypassAt[firstOn]) ++firstOn;
                    for (size_t n = 0; n < firstOn; ++n) require(r.out[c][n] == r.in[c][n] * kGain, "before the switch the output is the engine's");
                    const float atRamp = r.out[c][firstOn + kFade / 2];
                    const float eng = r.in[c][firstOn + kFade / 2] * kGain;
                    const float dry = r.in[c][firstOn + kFade / 2 - kLatency];
                    require(std::fabs(atRamp - eng) > 1e-4f && std::fabs(atRamp - dry) > 1e-4f, "half way through the fade the output is neither signal");
                    require(r.out[c][firstOn + kFade + 2] == r.in[c][firstOn + kFade + 2 - kLatency], "after the fade the output is exactly the delayed input");
                    const size_t end = r.in[c].size() - 1;
                    require(r.bypassAt[end] == false, "the plan ends unbypassed");
                    // The last 4096 samples are unbypassed again: after the fade back they are exactly the engine's.
                    require(r.out[c][end] == r.in[c][end] * kGain, "after switching back the output is exactly the engine's");
                    // Each direction moves the mix by one step at a time: no jump larger than the signal itself.
                    for (size_t n = 1; n < r.out[c].size(); ++n) {
                        const double step = std::fabs(static_cast<double>(r.out[c][n]) - r.out[c][n - 1]);
                        require(step < 1.0, "no discontinuity beyond the signal's own range");
                    }
                }
                ++checks;
            }
            // 3. A refused packet: silent when not bypassed, the dry path when bypassed.
            {
                ProcessState state;
                require(state.prepare(kRate, channels), "prepare");
                const Result silent = run(state, channels, {{256, false}, {256, false}}, inPlace, true);
                for (uint32_t c = 0; c < channels; ++c)
                    for (float v : silent.out[c]) require(v == 0.f, "a non-finite packet silences the unbypassed block");
                const Result dry = run(state, channels, {{300, true}, {1000, true}}, inPlace, true);
                // The state was silenced first (mix at 0), so the fade runs; after it the dry path is exact.
                for (uint32_t c = 0; c < channels; ++c)
                    for (size_t n = kFade + 2; n < dry.out[c].size(); ++n)
                        require(dry.out[c][n] == dry.in[c][n - kLatency] || n < kLatency, "a bypassed refused packet still outputs the delayed input");
                ++checks;
            }
            // 4. Reset clears the delay line: bypassed output starts from zeros again.
            {
                ProcessState state;
                require(state.prepare(kRate, channels), "prepare");
                run(state, channels, {{1000, true}}, inPlace);
                state.reset();
                const Result r = run(state, channels, {{200, true}}, inPlace);
                for (uint32_t c = 0; c < channels; ++c) {
                    for (size_t n = 0; n < kLatency; ++n) require(r.out[c][n] == 0.f, "after reset the delay line starts empty");
                    for (size_t n = kLatency; n < 200; ++n) require(r.out[c][n] == r.in[c][n - kLatency], "after reset the bypass delays the new input");
                }
                ++checks;
            }
        }
    }
    std::printf("AAX bypass proof: latency %u samples, %zu-sample crossfade; %lu checks (mono and stereo, in place and separate buffers), %lu output samples compared; blocks of 1, 7, 33, 100, 511, 512, 513, 2047, 2048, 2049, 3000 frames; switching both ways matches the per-sample reference within 1e-6, bypass is exactly the delayed input, reset empties the line. Not a Pro Tools test: PASS\n",
                kLatency, kFade, checks, samples);
    return 0;
}
