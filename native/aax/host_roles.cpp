// SPDX-License-Identifier: MIT
// The channel order proof (0.6 task 8) through the AAX shell. The plugin is Role Tags
// (examples/role_tags), linked into this binary, which declares every SDK layout from
// mono to 9.1.6 and checks every input channel against the role it believes the
// channel has (see native/include/role_tags.h). The host takes the components the
// shell describes (buildConfigs), prepares each stem pair as Pro Tools would, and
// calls the real algorithm callback with its channels in Pro Tools' film order, which
// it knows from its own table (the comments on AAX_EStemFormat in AAX_Enums.h). Every
// output channel must come back with the plugin's tag of the same speaker. Not a Pro
// Tools test.
//
//   ctest --test-dir build-shell -R aax_roles_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "role_tags.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

using namespace openplugin_aax;
using namespace roletags;

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "AAX roles proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)
constexpr int32_t kFrames = 64;

namespace {
// Pro Tools' channel order of each stem, as SDK roles. Ls Rs of 5.x are the SDK's back
// pair; Lss Rss the sides, Lsr Rsr the backs; Ltf Rtf, Ltr Rtr the top front and top
// rear rows; Ltm Rtm (top middle) and Lts Rts (top side) the SDK's top side pair; Lw Rw
// the wides.
struct HostStem {
    AAX_EStemFormat format;
    const char* name;
    std::vector<uint8_t> speakers;
};
const std::vector<HostStem>& hostStems() {
    static const std::vector<HostStem> stems = {
        {AAX_eStemFormat_Mono, "mono", {kMonoCode}},
        {AAX_eStemFormat_Stereo, "stereo", {FL, FR}},
        {AAX_eStemFormat_LCR, "LCR", {FL, FC, FR}},
        {AAX_eStemFormat_Quad, "quad", {FL, FR, BL, BR}},
        {AAX_eStemFormat_5_0, "5.0", {FL, FC, FR, BL, BR}},
        {AAX_eStemFormat_5_1, "5.1", {FL, FC, FR, BL, BR, LFE}},
        {AAX_eStemFormat_6_1, "6.1", {FL, FC, FR, BL, BC, BR, LFE}},
        {AAX_eStemFormat_7_1_DTS, "7.1", {FL, FC, FR, SL, SR, BL, BR, LFE}},
        {AAX_eStemFormat_7_1_4, "7.1.4", {FL, FC, FR, SL, SR, BL, BR, LFE, TFL, TFR, TBL, TBR}},
        {AAX_eStemFormat_5_1_2, "5.1.2", {FL, FC, FR, BL, BR, LFE, TSL, TSR}},
        {AAX_eStemFormat_5_1_4, "5.1.4", {FL, FC, FR, BL, BR, LFE, TFL, TFR, TBL, TBR}},
        {AAX_eStemFormat_7_1_2, "7.1.2", {FL, FC, FR, SL, SR, BL, BR, LFE, TSL, TSR}},
        {AAX_eStemFormat_9_1_6, "9.1.6", {FL, FC, FR, FLW, FRW, SL, SR, BL, BR, LFE, TFL, TFR, TSL, TSR, TBL, TBR}},
    };
    return stems;
}

using Audio = std::vector<std::vector<float>>;

// One callback over the prepared state, `speakers` the role of each host channel.
Audio run(ProcessState& state, uint32_t params, const std::vector<uint8_t>& speakers) {
    PacketBuffer packet(params);
    Audio in, out(speakers.size(), std::vector<float>(kFrames, 7.0f));
    for (uint8_t code : speakers) in.push_back(std::vector<float>(kFrames, inTag(code)));
    std::vector<float*> inPtr, outPtr;
    for (auto& c : in) inPtr.push_back(c.data());
    for (auto& c : out) outPtr.push_back(c.data());
    int32_t frames = kFrames;
    ProcessState* statePtr = &state;
    Context context{inPtr.data(), outPtr.data(), &frames, packet.get(), &statePtr, nullptr, nullptr, nullptr, nullptr};
    Context* contexts[] = {&context};
    openplugin_aax::process(contexts, contexts + 1);
    return out;
}
}  // namespace

int main() {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) && info.param_count > 0);
    const std::vector<Config> configs = buildConfigs(info);

    // Every layout of the plugin has a component, with the same stem in and out.
    CHECK(op_shell_layout_count() == hostStems().size());
    for (uint32_t i = 0; i < op_shell_layout_count(); ++i) {
        const bool offered = std::any_of(configs.begin(), configs.end(), [&](const Config& c) { return c.layout == static_cast<int32_t>(i) && c.input == c.output; });
        if (!offered) {
            OpLayout layout{};
            op_shell_layout(i, &layout);
            std::fprintf(stderr, "AAX roles: layout %u (%s) has no component\n", i, layout.name);
        }
        CHECK(offered);
    }

    int ran = 0;
    for (const HostStem& stem : hostStems()) {
        const bool described = std::any_of(configs.begin(), configs.end(), [&](const Config& c) { return c.input == stem.format && c.output == stem.format; });
        CHECK(described);
        ProcessState state;
        CHECK(state.prepareStems(48000.0, stem.format, stem.format));
        CHECK(state.in.aax == stem.speakers.size() && state.out.aax == stem.speakers.size());
        const Audio out = run(state, info.param_count, stem.speakers);
        for (size_t j = 0; j < out.size(); ++j) CHECK(checkChannel("AAX", stem.name, static_cast<uint32_t>(j), stem.speakers[j], out[j].data(), kFrames));
        std::printf("AAX roles: %-6s %2zu channels, Pro Tools order %s: every role at its index in and out\n", stem.name, stem.speakers.size(), codes(stem.speakers).c_str());
        ++ran;
    }
    CHECK(ran == 13);

    // The negative control: the 9.1.6 tags one channel off are caught.
    {
        const HostStem& wide = hostStems().back();
        ProcessState state;
        CHECK(state.prepareStems(48000.0, wide.format, wide.format));
        std::vector<uint8_t> skewed = wide.speakers;
        std::rotate(skewed.begin(), skewed.begin() + 1, skewed.end());
        CHECK(allFlagged(run(state, info.param_count, skewed)));
        std::puts("AAX roles: 9.1.6 with the tags one channel off: every channel flagged (negative control)");
    }
    // Stems with no layout of the plugin are not prepared.
    {
        ProcessState state;
        CHECK(!state.prepareStems(48000.0, AAX_eStemFormat_7_0_2, AAX_eStemFormat_7_0_2));
        CHECK(!state.prepareStems(48000.0, AAX_eStemFormat_9_1_4, AAX_eStemFormat_9_1_4));
        CHECK(!state.prepareStems(48000.0, AAX_eStemFormat_5_1, AAX_eStemFormat_7_1_DTS));
        std::puts("AAX roles: 7.0.2, 9.1.4 and 5.1 in 7.1 out (no layout of the plugin) are refused");
    }
    std::puts("AAX roles proof: mono, stereo, LCR, quad, 5.0, 5.1, 6.1, 7.1, 7.1.4, 5.1.2, 5.1.4, 7.1.2, 9.1.6; skipped: none: PASS");
    return 0;
}
