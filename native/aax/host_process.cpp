// SPDX-License-Identifier: MIT
// Calls the AAX process callback directly over the SDK example shell. This is
// not a Pro Tools host test. Expected parameter count, defaults, latency and
// audio come from the op_shell_* C ABI.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_process_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "proof_shell.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <string>
#include <vector>
static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX proof failed: %s\n", message);
        std::exit(1);
    }
}
static uint32_t find_param(const std::vector<OpParamDesc>& descs, const char* name) {
    for (uint32_t i = 0; i < descs.size(); ++i) {
        if (std::strcmp(descs[i].name, name) == 0) return i;
    }
    std::fprintf(stderr, "AAX proof: the shell has no parameter named %s\n", name);
    std::exit(1);
}
struct Rendered {
    std::vector<float> left;
    std::vector<float> right;
    std::vector<float> dry;
};
static Rendered render(openplugin_aax::ProcessState& state, openplugin_aax::Packet& packet, int blocks) {
    constexpr int kFrames = 512;
    std::vector<float> left(kFrames), right(kFrames), outL(kFrames), outR(kFrames);
    float* input[] = {left.data(), right.data()};
    float* output[] = {outL.data(), outR.data()};
    int32_t frames = kFrames;
    auto* statePtr = &state;
    openplugin_aax::Context context{input, output, &frames, &packet, &statePtr};
    openplugin_aax::Context* contexts[] = {&context};
    Rendered result;
    for (int block = 0; block < blocks; ++block) {
        fill_sine(left.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        fill_sine(right.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        result.dry = left;
        openplugin_aax::process(contexts, contexts + 1);
        for (float sample : outL) require(std::isfinite(sample), "finite left output");
        for (float sample : outR) require(std::isfinite(sample), "finite right output");
        if (block + 1 == blocks) {
            result.left = outL;
            result.right = outR;
        }
    }
    return result;
}
static void require_silence(const std::vector<float>& samples, const char* what) {
    for (float sample : samples) require(sample == 0.0f, what);
}
static void require_exact(const std::vector<float>& a, const std::vector<float>& b, const char* what) {
    require(a.size() == b.size(), what);
    for (size_t i = 0; i < a.size(); ++i) require(a[i] == b[i], what);
}
int main() {
    using namespace openplugin_aax;
    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.abi == OP_SHELL_ABI && info.param_count > 0, "shell info");
    require(info.param_count <= kMaxParams, "parameter count fits an AAX packet");
    std::vector<OpParamDesc> descs(info.param_count);
    std::vector<std::string> ids(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        require(op_shell_param(i, &descs[i]) != 0, "shell param");
        require(descs[i].id[0] != '\0', "parameter id");
        ids[i] = descs[i].id;
        for (uint32_t earlier = 0; earlier < i; ++earlier) require(ids[earlier] != ids[i], "unique parameter id");
    }
    require(info.plugin_id_mono != info.plugin_id_stereo && info.manufacturer_id != 0, "development AAX ids");
    const uint32_t gain = find_param(descs, "Gain");
    const uint32_t warm = find_param(descs, "Warm");
    ProcessState bad;
    require(!bad.prepare(0.0, 2) && !bad.prepare(48000.0, 3) && !bad.prepare(std::numeric_limits<double>::quiet_NaN(), 2), "rejected rate or channel count");
    ProcessState mono;
    ProcessState stereo;
    require(mono.prepare(48000.0, 1) && stereo.prepare(48000.0, 2), "mono and stereo prepare");
    require(mono.paramCount == info.param_count && stereo.paramCount == info.param_count, "ABI parameter count reaches the process state");
    ReferenceShell stereoReference(2);
    ReferenceShell monoReference(1);
    require(stereoReference.instance && monoReference.instance, "reference shell instance");
    require(op_shell_latency(stereo.engine) == op_shell_latency(stereoReference.instance), "stereo latency matches the ABI");
    require(op_shell_latency(mono.engine) == op_shell_latency(monoReference.instance), "mono latency matches the ABI");
    std::printf("AAX latency: process state %u samples, ABI %u; AAX publishes it in EffectInit\n",
                op_shell_latency(stereo.engine), op_shell_latency(stereoReference.instance));
    Packet packet{};
    packet.count = info.param_count;
    std::vector<float> defaults(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        packet.plain[i] = static_cast<float>(descs[i].default_plain);
        defaults[i] = packet.plain[i];
    }
    const Rendered stereoDefault = render(stereo, packet, 4);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        require(std::fabs(op_shell_get_plain(stereo.engine, i) - static_cast<float>(descs[i].default_plain)) < 1e-6, "default plain value reaches the engine");
    }
    op_shell_reset(stereoReference.instance);
    const auto stereoShellDefault = render_shell(stereoReference.instance, 2, 4, defaults);
    std::printf("AAX stereo default output against the shell: max difference %.3g\n", max_difference(stereoShellDefault, stereoDefault.left));
    require(max_difference(stereoShellDefault, stereoDefault.left) <= 1e-6, "default output matches the shell");
    require(max_difference(stereoDefault.left, stereoDefault.right) <= 1e-6, "stereo channels match");
    const double testGain = std::clamp(6.0, descs[gain].min, descs[gain].max);
    packet.plain[gain] = static_cast<float>(testGain);
    packet.plain[warm] = 0.0f;
    std::vector<float> plain = defaults;
    plain[gain] = static_cast<float>(testGain);
    plain[warm] = 0.0f;
    op_shell_reset(stereoReference.instance);
    const auto stereoShellGain = render_shell(stereoReference.instance, 2, 8, plain);
    const Rendered stereoGain = render(stereo, packet, 8);
    std::printf("AAX stereo gain %.2f dB against the shell: max difference %.3g\n", testGain, max_difference(stereoShellGain, stereoGain.left));
    require(max_difference(stereoShellGain, stereoGain.left) <= 1e-6, "gain output matches the shell");
    const double measured = gain_db(stereoGain.dry.data(), stereoGain.left.data(), 128, 256);
    std::printf("AAX stereo measured gain: %.3f dB\n", measured);
    require(std::fabs(measured - testGain) < 0.05, "measured gain");
    op_shell_reset(monoReference.instance);
    const auto monoShellGain = render_shell(monoReference.instance, 1, 8, plain);
    const Rendered monoGain = render(mono, packet, 8);
    std::printf("AAX mono gain %.2f dB against the shell: max difference %.3g\n", testGain, max_difference(monoShellGain, monoGain.left));
    require(max_difference(monoShellGain, monoGain.left) <= 1e-6, "mono gain output matches the shell");
    packet.plain[warm] = 1.0f;
    std::vector<float> warmPlain = plain;
    warmPlain[warm] = 1.0f;
    op_shell_reset(stereoReference.instance);
    const auto stereoShellWarm = render_shell(stereoReference.instance, 2, 8, warmPlain);
    const Rendered stereoWarm = render(stereo, packet, 8);
    std::printf("AAX stereo warm output against the shell: max difference %.3g\n", max_difference(stereoShellWarm, stereoWarm.left));
    require(max_difference(stereoShellWarm, stereoWarm.left) <= 1e-6, "warm output matches the shell");
    require(max_difference(stereoGain.left, stereoWarm.left) > 0.01, "warm changes the output");
    packet.bypass = 1;
    const Rendered bypassed = render(stereo, packet, 2);
    require_exact(bypassed.left, bypassed.dry, "bypass copies the dry left buffer");
    require_exact(bypassed.right, bypassed.dry, "bypass copies the dry right buffer");
    packet.bypass = 0;
    packet.plain[gain] = std::numeric_limits<float>::quiet_NaN();
    const Rendered nonFinite = render(stereo, packet, 2);
    require_silence(nonFinite.left, "non-finite packet writes silence");
    require_silence(nonFinite.right, "non-finite packet writes silence on the right");
    packet.plain[gain] = static_cast<float>(testGain);
    packet.count = kMaxParams + 1;
    const Rendered oversized = render(stereo, packet, 2);
    require_silence(oversized.left, "oversized packet writes silence");
    packet.count = info.param_count;
    std::puts("AAX proof: prepare, ABI parameter count and defaults, mono/stereo output against the shell, dry-copy bypass, non-finite and oversized packets. State is checked by aax_state_proof; tail and host latency reporting are format gaps documented in docs/SHELL.md. Not a Pro Tools test: PASS");
    return 0;
}
