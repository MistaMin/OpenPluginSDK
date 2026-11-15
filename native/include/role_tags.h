// SPDX-License-Identifier: MIT
// The tags of the channel order proof (0.6 task 8), shared by the VST3, Audio Unit
// and AAX role proofs. The plugin is examples/role_tags (Role Tags), linked in place
// of the SDK example; its src/lib.rs defines the same values:
//   - a host writes inTag(code) on every input channel, where code is the SDK role id
//     of the speaker that host channel is (kMonoCode for a mono channel);
//   - the plugin checks each input channel against the role it believes the channel
//     has, and writes outTag(code) of that role, or kMismatch plus the input sample
//     where the input was not the tag it expected.
// So a host channel that comes back with outTag of its own speaker carried the right
// role into the plugin and back out. Test builds only.
#pragma once
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

namespace roletags {
// SDK role ids (openplugin_core::layout::SURROUND_*): the CLAP surround ids, then the
// SDK-only top side and wide pairs.
enum Role : uint8_t {
    FL = 0, FR = 1, FC = 2, LFE = 3, BL = 4, BR = 5, FLC = 6, FRC = 7, BC = 8, SL = 9, SR = 10,
    TC = 11, TFL = 12, TFC = 13, TFR = 14, TBL = 15, TBC = 16, TBR = 17, TSL = 18, TSR = 19, FLW = 20, FRW = 21,
};
constexpr uint8_t kMonoCode = 22;    // SURROUND_ROLE_COUNT: a channel with no role
constexpr uint8_t kUnknown = 0xFF;   // a host speaker with no SDK role
constexpr float kMismatch = 2.0f;

inline float inTag(uint8_t code) { return (static_cast<float>(code) + 1.0f) / 32.0f; }
inline float outTag(uint8_t code) { return -inTag(code); }

inline const char* name(uint8_t code) {
    static const char* const names[] = {"FL", "FR", "FC", "LFE", "BL", "BR", "FLC", "FRC", "BC", "SL", "SR", "TC",
                                        "TFL", "TFC", "TFR", "TBL", "TBC", "TBR", "TSL", "TSR", "FLW", "FRW", "M"};
    return code <= kMonoCode ? names[code] : "?";
}

// Checks host output channel `channel`, whose speaker is `code`, over `frames` samples.
// Prints what is wrong and returns false.
inline bool checkChannel(const char* format, const char* layout, uint32_t channel, uint8_t code, const float* samples, uint32_t frames) {
    for (uint32_t n = 0; n < frames; ++n) {
        const float v = samples[n];
        if (v >= kMismatch - 1.0f) {
            std::fprintf(stderr, "%s %s: host channel %u (%s) sample %u: the plugin expected another input tag, got %g\n", format, layout,
                         channel, name(code), n, static_cast<double>(v - kMismatch));
            return false;
        }
        if (std::fabs(v - outTag(code)) > 1e-6f) {
            std::fprintf(stderr, "%s %s: host channel %u (%s) sample %u holds %g, not the output tag of %s\n", format, layout, channel,
                         name(code), n, static_cast<double>(v), name(code));
            return false;
        }
    }
    return true;
}

// True when every sample of every channel is flagged as a wrong input tag.
inline bool allFlagged(const std::vector<std::vector<float>>& out) {
    for (const auto& channel : out)
        for (float v : channel)
            if (v < kMismatch - 1.0f) return false;
    return true;
}

inline std::string codes(const std::vector<uint8_t>& speakers) {
    std::string text;
    for (uint8_t c : speakers) {
        if (!text.empty()) text += ' ';
        text += name(c);
    }
    return text;
}
}  // namespace roletags
