// SPDX-License-Identifier: MIT
// Audio Unit channel layout tags and labels on the SDK's canonical channel order.
//
// The shell ABI lists every bus in the SDK's order (the CLAP surround ids,
// OpBus::roles). An Audio Unit host names its order with an AudioChannelLayout.
// This header maps one onto the other: for a bus and a host layout it finds
// perm[j], the canonical index of host channel j. Pure tables and loops, no
// Audio Unit object, so a proof can call it.
//
// Tags are written as (id << 16) | channels so the file builds against any SDK
// that has the tag; the names are the ones in CoreAudioBaseTypes.h.
#pragma once
#include "op_shell.h"
#include <CoreAudioTypes/CoreAudioTypes.h>
#include <cstdint>
#include <cstring>
#include <vector>

namespace opau {

// SDK role ids, the values in OpBus::roles: the CLAP surround ids, then the SDK-only
// top side (Dolby top middle) and front wide pairs.
enum Role : uint8_t {
    FL = 0, FR = 1, FC = 2, LFE = 3, BL = 4, BR = 5, FLC = 6, FRC = 7, BC = 8,
    SL = 9, SR = 10, TC = 11, TFL = 12, TFC = 13, TFR = 14, TBL = 15, TBC = 16, TBR = 17,
    TSL = 18, TSR = 19, FLW = 20, FRW = 21
};

constexpr AudioChannelLayoutTag makeTag(uint32_t id, uint32_t channels) { return (id << 16) | channels; }
constexpr AudioChannelLayoutTag kTagMono = makeTag(100, 1);
constexpr uint32_t kTagDiscreteId = 147;  // kAudioChannelLayoutTag_DiscreteInOrder | channels
constexpr AudioChannelLayoutTag kTagUseDescriptions = 0;

struct TagDef {
    AudioChannelLayoutTag tag;
    uint8_t count;
    uint8_t roles[16];  // the role of host channel j, in the tag's order
};

// The preferred tag for a channel count comes first. Names and orders from CoreAudioBaseTypes.h.
inline const std::vector<TagDef>& tagTable() {
    static const std::vector<TagDef> table = {
        {makeTag(101, 2), 2, {FL, FR}},                                      // Stereo
        {makeTag(113, 3), 3, {FL, FR, FC}},                                  // MPEG_3_0_A: L R C
        {makeTag(108, 4), 4, {FL, FR, BL, BR}},                              // Quadraphonic: L R Ls Rs
        {makeTag(117, 5), 5, {FL, FR, FC, BL, BR}},                          // MPEG_5_0_A: L R C Ls Rs
        {makeTag(118, 5), 5, {FL, FR, BL, BR, FC}},                          // MPEG_5_0_B (AudioUnit_5_0): L R Ls Rs C
        {makeTag(121, 6), 6, {FL, FR, FC, LFE, BL, BR}},                     // MPEG_5_1_A (AudioUnit_5_1): L R C LFE Ls Rs
        {makeTag(122, 6), 6, {FL, FR, BL, BR, FC, LFE}},                     // MPEG_5_1_B: L R Ls Rs C LFE
        {makeTag(123, 6), 6, {FL, FC, FR, BL, BR, LFE}},                     // MPEG_5_1_C: L C R Ls Rs LFE
        {makeTag(124, 6), 6, {FC, FL, FR, BL, BR, LFE}},                     // MPEG_5_1_D: C L R Ls Rs LFE
        {makeTag(125, 7), 7, {FL, FR, FC, LFE, BL, BR, BC}},                 // MPEG_6_1_A: L R C LFE Ls Rs Cs
        {makeTag(128, 8), 8, {FL, FR, FC, LFE, SL, SR, BL, BR}},             // MPEG_7_1_C (AudioUnit_7_1): L R C LFE Ls Rs Rls Rrs
        {makeTag(195, 10), 10, {FL, FR, FC, LFE, BL, BR, TFL, TFR, TBL, TBR}},  // Atmos_5_1_4: L R C LFE Ls Rs Vhl Vhr Ltr Rtr
        {makeTag(192, 12), 12, {FL, FR, FC, LFE, SL, SR, BL, BR, TFL, TFR, TBL, TBR}},  // Atmos_7_1_4: L R C LFE Ls Rs Rls Rrs Vhl Vhr Ltr Rtr
        {makeTag(202, 12), 12, {FL, FR, BL, BR, SL, SR, FC, LFE, TFL, TFR, TBL, TBR}},  // Logic_Atmos_7_1_4_B: L R Rls Rrs Ls Rs C LFE Vhl Vhr Ltr Rtr
        // The Atmos beds with top middle (Ltm Rtm: the SDK's top side pair) and wide channels.
        {makeTag(194, 8), 8, {FL, FR, FC, LFE, BL, BR, TSL, TSR}},             // Atmos_5_1_2: L R C LFE Ls Rs Ltm Rtm
        {makeTag(196, 10), 10, {FL, FR, FC, LFE, SL, SR, BL, BR, TSL, TSR}},   // Atmos_7_1_2: L R C LFE Ls Rs Rls Rrs Ltm Rtm
        {makeTag(193, 16), 16, {FL, FR, FC, LFE, SL, SR, BL, BR, FLW, FRW, TFL, TFR, TSL, TSR, TBL, TBR}},  // Atmos_9_1_6: L R C LFE Ls Rs Rls Rrs Lw Rw Vhl Vhr Ltm Rtm Ltr Rtr
    };
    return table;
}

// kAudioChannelLabel_* to the roles it can mean, best first. A label with two
// candidates (Ls, Rs) takes the first one the bus has and has not used: a 5.1
// bus has no side channels, so Ls is its back-left; a 7.1 bus has both, so Ls is
// the side and Rls (33) is the back. LeftTopMiddle (49) and LeftTopSurround (60)
// are both the SDK's top side TSL, as in the AAX shell; LeftWide (35) is FLW.
struct LabelDef { uint32_t label; uint8_t roles[2]; };
inline const std::vector<LabelDef>& labelTable() {
    static const std::vector<LabelDef> table = {
        {1, {FL, 0xFF}}, {2, {FR, 0xFF}}, {3, {FC, 0xFF}}, {4, {LFE, 0xFF}},
        {5, {SL, BL}}, {6, {SR, BR}}, {7, {FLC, 0xFF}}, {8, {FRC, 0xFF}}, {9, {BC, 0xFF}},
        {10, {SL, 0xFF}}, {11, {SR, 0xFF}}, {12, {TC, 0xFF}},
        {13, {TFL, 0xFF}}, {14, {TFC, 0xFF}}, {15, {TFR, 0xFF}},
        {16, {TBL, 0xFF}}, {17, {TBC, 0xFF}}, {18, {TBR, 0xFF}},
        {33, {BL, 0xFF}}, {34, {BR, 0xFF}}, {35, {FLW, 0xFF}}, {36, {FRW, 0xFF}},
        {49, {TSL, 0xFF}}, {51, {TSR, 0xFF}},
        {52, {TBL, 0xFF}}, {53, {TBC, 0xFF}}, {54, {TBR, 0xFF}},
        {55, {SL, 0xFF}}, {56, {SR, 0xFF}}, {60, {TSL, 0xFF}}, {61, {TSR, 0xFF}}, {63, {BL, 0xFF}}, {64, {BR, 0xFF}},
    };
    return table;
}

inline bool busHasRoles(const OpBus& bus) {
    return bus.kind != OP_BUS_ABSENT && bus.channels > 0 && bus.channels <= OP_ROLE_SLOTS && bus.roles[0] != OP_ROLE_NONE;
}

// Same channels and the same set of roles, in any order.
inline bool tagFits(const TagDef& def, const OpBus& bus) {
    if (def.count != bus.channels) return false;
    for (uint32_t j = 0; j < def.count; ++j) {
        bool found = false;
        for (uint32_t i = 0; i < bus.channels; ++i) found = found || bus.roles[i] == def.roles[j];
        if (!found) return false;
    }
    return true;
}

inline const TagDef* findTag(AudioChannelLayoutTag tag) {
    for (const TagDef& def : tagTable()) {
        if (def.tag == tag) return &def;
    }
    return nullptr;
}

// The tags a bus accepts, preferred first. A bus with roles takes the table's
// tags whose roles are the bus's, then UseChannelDescriptions; mono takes the mono tag, and a bus without
// roles (discrete, ambisonic) takes DiscreteInOrder.
inline void appendTags(const OpBus& bus, std::vector<AudioChannelLayoutTag>& tags) {
    auto add = [&tags](AudioChannelLayoutTag tag) {
        for (AudioChannelLayoutTag have : tags) if (have == tag) return;
        tags.push_back(tag);
    };
    if (bus.kind == OP_BUS_ABSENT || bus.channels == 0) return;
    if (busHasRoles(bus)) {
        for (const TagDef& def : tagTable()) if (tagFits(def, bus)) add(def.tag);
        // The SDK accepts a layout with channel descriptions only if the list has this entry.
        // Only surround buses offer it: a mono or stereo bus has one layout, and auval reads
        // this entry on a stereo bus as a promise to take every two-channel layout (Binaural,
        // MatrixStereo, ...), which the shell would then have to refuse.
        if (bus.channels > 2) add(kTagUseDescriptions);
    } else if (bus.channels == 1) {
        add(kTagMono);
    } else {
        add(makeTag(kTagDiscreteId, bus.channels));
    }
}

// Fills perm[0..channels): perm[j] is the canonical index of host channel j.
// `host` is the layout the host set on the element, or null if it set none (the
// bus's preferred tag is then assumed). Returns false when the host's layout
// cannot be put on the bus: an unknown tag, other roles, a label the bus lacks.
inline bool planBus(const OpBus& bus, const AudioChannelLayout* host, uint32_t channels, uint8_t* perm) {
    if (bus.kind == OP_BUS_ABSENT || bus.channels != channels || channels > OP_MAX_CHANNELS) return false;
    for (uint32_t j = 0; j < channels; ++j) perm[j] = static_cast<uint8_t>(j);
    if (!busHasRoles(bus)) {
        // Any layout of the right size labels a bus that has no speaker positions; the
        // base class has already checked the host's channel count.
        return true;
    }
    const TagDef* def = nullptr;
    if (!host) {
        for (const TagDef& candidate : tagTable()) {
            if (tagFits(candidate, bus)) { def = &candidate; break; }
        }
        if (!def) return false;
    } else if (host->mChannelLayoutTag == kTagUseDescriptions) {
        if (host->mNumberChannelDescriptions != channels) return false;
        bool used[OP_ROLE_SLOTS] = {};
        for (uint32_t j = 0; j < channels; ++j) {
            const uint32_t label = host->mChannelDescriptions[j].mChannelLabel;
            const LabelDef* known = nullptr;
            for (const LabelDef& entry : labelTable()) if (entry.label == label) known = &entry;
            if (!known) return false;
            int chosen = -1;
            for (uint8_t role : known->roles) {
                if (role == 0xFF) continue;
                for (uint32_t i = 0; i < channels && chosen < 0; ++i) {
                    if (bus.roles[i] == role && !used[i]) chosen = static_cast<int>(i);
                }
                if (chosen >= 0) break;
            }
            if (chosen < 0) return false;
            used[chosen] = true;
            perm[j] = static_cast<uint8_t>(chosen);
        }
        return true;
    } else {
        def = findTag(host->mChannelLayoutTag);
        if (!def || !tagFits(*def, bus)) return false;
    }
    for (uint32_t j = 0; j < channels; ++j) {
        for (uint32_t i = 0; i < channels; ++i) {
            if (bus.roles[i] == def->roles[j]) perm[j] = static_cast<uint8_t>(i);
        }
    }
    return true;
}

}  // namespace opau
