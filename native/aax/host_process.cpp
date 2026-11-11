// SPDX-License-Identifier: MIT
// Calls the AAX process callback directly. This is not a Pro Tools host test.
#include "OpenPluginAAX.h"
#include "proof_eq.h"
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
static int find_param(const char* name) {
    OpShellInfo info{};
    require(op_shell_info(&info), "shell info");
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        require(op_shell_param(i, &desc), "shell param");
        if (std::strcmp(desc.name, name) == 0) return static_cast<int>(i);
    }
    std::fprintf(stderr, "AAX proof missing parameter %s\n", name);
    std::exit(1);
}
static void set_band(openplugin_aax::Packet& packet, int band, float type, float gain) {
    char typeName[32];
    char gainName[32];
    std::snprintf(typeName, sizeof(typeName), "Band %d Type", band);
    std::snprintf(gainName, sizeof(gainName), "Band %d Gain", band);
    packet.plain[find_param(typeName)] = type;
    packet.plain[find_param(gainName)] = gain;
}
static double render(openplugin_aax::ProcessState& state, openplugin_aax::Packet& packet, int blocks) {
    constexpr int kFrames = 512;
    std::vector<float> left(kFrames), right(kFrames), outL(kFrames), outR(kFrames), dry(kFrames);
    float* input[] = {left.data(), right.data()};
    float* output[] = {outL.data(), outR.data()};
    int32_t frames = kFrames;
    auto* statePtr = &state;
    openplugin_aax::Context context{input, output, &frames, &packet, &statePtr};
    openplugin_aax::Context* contexts[] = {&context};
    double measured = 0.0;
    for (int block = 0; block < blocks; ++block) {
        fill_sine(left.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        fill_sine(right.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        dry = left;
        openplugin_aax::process(contexts, contexts + 1);
        for (float sample : outL) require(std::isfinite(sample), "finite output");
        if (block + 1 == blocks) measured = gain_db(dry.data(), outL.data(), 256, 256);
    }
    return measured;
}
int main() {
    using namespace openplugin_aax;
    ProcessState bad;
    require(!bad.prepare(0.0, 2) && !bad.prepare(48000.0, 3), "rejected rate or channel count");
    ProcessState mono;
    ProcessState stereo;
    require(mono.prepare(48000.0, 1) && stereo.prepare(48000.0, 2), "mono and stereo prepare");
    require(op_shell_latency(stereo.engine) == 0, "minimum-phase latency");
    OpShellInfo info{};
    require(op_shell_info(&info) && info.param_count == 26 && info.param_count <= kMaxParams, "FreEQ parameter count");
    require(info.plugin_id_mono != info.plugin_id_stereo && info.manufacturer_id != 0, "development AAX ids");
    Packet packet{};
    packet.count = info.param_count;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        require(op_shell_param(i, &desc), "default");
        packet.plain[i] = static_cast<float>(desc.default_plain);
    }
    for (int band = 1; band <= 6; ++band) set_band(packet, band, 0.f, 0.f);
    packet.plain[find_param("Phase")] = 0.f;
    packet.plain[find_param("Trim")] = 0.f;
    const double unity = render(stereo, packet, 4);
    require(std::fabs(unity) < 0.05, "unity peaks");
    set_band(packet, 4, 0.f, 6.f);
    packet.plain[find_param("Band 4 Freq")] = 1000.f;
    packet.plain[find_param("Band 4 Q")] = 1.f;
    const double bell = render(stereo, packet, 12);
    std::printf("AAX stereo Band 4 at 1 kHz: %.2f dB\n", bell);
    require(std::fabs(bell - 6.0) < 0.2, "+6 dB bell");
    const double monoBell = render(mono, packet, 12);
    std::printf("AAX mono Band 4 at 1 kHz: %.2f dB\n", monoBell);
    require(std::fabs(monoBell - 6.0) < 0.2, "mono +6 dB");
    packet.bypass = 1;
    const double bypassed = render(stereo, packet, 2);
    require(std::fabs(bypassed) < 0.05, "bypass is dry");
    packet.bypass = 0;
    packet.plain[find_param("Trim")] = std::numeric_limits<float>::quiet_NaN();
    constexpr int kFrames = 32;
    std::vector<float> left(kFrames, 0.2f), outL(kFrames, 1.f), right(kFrames, 0.2f), outR(kFrames, 1.f);
    float* input[] = {left.data(), right.data()};
    float* output[] = {outL.data(), outR.data()};
    int32_t frames = kFrames;
    auto* statePtr = &stereo;
    Context context{input, output, &frames, &packet, &statePtr};
    Context* contexts[] = {&context};
    process(contexts, contexts + 1);
    require(outL[0] == 0.f && outR[0] == 0.f, "non-finite packet silenced");
    std::puts("AAX proof: prepare, mono/stereo, unity, +6 dB, dry bypass, malformed packet. Not a Pro Tools test: PASS");
    return 0;
}
