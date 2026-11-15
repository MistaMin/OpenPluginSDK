#include <cstdint>
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
static Rendered render(openplugin_aax::ProcessState& state, openplugin_aax::PacketBuffer& packet, int blocks) {
    constexpr int kFrames = 512;
    std::vector<float> left(kFrames), right(kFrames), outL(kFrames), outR(kFrames);
    float* input[] = {left.data(), right.data()};
    float* output[] = {outL.data(), outR.data()};
    int32_t frames = kFrames;
    auto* statePtr = &state;
    openplugin_aax::Context context{input, output, &frames, packet.get(), &statePtr};
    // Pro Tools leaves a context field that Describe did not register undefined, not null
    // (found in Pro Tools 2025.10: a plugin without MIDI faulted on its MIDI input field).
    // Fill each field this plugin does not register with an address that faults when read,
    // so the shell is proven to leave them alone.
    const uint32_t notePorts = op_shell_note_ports();
    if (!(notePorts & OP_NOTE_PORT_INPUT)) context.midiIn = reinterpret_cast<AAX_IMIDINode*>(uintptr_t{0x8});
    if (!(notePorts & OP_NOTE_PORT_OUTPUT)) context.midiOut = reinterpret_cast<AAX_IMIDINode*>(uintptr_t{0x18});
    if (!state.sidechain) context.sideChain = reinterpret_cast<const int32_t*>(uintptr_t{0x28});
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
    require(info.param_count <= kMaxParams, "parameter count fits the shell bound");
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
    PacketBuffer packet(info.param_count);
    std::vector<float> defaults(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        packet.plain()[i] = static_cast<float>(descs[i].default_plain);
        defaults[i] = packet.plain()[i];
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
    packet.plain()[gain] = static_cast<float>(testGain);
    packet.plain()[warm] = 0.0f;
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
    packet.plain()[warm] = 1.0f;
    std::vector<float> warmPlain = plain;
    warmPlain[warm] = 1.0f;
    op_shell_reset(stereoReference.instance);
    const auto stereoShellWarm = render_shell(stereoReference.instance, 2, 8, warmPlain);
    const Rendered stereoWarm = render(stereo, packet, 8);
    std::printf("AAX stereo warm output against the shell: max difference %.3g\n", max_difference(stereoShellWarm, stereoWarm.left));
    require(max_difference(stereoShellWarm, stereoWarm.left) <= 1e-6, "warm output matches the shell");
    require(max_difference(stereoGain.left, stereoWarm.left) > 0.01, "warm changes the output");
    packet.get()->bypass = 1;
    const Rendered bypassed = render(stereo, packet, 2);
    // The example reports no latency, so the delayed dry signal is the input, and after
    // the 10 ms crossfade (480 samples at 48 kHz) the second block is exactly that.
    // aax_bypass_proof checks the delay and the crossfade against a latency stub.
    require(op_shell_latency(stereo.engine) == 0, "the example reports no latency");
    require_exact(bypassed.left, bypassed.dry, "bypass outputs the dry left buffer after the crossfade");
    require_exact(bypassed.right, bypassed.dry, "bypass outputs the dry right buffer after the crossfade");
    packet.get()->bypass = 0;
    packet.plain()[gain] = std::numeric_limits<float>::quiet_NaN();
    const Rendered nonFinite = render(stereo, packet, 2);
    require_silence(nonFinite.left, "non-finite packet writes silence");
    require_silence(nonFinite.right, "non-finite packet writes silence on the right");
    packet.plain()[gain] = static_cast<float>(testGain);
    packet.get()->count = info.param_count + 1;
    const Rendered oversized = render(stereo, packet, 2);
    require_silence(oversized.left, "oversized packet writes silence");
    packet.get()->count = info.param_count;
    std::puts("AAX proof: prepare, ABI parameter count and defaults, mono/stereo output against the shell, bypass after its crossfade, non-finite and oversized packets. State is checked by aax_state_proof; AAX has no tail report (a format limit, docs/SHELL.md) and publishes latency in EffectInit. Not a Pro Tools test: PASS");
    return 0;
}
