// SPDX-License-Identifier: MIT
// Format parity, AAX: renders the shared scenario through the AAX shell's real
// process callback (openplugin_aax::process), as native/aax/host_process.cpp
// does. This is the shell's callback, not Pro Tools. Local only: it needs the
// AAX SDK, which is not redistributable (docs/FORMAT_PARITY.md).
//
// Contract: AAX posts one controls packet per process callback, so a change
// takes effect on a callback boundary. The host cuts a callback at every
// event frame and posts the packet before it.
//
//   ParityAAX <scenario> <output>
#include "scenario.h"
#include "OpenPluginAAX.h"
#include "op_shell.h"
#include <cmath>

int main(int argc, char** argv) {
    using namespace openplugin_aax;
    PARITY_CHECK(argc == 3);
    const ParityScenario s = parity_read(argv[1]);
    OpShellInfo info{};
    PARITY_CHECK(op_shell_info(&info) != 0 && info.abi == OP_SHELL_ABI);
    int index[2] = {-1, -1};
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        PARITY_CHECK(op_shell_param(i, &desc) != 0);
        for (uint32_t p = 0; p < 2; ++p) if (std::strcmp(desc.name, parity_param_name(p)) == 0) index[p] = static_cast<int>(i);
    }
    PARITY_CHECK(index[0] >= 0 && index[1] >= 0);
    ProcessState state;
    PARITY_CHECK(state.prepare(s.rate, 2));
    PacketBuffer packet(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        op_shell_param(i, &desc);
        packet.plain()[i] = static_cast<float>(desc.default_plain);
    }
    std::vector<float> out[2] = {std::vector<float>(s.frames), std::vector<float>(s.frames)};
    std::vector<float> in[2] = {s.input[0], s.input[1]};
    const auto cuts = parity_cuts(s);
    for (size_t k = 0; k + 1 < cuts.size(); ++k) {
        const uint32_t a = cuts[k], b = cuts[k + 1];
        for (const auto& e : s.events) if (e.frame == a) packet.plain()[index[e.param]] = e.value;
        float* inputs[] = {in[0].data() + a, in[1].data() + a};
        float* outputs[] = {out[0].data() + a, out[1].data() + a};
        int32_t frames = static_cast<int32_t>(b - a);
        ProcessState* statePtr = &state;
        Context context{inputs, outputs, &frames, packet.get(), &statePtr};
        Context* contexts[] = {&context};
        process(contexts, contexts + 1);
    }
    parity_write(argv[2], out);
    return 0;
}
