// SPDX-License-Identifier: MIT
// Thin VST3 adapter. The plugin is the Rust op_shell_* ABI. This file does not
// contain DSP. With OP_HAS_EDITOR the controller hosts the plugin's op_editor_* view;
// without it a host shows its generic controls.
#include "op_shell.h"
#if OP_HAS_EDITOR
// The host's parameter menu and track reach the editor (op_editor.h).
#ifndef OP_EDITOR_MENU
#define OP_EDITOR_MENU 1
#endif
#include "op_editor.h"
#include "public.sdk/source/common/pluginview.h"
#include "pluginterfaces/vst/ivstmessage.h"
#endif
#include "public.sdk/source/main/pluginfactory.h"
#include "public.sdk/source/vst/vstaudioeffect.h"
#include "public.sdk/source/vst/vsteditcontroller.h"
#include "public.sdk/source/vst/vstparameters.h"
#include "pluginterfaces/base/ustring.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstchannelcontextinfo.h"
#include "pluginterfaces/vst/ivstcontextmenu.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include "pluginterfaces/vst/ivstmidicontrollers.h"
#include "pluginterfaces/vst/ivstnoteexpression.h"
#include "pluginterfaces/vst/ivstphysicalui.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/ivstunits.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include "base/source/fstreamer.h"
#include "base/source/timer.h"
#include "pluginterfaces/base/ibstream.h"
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cctype>
#include <cstring>
#include <memory>
#include <string>
#include <string>
#include <vector>

using namespace Steinberg;
using namespace Steinberg::Vst;

namespace openplugin_vst {
// The library must be at least the ABI this shell was written for (see
// docs/SHELL.md, Versioning): a newer library only adds. Checked before any
// ABI 2 symbol is called, so the shell never runs on an older library.
bool load_info(OpShellInfo& info) {
    std::memset(&info, 0, sizeof(info));
    if (op_shell_abi_version() < OP_SHELL_ABI) return false;
    return op_shell_info(&info) && info.abi >= OP_SHELL_ABI && info.param_count > 0;
}

// ---- Channel order at the edge ---------------------------------------------
// The ABI lists channels in the SDK's canonical order (the CLAP surround ids,
// then the SDK-only top side and wide roles 18 to 21). A VST3 SpeakerArrangement
// is a bit set whose channels sit in ascending bit order in the host's buffers.
// Bits 0..17 (L R C Lfe Ls Rs Lc Rc Cs Sl Sr Tc Tfl Tfc Tfr Trl Trc Trr) are
// numbered exactly like the CLAP ids, so the speaker of roles 0..17 is the bit of
// the same number; the top side pair is kSpeakerTsl and kSpeakerTsr (bits 24, 25)
// and the wides kSpeakerLw and kSpeakerRw (bits 59, 60). The host index of a role
// is the number of lower bits set. Mono is kSpeakerM, ambisonics are the ACN
// speakers.
constexpr uint32_t kLegacyLayout = ~0u;  // not an ABI layout: create() channels only
constexpr uint32_t kMaxAmbisonicChannels = 25;  // VST3 names ACN 0..24.

// The VST3 speaker of an SDK role; 0 for a role VST3 cannot name.
constexpr Speaker speaker_of(uint8_t role) {
    if (role <= 17) return static_cast<Speaker>(1) << role;
    switch (role) {
    case 18: return kSpeakerTsl;  // TSL, top side (Dolby top middle)
    case 19: return kSpeakerTsr;  // TSR
    case 20: return kSpeakerLw;   // FLW, front wide
    case 21: return kSpeakerRw;   // FRW
    default: return 0;
    }
}

// The canonical role lists of SpeakerLayout::surround_map (docs/SHELL.md, Canonical
// order) give the SDK's named VST3 arrangements, so a host that offers one of these
// is offered the plugin's layout. 7.1 is k71Music (Ls Rs are the back pair, Sl Sr the
// sides); 5.1.2 is the top side k51_2_TS, not k51_2 (front heights Tfl Tfr); 9.1.6 is
// Dolby's k91_6_W with wides, not k91_6 (Lc Rc).
template <size_t N> constexpr SpeakerArrangement arrangement_of_roles(const uint8_t (&roles)[N]) {
    SpeakerArrangement out = 0;
    for (size_t i = 0; i < N; ++i) out |= speaker_of(roles[i]);
    return out;
}
constexpr uint8_t kRoles51[] = {0, 1, 2, 3, 4, 5};
constexpr uint8_t kRoles71[] = {0, 1, 2, 3, 9, 10, 4, 5};
constexpr uint8_t kRoles714[] = {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17};
constexpr uint8_t kRoles512[] = {0, 1, 2, 3, 4, 5, 18, 19};
constexpr uint8_t kRoles514[] = {0, 1, 2, 3, 4, 5, 12, 14, 15, 17};
constexpr uint8_t kRoles712[] = {0, 1, 2, 3, 9, 10, 4, 5, 18, 19};
constexpr uint8_t kRoles916[] = {0, 1, 2, 3, 9, 10, 4, 5, 20, 21, 12, 14, 18, 19, 15, 17};
static_assert(arrangement_of_roles(kRoles51) == SpeakerArr::k51, "5.1");
static_assert(arrangement_of_roles(kRoles71) == SpeakerArr::k71Music, "7.1");
static_assert(arrangement_of_roles(kRoles714) == SpeakerArr::k71_4, "7.1.4");
static_assert(arrangement_of_roles(kRoles512) == SpeakerArr::k51_2_TS, "5.1.2");
static_assert(arrangement_of_roles(kRoles514) == SpeakerArr::k51_4, "5.1.4");
static_assert(arrangement_of_roles(kRoles712) == SpeakerArr::k71_2, "7.1.2");
static_assert(arrangement_of_roles(kRoles916) == SpeakerArr::k91_6_W, "9.1.6");

SpeakerArrangement acn_bit(uint32_t k) { return static_cast<SpeakerArrangement>(1) << (k < 4 ? 20 + k : 34 + k); }

// The arrangement that carries `bus`; false when VST3 cannot name it (discrete
// buses, a repeated or unnamed role, ambisonics beyond 4th order).
bool arrangement_of(const OpBus& bus, SpeakerArrangement& out) {
    out = SpeakerArr::kEmpty;
    switch (bus.kind) {
    case OP_BUS_ABSENT: return bus.channels == 0;
    case OP_BUS_MONO: out = SpeakerArr::kMono; return bus.channels == 1;
    case OP_BUS_STEREO: out = SpeakerArr::kStereo; return bus.channels == 2;
    case OP_BUS_SURROUND: {
        if (bus.channels == 0 || bus.channels > OP_ROLE_SLOTS) return false;
        for (uint32_t i = 0; i < bus.channels; ++i) {
            const SpeakerArrangement bit = speaker_of(bus.roles[i]);
            if (bit == 0 || (out & bit)) return false;
            out |= bit;
        }
        return true;
    }
    case OP_BUS_AMBISONIC:
        if (bus.channels == 0 || bus.channels > kMaxAmbisonicChannels) return false;
        for (uint32_t k = 0; k < bus.channels; ++k) out |= acn_bit(k);
        return true;
    default: return false;
    }
}

// map[i] = the host channel that carries canonical channel i of `bus`.
void channel_map(const OpBus& bus, SpeakerArrangement arrangement, uint8_t* map) {
    for (uint32_t i = 0; i < bus.channels && i < OP_MAX_CHANNELS; ++i) {
        const SpeakerArrangement bit = bus.kind == OP_BUS_SURROUND && i < OP_ROLE_SLOTS ? speaker_of(bus.roles[i]) : 0;
        if (bit != 0) {
            map[i] = static_cast<uint8_t>(SpeakerArr::getChannelCount(arrangement & (bit - 1)));
        } else {
            map[i] = static_cast<uint8_t>(i);
        }
    }
}

// One layout of the plugin as VST3 arrangements.
struct LayoutEntry {
    OpLayout layout{};
    SpeakerArrangement input = 0, output = 0, sidechain = 0;
};
struct Entry : LayoutEntry {
    uint32_t index = 0;  // op_shell_set_layout index, or kLegacyLayout
};

// The VST3 category string. The library's own category for an effect; an
// instrument is "Instrument|Synth" unless the plugin named its own
// "Instrument|..." sub-category; a note effect is an "Fx".
std::string category_for(const char* library, uint32_t kind) {
    const std::string name = library ? library : "";
    const bool instrument = name.rfind("Instrument|", 0) == 0;
    if (kind == OP_KIND_INSTRUMENT) return instrument ? name : "Instrument|Synth";
    if (name.rfind("Fx", 0) == 0) return name;
    return "Fx";
}

// ---- MIDI input through IMidiMapping ---------------------------------------
// The standard way in VST3 3.x to receive MIDI controllers, pitch bend and
// channel pressure is IMidiMapping: the controller maps (channel, controller
// number) to a hidden parameter and the host sends that parameter's changes
// in IParameterChanges, with sample offsets. The processor turns each point
// back into a MIDI message. Controller numbers 0..127 are CCs, kAfterTouch
// (128) channel pressure, kPitchBend (129) pitch bend. A plugin with a note
// input gets 16 x 130 hidden parameters from kMidiParamBase. Legacy
// kLegacyMIDICCOutEvent events on the event input are accepted too.
constexpr ParamID kMidiParamBase = 0x4D494400;  // "MID", clear of 0..n-1 and kBypassId
constexpr uint32_t kMidiControllers = 130;
constexpr uint32_t kMidiParamCount = 16 * kMidiControllers;

// ---- Parameter IDs (ABI 8) ---------------------------------------------------
// The ParamID the host stores for each plugin parameter, and the way back. A
// plugin without pins has its ABI indices as IDs, as before pins existed; a
// pinned one (op_shell_param_host_id) keeps its IDs through any reordering of
// its table. Everything inside the shell and the ABI addresses a parameter by
// its index; only the edge to the host uses the ID.
constexpr ParamID kBypassParamId = 0x42595053;
constexpr ParamID kMidiParamFirst = 0x4D494400;
constexpr uint32_t kMidiParamSpan = 16 * 130;
class ParamIds {
public:
    void load(uint32_t count) {
        ids_.assign(count, 0);
        sorted_.clear();
        identity_ = true;
        bool usable = true;
        for (uint32_t i = 0; i < count; ++i) {
            const uint32_t id = op_shell_param_host_id(OP_HOST_VST3, i);
            ids_[i] = id;
            if (id != i) identity_ = false;
            // The library has checked its pins; a library that answers an ID the
            // shell keeps for itself, or none, gets the indices instead.
            if (id > 0x7FFFFFFFu || id == kBypassParamId || (id >= kMidiParamFirst && id - kMidiParamFirst < kMidiParamSpan)) usable = false;
        }
        if (!identity_ && usable) {
            sorted_.reserve(count);
            for (uint32_t i = 0; i < count; ++i) sorted_.push_back({ids_[i], i});
            std::sort(sorted_.begin(), sorted_.end());
            for (size_t i = 1; i < sorted_.size(); ++i) usable = usable && sorted_[i].first != sorted_[i - 1].first;
        }
        if (!identity_ && !usable) {
            for (uint32_t i = 0; i < count; ++i) ids_[i] = i;
            sorted_.clear();
            identity_ = true;
        }
    }
    bool identity() const { return identity_; }
    ParamID id(uint32_t index) const { return ids_[index]; }
    // The ABI index of the parameter the host calls `id`, or -1. Audio thread: no allocation.
    int32 index(ParamID id) const {
        if (identity_) return id < ids_.size() ? static_cast<int32>(id) : -1;
        const auto at = std::lower_bound(sorted_.begin(), sorted_.end(), std::pair<ParamID, uint32_t>{id, 0});
        return at != sorted_.end() && at->first == id ? static_cast<int32>(at->second) : -1;
    }

private:
    std::vector<ParamID> ids_;
    std::vector<std::pair<ParamID, uint32_t>> sorted_;
    bool identity_ = true;
};

// ---- The head of the component state -------------------------------------------
// Version 1 (a plugin without pinned parameter IDs writes it, as always):
//   int32 1, int32 count, count normalized doubles in table order, int32 bypass.
// Version 2 (written when the parameter IDs are pinned):
//   int32 2, int32 count, count pairs of (int32 ParamID, double normalized), int32 bypass.
// The plugin-state trailer follows either. A pinned shell reads both: in a
// version 1 head a value's position is its ParamID, which is what it was in
// the build that wrote it; a value for an ID the plugin no longer has is
// dropped, and a parameter the head does not name keeps its default. Returns
// false, changing nothing, for a head it cannot read. `values` arrives holding
// the defaults.
constexpr int32 kMaxStateParams = 1 << 20;
inline bool read_state_head(IBStreamer& reader, const ParamIds& ids, std::vector<double>& values, bool clampRange) {
    int32 version = 0;
    int32 count = 0;
    if (!reader.readInt32(version) || !reader.readInt32(count)) return false;
    const auto usable = [clampRange](double value) { return std::isfinite(value) && (!clampRange || (value >= 0.0 && value <= 1.0)); };
    if (ids.identity()) {
        if (version != 1 || count != static_cast<int32>(values.size())) return false;
        for (double& slot : values) {
            if (!reader.readDouble(slot) || !usable(slot)) return false;
        }
        return true;
    }
    if ((version != 1 && version != 2) || count < 0 || count > kMaxStateParams) return false;
    if (version == 1 && count > static_cast<int32>(values.size())) return false;
    for (int32 i = 0; i < count; ++i) {
        int32 id = i;
        double value = 0.0;
        if (version == 2 && !reader.readInt32(id)) return false;
        if (!reader.readDouble(value) || !usable(value)) return false;
        const int32 index = ids.index(static_cast<ParamID>(id));
        // A version 1 head names only parameters the old build had; one of them missing is another plugin's state.
        if (index < 0 && version == 1) return false;
        if (index >= 0) values[static_cast<size_t>(index)] = value;
    }
    return true;
}

FUID fuid_from(const uint8_t bytes[16]) {
    TUID cid{};
    std::memcpy(cid, bytes, 16);
    return FUID::fromTUID(cid);
}

// ---- Parameter text -----------------------------------------------------------
// The host asks the controller for the text of a value (getParamStringByValue)
// and for the value of a typed text (getParamValueByString). The plugin owns
// both texts (op_shell_param_format / op_shell_param_parse, main thread); the
// controller only converts between UTF-8, VST3's UTF-16 and normalized values.

// UTF-8 to UTF-16, at most `capacity - 1` units plus a NUL. A text cut by the
// capacity never ends inside a surrogate pair. Malformed bytes become U+FFFD.
void utf8_to_utf16(const char* text, size_t length, TChar* out, size_t capacity) {
    if (capacity == 0) return;
    size_t count = 0;
    size_t i = 0;
    while (i < length) {
        const uint8_t lead = static_cast<uint8_t>(text[i]);
        uint32_t cp = 0xFFFD;
        size_t extra = 0;
        if (lead < 0x80) cp = lead;
        else if (lead >= 0xC2 && lead < 0xE0) { cp = lead & 0x1F; extra = 1; }
        else if (lead >= 0xE0 && lead < 0xF0) { cp = lead & 0x0F; extra = 2; }
        else if (lead >= 0xF0 && lead < 0xF5) { cp = lead & 0x07; extra = 3; }
        size_t used = 1;
        if (extra) {
            bool good = i + extra < length;  // all continuation bytes present
            for (size_t k = 1; k <= extra && good; ++k) {
                const uint8_t next = static_cast<uint8_t>(text[i + k]);
                if ((next & 0xC0) != 0x80) good = false;
                else cp = (cp << 6) | (next & 0x3F);
            }
            const bool overlong = (extra == 2 && cp < 0x800) || (extra == 3 && cp < 0x10000);
            if (good && !overlong && cp <= 0x10FFFF && !(cp >= 0xD800 && cp < 0xE000)) used = extra + 1;
            else cp = 0xFFFD;
        }
        const size_t need = cp > 0xFFFF ? 2 : 1;
        if (count + need + 1 > capacity) break;
        if (cp > 0xFFFF) {
            cp -= 0x10000;
            out[count++] = static_cast<TChar>(0xD800 + (cp >> 10));
            out[count++] = static_cast<TChar>(0xDC00 + (cp & 0x3FF));
        } else {
            out[count++] = static_cast<TChar>(cp);
        }
        i += used;
    }
    out[count] = 0;
}

// UTF-16 to UTF-8; unpaired surrogates become U+FFFD.
std::string utf16_to_utf8(const TChar* text) {
    std::string out;
    for (size_t i = 0; text && text[i];) {
        uint32_t cp = static_cast<uint16_t>(text[i++]);
        if (cp >= 0xD800 && cp < 0xDC00 && static_cast<uint16_t>(text[i]) >= 0xDC00 && static_cast<uint16_t>(text[i]) < 0xE000) {
            cp = 0x10000 + ((cp - 0xD800) << 10) + (static_cast<uint16_t>(text[i++]) - 0xDC00);
        } else if (cp >= 0xD800 && cp < 0xE000) {
            cp = 0xFFFD;
        }
        if (cp < 0x80) {
            out += static_cast<char>(cp);
        } else if (cp < 0x800) {
            out += static_cast<char>(0xC0 | (cp >> 6));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        } else if (cp < 0x10000) {
            out += static_cast<char>(0xE0 | (cp >> 12));
            out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        } else {
            out += static_cast<char>(0xF0 | (cp >> 18));
            out += static_cast<char>(0x80 | ((cp >> 12) & 0x3F));
            out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (cp & 0x3F));
        }
    }
    return out;
}

// The plugin's text for plain value `plain` of parameter `index`.
std::string shell_format(uint32_t index, double plain) {
    char small[128];
    const size_t length = op_shell_param_format(index, plain, small, sizeof(small));
    if (length < sizeof(small)) return std::string(small, length);
    std::string big(length + 1, '\0');
    const size_t again = op_shell_param_format(index, plain, big.data(), big.size());
    big.resize(std::min(again, big.size() - 1));
    return big;
}

// ---- Per-note expression (ABI 7) -> VST3 note expression ---------------------
// The library lists the expressions the plugin reacts to (op_shell_expressions)
// with their range in the SDK's units (op_shell_expression_info). Volume to
// brightness are the VST3 predefined types of the same numbers (kVolumeTypeID 0
// .. kBrightnessTypeID 5, ivstnoteexpression.h); pressure has no predefined type
// and is the custom type kCustomStart. Every range is linear, and the predefined
// VST3 mappings are the same lines: volume plain = 20 log(4 norm) dB is a gain of
// 4 norm over 0..4, tuning plain = 240 (norm - 0.5) semitones over -120..120, the
// others norm itself. So normalized = (value - min) / (max - min) for all.
constexpr NoteExpressionTypeID kPressureTypeID = NoteExpressionTypeIDs::kCustomStart;

struct ShellExpressions {
    uint32_t mask = 0;
    uint32_t count = 0;
    uint32_t order[OP_EXPRESSION_COUNT] = {};  // OP_EXPRESSION_* ids, listed order
    OpExpressionInfo info[OP_EXPRESSION_COUNT] = {};
    void load() {
        mask = 0;
        count = 0;
        const uint32_t listed = op_shell_expressions();
        for (uint32_t id = 0; id < OP_EXPRESSION_COUNT; ++id) {
            if (!(listed & (1u << id)) || !op_shell_expression_info(id, &info[id]) || !(info[id].max > info[id].min)) continue;
            mask |= 1u << id;
            order[count++] = id;
        }
    }
    bool has(uint32_t id) const { return id < OP_EXPRESSION_COUNT && (mask & (1u << id)); }
    static NoteExpressionTypeID typeOf(uint32_t id) { return id == OP_EXPRESSION_PRESSURE ? kPressureTypeID : static_cast<NoteExpressionTypeID>(id); }
    // The OP_EXPRESSION_* id of a VST3 type the plugin lists.
    bool idOf(NoteExpressionTypeID type, uint32_t& id) const {
        if (type == kPressureTypeID) id = OP_EXPRESSION_PRESSURE;
        else if (type < OP_EXPRESSION_PRESSURE) id = static_cast<uint32_t>(type);
        else return false;
        return has(id);
    }
    double plain(uint32_t id, double normalized) const {
        const OpExpressionInfo& i = info[id];
        return i.min + std::clamp(normalized, 0.0, 1.0) * (i.max - i.min);
    }
    double normalized(uint32_t id, double plain) const {
        const OpExpressionInfo& i = info[id];
        return std::clamp((plain - i.min) / (i.max - i.min), 0.0, 1.0);
    }
};

// ---- Parameter groups (ABI 5) -> IUnitInfo -----------------------------------
// The group path of parameter `index` ("Filter/Envelope"; "" is the top level).
std::string shell_group(uint32_t index) {
    char small[128];
    const size_t length = op_shell_param_group(index, small, sizeof(small));
    if (length < sizeof(small)) return std::string(small, length);
    std::string big(length + 1, '\0');
    const size_t again = op_shell_param_group(index, big.data(), big.size());
    big.resize(std::min(again, big.size() - 1));
    return big;
}

// One VST3 unit per group path. units[0] is the root (kRootUnitId, named
// "Root"); then one unit for every path and every parent of a path, parents
// first, numbered from 1 in the order their first parameter is declared. A
// unit's name is the last name of its path, its parent the unit of the rest.
// A parameter sits in the unit of its own path, a top-level one in the root.
// No units at all when no parameter has a group.
struct ShellUnit {
    UnitID id;
    UnitID parent;
    std::string name;
    std::string path;
};
struct ShellUnits {
    std::vector<ShellUnit> units;
    std::vector<UnitID> unitOf;  // by parameter index
};
ShellUnits shell_units(uint32_t count) {
    ShellUnits out;
    out.unitOf.assign(count, kRootUnitId);
    for (uint32_t i = 0; i < count; ++i) {
        const std::string path = shell_group(i);
        if (path.empty()) continue;
        if (out.units.empty()) out.units.push_back({kRootUnitId, kNoParentUnitId, "Root", ""});
        UnitID parent = kRootUnitId;
        size_t start = 0;
        for (;;) {
            const size_t slash = path.find('/', start);
            const std::string prefix = slash == std::string::npos ? path : path.substr(0, slash);
            UnitID id = -1;
            for (const ShellUnit& unit : out.units)
                if (unit.id != kRootUnitId && unit.path == prefix) id = unit.id;
            if (id < 0) {
                id = static_cast<UnitID>(out.units.size());
                const std::string name = slash == std::string::npos ? path.substr(start) : path.substr(start, slash - start);
                out.units.push_back({id, parent, name, prefix});
            }
            parent = id;
            if (slash == std::string::npos) break;
            start = slash + 1;
        }
        out.unitOf[i] = parent;
    }
    return out;
}

// "6.0 dB" with unit "dB" is "6.0"; text without that unit at its end is unchanged.
static std::string strip_unit(const std::string& text, const char* unit) {
    if (!unit || !*unit) return text;
    const std::string u(unit);
    if (text.size() <= u.size() || text.compare(text.size() - u.size(), u.size(), u) != 0) return text;
    std::string rest = text.substr(0, text.size() - u.size());
    while (!rest.empty() && rest.back() == ' ') rest.pop_back();
    return rest.empty() ? text : rest;
}

class ShellParameter final : public Parameter {
public:
    // `tag` is the ID the host stores, `index` the parameter's place in the ABI.
    ShellParameter(const TChar* title, const TChar* units, ParamID tag, uint32_t index, ParamValue normalized, int32 stepCount, int32 flags)
        : Parameter(title, tag, units, normalized, stepCount, flags), index_(index) {}
    ParamValue toPlain(ParamValue normalized) const SMTG_OVERRIDE { return op_shell_to_plain(index_, normalized); }
    ParamValue toNormalized(ParamValue plain) const SMTG_OVERRIDE { return op_shell_to_normalized(index_, plain); }
    // The plugin's own text for the plain value ("6.0 dB"), not the
    // normalized number the SDK prints.
    // VST3 hosts print the parameter's units after this string, so a trailing
    // unit the plugin's text already carries is dropped ("6.0 dB" -> "6.0").
    void toString(ParamValue normalized, String128 string) const SMTG_OVERRIDE {
        std::string text = shell_format(index_, toPlain(normalized));
        OpParamDesc desc{};
        if (op_shell_param(index_, &desc)) text = strip_unit(text, desc.unit);
        utf8_to_utf16(text.data(), text.size(), string, 128);
    }
    bool fromString(const TChar* string, ParamValue& normalized) const SMTG_OVERRIDE {
        const std::string text = utf16_to_utf8(string);
        double plain = 0.0;
        if (!op_shell_param_parse(index_, text.c_str(), &plain) || !std::isfinite(plain)) return false;
        const double value = toNormalized(plain);
        if (!std::isfinite(value)) return false;
        normalized = std::min(1.0, std::max(0.0, value));
        return true;
    }
    OBJ_METHODS(ShellParameter, Parameter)
private:
    uint32_t index_;
};

// Bypass reads "Off" and "On" instead of 0.0000 and 1.0000.
class BypassParameter final : public Parameter {
public:
    explicit BypassParameter(ParamID tag)
        : Parameter(STR16("Bypass"), tag, nullptr, 0.0, 1, ParameterInfo::kCanAutomate | ParameterInfo::kIsBypass) {}
    void toString(ParamValue normalized, String128 string) const SMTG_OVERRIDE {
        const char* text = normalized >= 0.5 ? "On" : "Off";
        utf8_to_utf16(text, std::strlen(text), string, 128);
    }
    bool fromString(const TChar* string, ParamValue& normalized) const SMTG_OVERRIDE {
        std::string text = utf16_to_utf8(string);
        for (auto& c : text) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        while (!text.empty() && std::isspace(static_cast<unsigned char>(text.back()))) text.pop_back();
        size_t start = 0;
        while (start < text.size() && std::isspace(static_cast<unsigned char>(text[start]))) ++start;
        text.erase(0, start);
        if (text == "on" || text == "1" || text == "yes" || text == "true") { normalized = 1.0; return true; }
        if (text == "off" || text == "0" || text == "no" || text == "false") { normalized = 0.0; return true; }
        return false;
    }
    OBJ_METHODS(BypassParameter, Parameter)
};

// The host bypass parameter. Its ID is far outside 0..n-1 so the plugin's own
// parameter IDs are the ABI indices, unshifted (or its pinned IDs, which the
// library keeps clear of this one).
constexpr ParamID kBypassId = kBypassParamId;
// Length of the switch between processed and bypassed audio.
constexpr double kBypassFadeSeconds = 0.010;

#if OP_HAS_EDITOR
constexpr const char* kEngineMessage = "openplugin.engine";
#endif
// Processor to controller (ABI 6): the plugin's latency or parameter info
// changed. Attribute "flags" (int, OP_CHANGE_*). Sent from the processor's
// main-thread timer; the controller answers with restartComponent.
constexpr const char* kChangesMessage = "openplugin.changes";
// How often the processor asks its engine what changed (op_shell_take_changes).
constexpr uint32 kChangesPollMs = 30;
// Controller to processor: the host's track (name, colour, kind), from
// IInfoListener::setChannelContextInfos. Attributes: "name" (TChar string,
// absent for none), "rgba" (int, 0xRRGGBBAA), "flags" (int, OP_TRACK_*).
constexpr const char* kTrackMessage = "openplugin.track";

// What the host last said about the track, as op_shell_set_track_info takes it.
struct TrackState {
    bool valid = false;
    std::string name;  // UTF-8; empty for none
    uint32_t rgba = 0;
    uint32_t flags = 0;
    const char* nameOrNull() const { return name.empty() ? nullptr : name.c_str(); }
};

// UTF-16 (a VST3 TChar string) to UTF-8, with surrogate pairs; an unpaired
// surrogate becomes U+FFFD.
std::string utf8_from_tchar(const TChar* text) {
    std::string out;
    if (!text) return out;
    for (size_t i = 0; text[i]; ++i) {
        uint32_t c = text[i];
        if (c >= 0xD800 && c <= 0xDBFF && text[i + 1] >= 0xDC00 && text[i + 1] <= 0xDFFF) {
            c = 0x10000 + ((c - 0xD800) << 10) + (text[i + 1] - 0xDC00);
            ++i;
        } else if (c >= 0xD800 && c <= 0xDFFF) {
            c = 0xFFFD;
        }
        if (c < 0x80) {
            out += static_cast<char>(c);
        } else if (c < 0x800) {
            out += static_cast<char>(0xC0 | (c >> 6));
            out += static_cast<char>(0x80 | (c & 0x3F));
        } else if (c < 0x10000) {
            out += static_cast<char>(0xE0 | (c >> 12));
            out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (c & 0x3F));
        } else {
            out += static_cast<char>(0xF0 | (c >> 18));
            out += static_cast<char>(0x80 | ((c >> 12) & 0x3F));
            out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
            out += static_cast<char>(0x80 | (c & 0x3F));
        }
    }
    return out;
}

// UTF-8 to UTF-16 into `out` (room for `capacity` TChars, NUL included). Text
// that is cut falls on a character boundary; invalid bytes become U+FFFD.
void tchar_from_utf8(const std::string& text, TChar* out, size_t capacity) {
    size_t n = 0;
    for (size_t i = 0; i < text.size();) {
        const uint8_t b = static_cast<uint8_t>(text[i]);
        uint32_t c = 0xFFFD;
        size_t len = 1;
        if (b < 0x80) {
            c = b;
        } else if (b >= 0xC2 && b < 0xE0 && i + 1 < text.size() && (text[i + 1] & 0xC0) == 0x80) {
            c = ((b & 0x1F) << 6) | (text[i + 1] & 0x3F);
            len = 2;
        } else if (b >= 0xE0 && b < 0xF0 && i + 2 < text.size() && (text[i + 1] & 0xC0) == 0x80 && (text[i + 2] & 0xC0) == 0x80) {
            c = ((b & 0x0F) << 12) | ((text[i + 1] & 0x3F) << 6) | (text[i + 2] & 0x3F);
            len = 3;
        } else if (b >= 0xF0 && b < 0xF5 && i + 3 < text.size() && (text[i + 1] & 0xC0) == 0x80 && (text[i + 2] & 0xC0) == 0x80 && (text[i + 3] & 0xC0) == 0x80) {
            c = ((b & 0x07) << 18) | ((text[i + 1] & 0x3F) << 12) | ((text[i + 2] & 0x3F) << 6) | (text[i + 3] & 0x3F);
            len = 4;
        }
        if (c >= 0xD800 && c <= 0xDFFF) c = 0xFFFD;
        const size_t need = c >= 0x10000 ? 2 : 1;
        if (n + need + 1 > capacity) break;
        if (c >= 0x10000) {
            c -= 0x10000;
            out[n++] = static_cast<TChar>(0xD800 + (c >> 10));
            out[n++] = static_cast<TChar>(0xDC00 + (c & 0x3FF));
        } else {
            out[n++] = static_cast<TChar>(c);
        }
        i += len;
    }
    if (capacity > 0) out[n] = 0;
}

// Component state. The head is unchanged since version 1: int32 version (1),
// int32 parameter count, one double per parameter, then an int32 bypass flag
// (absent in states written before bypass). After the flag a state may carry
// the plugin's whole state from op_shell_save_state as one trailer:
//   uint32 kStateTrailerTag, int32 trailer version (1), uint32 length, bytes.
// The trailer is last, so a shell that predates it reads the head and stops.
// A state without a trailer is a version 1 state and loads as before. A
// trailer with another tag or version, a length of 0 or above kMaxStateBlob,
// fewer bytes than its length, or bytes after it, is refused.
constexpr uint32 kStateTrailerTag = 0x4F505342;  // 'OPSB'
constexpr int32 kStateTrailerVersion = 1;
constexpr uint32 kMaxStateBlob = 1u << 30;

// Saves the whole state of `instance`: query the size, then copy, and repeat
// while the plugin says the state grew in between (OP_STATE_TOO_SMALL).
bool save_blob(void* instance, std::vector<uint8_t>& out) {
    uint32_t needed = 0;
    if (!instance || op_shell_save_state(instance, nullptr, 0, &needed) != OP_STATE_TOO_SMALL) return false;
    for (int attempt = 0; attempt < 64; ++attempt) {
        if (needed == 0 || needed > kMaxStateBlob) return false;
        out.assign(needed, 0);
        const uint32_t result = op_shell_save_state(instance, out.data(), needed, &needed);
        if (result == OP_STATE_OK) {
            if (needed == 0 || needed > out.size()) return false;
            out.resize(needed);
            return true;
        }
        if (result != OP_STATE_TOO_SMALL) return false;
    }
    return false;
}

class Processor final : public AudioEffect, public ITimerCallback {
public:
    Processor() {
        OpShellInfo info{};
        if (!load_info(info)) return;
        setControllerClass(fuid_from(info.controller_cid));
        count_ = info.param_count;
        ids_.load(count_);
        values_ = std::make_unique<std::atomic<double>[]>(count_);
        defaults_.assign(count_, 0.0);
        for (uint32_t i = 0; i < count_; ++i) {
            OpParamDesc desc{};
            if (op_shell_param(i, &desc)) defaults_[i] = desc.default_normalized;
            values_[i].store(defaults_[i]);
        }
        loadLayouts();
        const uint32_t ports = op_shell_note_ports();
        noteIn_ = (ports & OP_NOTE_PORT_INPUT) != 0;
        noteOut_ = (ports & OP_NOTE_PORT_OUTPUT) != 0;
        expressions_.load();
    }
    ~Processor() override {
        stopPolling();
        destroy();
    }
    static FUnknown* create(void*) { return static_cast<IAudioProcessor*>(new Processor); }

    tresult PLUGIN_API initialize(FUnknown* context) SMTG_OVERRIDE {
        if (count_ == 0) return kResultFalse;
        tresult result = AudioEffect::initialize(context);
        if (result != kResultOk) return result;
        // Buses follow the plugin's layouts: a main input and output when any
        // layout has one, an aux input for the sidechain (inactive until the
        // host activates it, the VST3 convention), an event bus per note port.
        if (hasIn_) addAudioInput(STR16("Input"), firstArrangement(&LayoutEntry::input));
        if (hasSide_) addAudioInput(STR16("Sidechain"), firstArrangement(&LayoutEntry::sidechain), kAux, 0);
        if (hasOut_) addAudioOutput(STR16("Output"), firstArrangement(&LayoutEntry::output));
        if (noteIn_) addEventInput(STR16("MIDI In"), 16);
        if (noteOut_) addEventOutput(STR16("MIDI Out"), 16);
        select(0);
        return kResultOk;
    }
    // Accepts an arrangement set when it equals the buses of one of the
    // plugin's layouts (op_shell_layout*); that layout is then selected for the
    // next activation. A host that offers something else gets false and reads
    // the current arrangement back through getBusArrangement.
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* inputs, int32 inputCount, SpeakerArrangement* outputs, int32 outputCount) SMTG_OVERRIDE {
        const int32 mainIn = hasIn_ ? 1 : 0;
        const int32 fullIn = mainIn + (hasSide_ ? 1 : 0);
        const int32 fullOut = hasOut_ ? 1 : 0;
        if (inputCount < 0 || outputCount < 0 || (inputCount && !inputs) || (outputCount && !outputs)) return kInvalidArgument;
        if ((inputCount != fullIn && inputCount != mainIn) || outputCount != fullOut) return kResultFalse;
        SpeakerArrangement in = SpeakerArr::kEmpty, out = SpeakerArr::kEmpty, side = SpeakerArr::kEmpty;
        if (hasIn_) in = inputs[0];
        if (hasSide_) {
            if (inputCount == fullIn) side = inputs[mainIn];
            else getBusArrangement(kInput, mainIn, side);
        }
        if (hasOut_) out = outputs[0];
        // A host that names no aux bus leaves the sidechain to the plugin: the
        // layout that keeps the current aux arrangement, else the first with
        // these main buses.
        const bool sideGiven = !hasSide_ || inputCount == fullIn;
        for (size_t i = 0; i < layouts_.size() && !sideGiven; ++i) {
            if (layouts_[i].input == in && layouts_[i].output == out && layouts_[i].sidechain == side) {
                select(i);
                return AudioEffect::setBusArrangements(inputs, inputCount, outputs, outputCount);
            }
        }
        for (size_t i = 0; i < layouts_.size(); ++i) {
            const LayoutEntry& entry = layouts_[i];
            if (entry.input != in || entry.output != out || (sideGiven && entry.sidechain != side)) continue;
            select(i);
            return AudioEffect::setBusArrangements(inputs, inputCount, outputs, outputCount);
        }
        return kResultFalse;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) SMTG_OVERRIDE { return size == kSample32 ? kResultTrue : kResultFalse; }
    tresult PLUGIN_API setupProcessing(ProcessSetup& setup) SMTG_OVERRIDE {
        if (setup.symbolicSampleSize != kSample32 || setup.maxSamplesPerBlock < 1 || setup.maxSamplesPerBlock > 65536) return kInvalidArgument;
        if (!std::isfinite(setup.sampleRate) || setup.sampleRate < 8000.0 || setup.sampleRate > 768000.0) return kInvalidArgument;
        return AudioEffect::setupProcessing(setup);
    }
    tresult PLUGIN_API setActive(TBool active) SMTG_OVERRIDE {
        if (!active) {
            stopPolling();
            announce(nullptr);
            keepChunk();
            destroy();
            return AudioEffect::setActive(active);
        }
        if (processSetup.maxSamplesPerBlock < 1) return kResultFalse;
        void* next = op_shell_create(processSetup.sampleRate, static_cast<uint32_t>(processSetup.maxSamplesPerBlock), channels_);
        if (!next) return kResultFalse;
        if (layoutIndex_ != kLegacyLayout && !op_shell_set_layout(next, layoutIndex_)) {
            op_shell_destroy(next);
            return kResultFalse;
        }
        offline_ = false;
        keepChunk();
        destroy();
        engine_ = next;
        // The plugin's state set while inactive, or kept from the engine that
        // just went away, goes into the new engine, then the values held here.
        // The reset applies them, so the latency the host reads next (after a
        // kLatencyChanged restart, too) is the latency of these values.
        if (!pendingBlob_.empty()) op_shell_load_state(engine_, pendingBlob_.data(), static_cast<uint32_t>(pendingBlob_.size()));
        pendingBlob_.clear();
        pushValues(engine_);
        op_shell_reset(engine_);
        prepareBypass();
        announce(engine_);
        applyTrack();
        startPolling();
        return AudioEffect::setActive(active);
    }
    // The controller's message with the host's track. Message thread. Kept for
    // an engine made later, and given to the one that exists now.
    tresult PLUGIN_API notify(IMessage* message) SMTG_OVERRIDE {
        if (message && message->getMessageID() && std::strcmp(message->getMessageID(), kTrackMessage) == 0) {
            IAttributeList* attributes = message->getAttributes();
            if (!attributes) return kInvalidArgument;
            TrackState next;
            TChar name[256] = {};
            if (attributes->getString("name", name, sizeof(name)) == kResultOk) next.name = utf8_from_tchar(name);
            int64 rgba = 0, flags = 0;
            if (attributes->getInt("rgba", rgba) == kResultOk) next.rgba = static_cast<uint32_t>(rgba);
            if (attributes->getInt("flags", flags) == kResultOk) next.flags = static_cast<uint32_t>(flags);
            next.valid = true;
            track_ = next;
            applyTrack();
            return kResultOk;
        }
        return AudioEffect::notify(message);
    }
    tresult PLUGIN_API setProcessing(TBool active) SMTG_OVERRIDE {
        if (active && engine_) {
            op_shell_reset(engine_);
            clearBypassLine();
            // A fresh start has nothing to fade from.
            mix_ = bypass_.load() ? 1.0f : 0.0f;
        }
        return kResultOk;
    }
    uint32 PLUGIN_API getLatencySamples() SMTG_OVERRIDE { return engine_ ? op_shell_latency(engine_) : 0; }
#if OP_HAS_TAIL
    uint32 PLUGIN_API getTailSamples() SMTG_OVERRIDE {
        if (!engine_) return kNoTail;
        const double samples = op_shell_tail_seconds(engine_) * processSetup.sampleRate;
        if (!(samples < 4.0e9)) return kInfiniteTail;
        return samples > 0.0 ? static_cast<uint32>(samples) : kNoTail;
    }
#endif
    // Loads the head and, if present, the plugin-state trailer (see above). It
    // changes nothing unless everything is accepted: op_shell_load_state
    // validates before it applies, and the parameter values and bypass change
    // only after that.
    tresult PLUGIN_API setState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        std::vector<double> loaded(defaults_);
        if (!read_state_head(reader, ids_, loaded, true)) return kResultFalse;
        // Bypass follows the parameters. A state written before bypass existed
        // ends here, which means not bypassed.
        int32 bypass = 0;
        bool hasTrailer = false;
        std::vector<uint8_t> blob;
        if (reader.readInt32(bypass)) {
            if (!readTrailer(stream, hasTrailer, blob)) return kResultFalse;
        } else {
            bypass = 0;
        }
        if (hasTrailer) {
            if (engine_) {
                if (!op_shell_load_state(engine_, blob.data(), static_cast<uint32_t>(blob.size()))) return kResultFalse;
            } else {
                // No engine to load into: check the blob on a scratch instance
                // and keep it for the next setActive.
                if (!checkBlob(blob)) return kResultFalse;
                pendingBlob_ = std::move(blob);
            }
        }
        for (uint32_t i = 0; i < count_; ++i) values_[i].store(loaded[i]);
        bypass_.store(bypass != 0);
        if (hasTrailer && engine_) pushValues(engine_);
        return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        // The plugin's state first, so a failure writes nothing. The engine
        // gets the values held here before it is saved, so head and blob agree.
        std::vector<uint8_t> blob;
        if (engine_) {
            pushValues(engine_);
            if (!save_blob(engine_, blob)) return kResultFalse;
        } else {
            blob = pendingBlob_;
        }
        IBStreamer writer(stream, kLittleEndian);
        // Pinned parameter IDs: each value is written under its ID (version 2).
        const bool keyed = !ids_.identity();
        if (!writer.writeInt32(keyed ? 2 : 1) || !writer.writeInt32(static_cast<int32>(count_))) return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) {
            if (keyed && !writer.writeInt32(static_cast<int32>(ids_.id(i)))) return kResultFalse;
            if (!writer.writeDouble(values_[i].load())) return kResultFalse;
        }
        if (!writer.writeInt32(bypass_.load() ? 1 : 0)) return kResultFalse;
        if (blob.empty()) return kResultOk;
        if (!writer.writeInt32(static_cast<int32>(kStateTrailerTag)) || !writer.writeInt32(kStateTrailerVersion) || !writer.writeInt32(static_cast<int32>(blob.size()))) return kResultFalse;
        int32 written = 0;
        if (stream->write(blob.data(), static_cast<int32>(blob.size()), &written) != kResultOk || written != static_cast<int32>(blob.size())) return kResultFalse;
        return kResultOk;
    }
    // One host block through op_shell_process_events. Parameter points,
    // notes and mapped MIDI are one sample-offset event list; the engine cuts
    // the block at each offset, so changes are sample-accurate. The bypass
    // switch cuts the block here instead, because the crossfade is the shell's.
    // A block longer than the prepared size, or with more than OP_MAX_EVENTS
    // events, is passed to the engine in consecutive chunks.
    tresult PLUGIN_API process(ProcessData& data) SMTG_OVERRIDE {
        const int32 numSamples = std::max<int32>(data.numSamples, 0);
        collect(data, numSamples);
        if (data.numSamples == 0) {
            applyHeld(0);
            return kResultOk;
        }
        if (!engine_ || data.symbolicSampleSize != kSample32 || data.numSamples < 0) return kResultFalse;
        // The buses of the selected layout, in the host's channel order.
        AudioBusBuffers* hostIn = nullptr;
        AudioBusBuffers* hostOut = nullptr;
        AudioBusBuffers* hostSide = nullptr;
        if (inCh_ > 0 && data.numInputs >= 1 && data.inputs && data.inputs[0].numChannels > 0) {
            hostIn = &data.inputs[0];
            if (hostIn->numChannels < static_cast<int32>(inCh_) || !hostIn->channelBuffers32) return kResultFalse;
        }
        if (outCh_ > 0) {
            if (data.numOutputs < 1 || !data.outputs) return kResultFalse;
            hostOut = &data.outputs[0];
            if (hostOut->numChannels < static_cast<int32>(outCh_) || !hostOut->channelBuffers32) return kResultFalse;
        }
        const int32 sideBus = hasIn_ ? 1 : 0;
        if (sideCh_ > 0 && hasSide_ && data.numInputs > sideBus && data.inputs && data.inputs[sideBus].numChannels >= static_cast<int32>(sideCh_) &&
            data.inputs[sideBus].channelBuffers32)
            hostSide = &data.inputs[sideBus];

        const bool offline = data.processMode == kOffline;
        if (offline != offline_) {
            op_shell_set_offline(engine_, offline ? 1 : 0);
            offline_ = offline;
        }
        // Start from the values held at the end of the previous block, then
        // record the values this block's points leave behind.
        for (uint32_t i = 0; i < count_; ++i) {
            if (!op_shell_set_normalized(engine_, i, static_cast<float>(values_[i].load()))) return kResultFalse;
        }
        for (uint32_t i = 0; i < nTagged_; ++i) {
            const OpEvent& ev = tagged_[i].ev;
            if (ev.kind == OP_EVENT_PARAM_NORMALIZED) values_[ev.index].store(ev.value);
        }
        const bool haveTransport = data.processContext != nullptr;
        if (haveTransport) buildTransport(*data.processContext);

        const int32 limit = processSetup.maxSamplesPerBlock > 0 ? processSetup.maxSamplesPerBlock : data.numSamples;
        int32 start = 0;
        uint32_t next = 0;
        uint32_t nextBypass = 0;
        while (start < data.numSamples) {
            while (nextBypass < nBypass_ && bypassPoints_[nextBypass].offset <= start) bypass_.store(bypassPoints_[nextBypass++].on);
            int32 end = std::min(data.numSamples, start + limit);
            if (nextBypass < nBypass_) end = std::min(end, bypassPoints_[nextBypass].offset);
            uint32_t count = 0;
            while (next + count < nTagged_ && count < OP_MAX_EVENTS && static_cast<int32>(tagged_[next + count].ev.offset) < end) ++count;
            if (count == OP_MAX_EVENTS && next + count < nTagged_ && static_cast<int32>(tagged_[next + count].ev.offset) < end) {
                // Too many events for one call: end the chunk where the first
                // one left over starts (at least one sample on).
                end = std::max(static_cast<int32>(tagged_[next + count].ev.offset), start + 1);
                while (count > 0 && static_cast<int32>(tagged_[next + count - 1].ev.offset) >= end) --count;
            }
            for (uint32_t k = 0; k < count; ++k) {
                chunk_[k] = tagged_[next + k].ev;
                const int32 at = static_cast<int32>(chunk_[k].offset);
                chunk_[k].offset = at > start ? static_cast<uint32_t>(at - start) : 0;
            }
            const uint32_t frames = static_cast<uint32_t>(end - start);
            if (hostIn)
                for (uint32_t i = 0; i < inCh_; ++i) inPtr_[i] = hostIn->channelBuffers32[inMap_[i]] + start;
            if (hostOut)
                for (uint32_t i = 0; i < outCh_; ++i) outPtr_[i] = hostOut->channelBuffers32[outMap_[i]] + start;
            if (hostSide)
                for (uint32_t i = 0; i < sideCh_; ++i) sidePtr_[i] = hostSide->channelBuffers32[sideMap_[i]] + start;
            OpProcess args{};
            args.struct_size = sizeof(OpProcess);
            args.frames = frames;
            args.inputs = hostIn ? inPtr_ : nullptr;
            args.input_channels = hostIn ? inCh_ : 0;
            args.output_channels = hostOut ? outCh_ : 0;
            args.outputs = hostOut ? outPtr_ : nullptr;
            args.sidechain = hostSide ? sidePtr_ : nullptr;
            args.sidechain_channels = hostSide ? sideCh_ : 0;
            args.event_count = count;
            args.events = count ? chunk_ : nullptr;
            args.transport = haveTransport ? &transport_ : nullptr;
            const float target = bypass_.load() ? 1.0f : 0.0f;
            // The delayed dry signal is taken before the engine runs, because the
            // host may have given the same buffer for input and output.
            if (hostOut) captureDry(hostIn, start, frames);
            if (!op_shell_process_events(engine_, &args)) return kResultFalse;
            if (hostOut) applyBypass(*hostOut, start, frames, target);
            emitMidi(data.outputEvents, start);
            next += count;
            start = end;
        }
        // Points at the very end of the block hold for the next one.
        applyHeld(next);
        if (hostOut) hostOut->silenceFlags = 0;
        return kResultOk;
    }

private:
    // Reads the trailer that follows the bypass flag. `present` stays false when
    // the stream ends right there; a partial, foreign, empty, oversized or
    // extended trailer is an error.
    static bool readTrailer(IBStream* stream, bool& present, std::vector<uint8_t>& blob) {
        present = false;
        IBStreamer reader(stream, kLittleEndian);
        uint32_t tagBytes = 0;
        int32 got = 0;
        if (stream->read(&tagBytes, 4, &got) != kResultOk || got == 0) return true;
        if (got != 4) return false;
#if BYTEORDER == kBigEndian
        tagBytes = SWAP_32(tagBytes);
#endif
        int32 version = 0;
        int32 length = 0;
        if (tagBytes != kStateTrailerTag || !reader.readInt32(version) || version != kStateTrailerVersion || !reader.readInt32(length)) return false;
        if (length < 1 || static_cast<uint32_t>(length) > kMaxStateBlob) return false;
        // Read in pieces so a corrupt length cannot make a huge allocation
        // before the stream has run out.
        constexpr int32 kPiece = 1 << 16;
        blob.clear();
        int32 remaining = length;
        while (remaining > 0) {
            const int32 want = std::min(remaining, kPiece);
            const size_t at = blob.size();
            blob.resize(at + static_cast<size_t>(want));
            int32 read = 0;
            if (stream->read(blob.data() + at, want, &read) != kResultOk || read != want) return false;
            remaining -= want;
        }
        uint8_t extra = 0;
        int32 more = 0;
        if (stream->read(&extra, 1, &more) == kResultOk && more != 0) return false;
        present = true;
        return true;
    }
    // Checks a blob without an engine: a scratch instance loads it, then goes.
    bool checkBlob(const std::vector<uint8_t>& blob) const {
        const double rate = processSetup.sampleRate >= 8000.0 ? processSetup.sampleRate : 48000.0;
        const uint32_t block = processSetup.maxSamplesPerBlock >= 1 ? static_cast<uint32_t>(processSetup.maxSamplesPerBlock) : 512u;
        void* scratch = op_shell_create(rate, block, channels_);
        if (!scratch) return false;
        const bool ok = op_shell_load_state(scratch, blob.data(), static_cast<uint32_t>(blob.size())) != 0;
        op_shell_destroy(scratch);
        return ok;
    }
    // Hands the engine the parameter values held here.
    void pushValues(void* instance) {
        for (uint32_t i = 0; i < count_; ++i) op_shell_set_normalized(instance, i, static_cast<float>(values_[i].load()));
    }
    // Before the engine goes away, keep the plugin's state for the next one.
    void keepChunk() {
        if (engine_ && !save_blob(engine_, pendingBlob_)) pendingBlob_.clear();
    }
    std::vector<uint8_t> pendingBlob_;
    // ---- layouts -----------------------------------------------------------
    void loadLayouts() {
        const uint32_t n = std::min<uint32_t>(op_shell_layout_count(), 64);
        for (uint32_t i = 0; i < n; ++i) {
            Entry entry;
            if (!op_shell_layout(i, &entry.layout)) continue;
            entry.index = i;
            if (arrangement_of(entry.layout.input, entry.input) && arrangement_of(entry.layout.output, entry.output) &&
                arrangement_of(entry.layout.sidechain, entry.sidechain))
                layouts_.push_back(entry);
        }
        // Shells before ABI 2 took mono or stereo for any plugin. A plugin that
        // only declares the default stereo layout keeps accepting mono (an
        // engine bus of one channel); a plugin whose layouts VST3 cannot name
        // is stereo and mono as well.
        const bool plainStereo = n == 1 && layouts_.size() == 1 && layouts_[0].input == SpeakerArr::kStereo &&
                                 layouts_[0].output == SpeakerArr::kStereo && layouts_[0].sidechain == SpeakerArr::kEmpty;
        if (layouts_.empty()) {
            layouts_.push_back(legacyEntry(2));
            layouts_.push_back(legacyEntry(1));
        } else if (plainStereo) {
            layouts_.push_back(legacyEntry(1));
        }
        for (const Entry& entry : layouts_) {
            hasIn_ = hasIn_ || entry.input != SpeakerArr::kEmpty;
            hasOut_ = hasOut_ || entry.output != SpeakerArr::kEmpty;
            hasSide_ = hasSide_ || entry.sidechain != SpeakerArr::kEmpty;
        }
    }
    static Entry legacyEntry(uint32_t channels) {
        Entry entry;
        entry.index = kLegacyLayout;
        OpBus bus{};
        bus.kind = channels == 1 ? OP_BUS_MONO : OP_BUS_STEREO;
        bus.channels = channels;
        std::memset(bus.roles, OP_ROLE_NONE, sizeof(bus.roles));
        if (channels == 2) {
            bus.roles[0] = 0;
            bus.roles[1] = 1;
        }
        entry.layout.input = entry.layout.output = bus;
        entry.layout.sidechain.kind = OP_BUS_ABSENT;
        entry.input = entry.output = channels == 1 ? SpeakerArr::kMono : SpeakerArr::kStereo;
        entry.sidechain = SpeakerArr::kEmpty;
        return entry;
    }
    SpeakerArrangement firstArrangement(SpeakerArrangement LayoutEntry::*bus) const {
        for (const Entry& entry : layouts_)
            if (entry.*bus != SpeakerArr::kEmpty) return entry.*bus;
        return SpeakerArr::kStereo;
    }
    // Makes layouts_[i] the layout of the next activation and builds the
    // canonical-to-host channel tables for its three buses.
    void select(size_t i) {
        if (i >= layouts_.size()) return;
        const Entry& entry = layouts_[i];
        layoutIndex_ = entry.index;
        channels_ = entry.index == kLegacyLayout && entry.layout.input.channels == 1 ? 1u : 2u;
        inCh_ = entry.layout.input.channels;
        outCh_ = entry.layout.output.channels;
        sideCh_ = entry.layout.sidechain.channels;
        channel_map(entry.layout.input, entry.input, inMap_);
        channel_map(entry.layout.output, entry.output, outMap_);
        channel_map(entry.layout.sidechain, entry.sidechain, sideMap_);
    }

    // ---- events ------------------------------------------------------------
    bool relevant(ParamID id) const {
        return ids_.index(id) >= 0 || id == kBypassId || (noteIn_ && id >= kMidiParamBase && id < kMidiParamBase + kMidiParamCount);
    }
    static uint8_t to_midi7(double normalized) {
        return static_cast<uint8_t>(std::lround(std::clamp(normalized, 0.0, 1.0) * 127.0));
    }
    // A VST3 note id as the ABI carries it: int32 bits, -1 for none.
    static uint32_t noteId(int32 id) { return static_cast<uint32_t>(id < 0 ? -1 : id); }
    static uint8_t velocity7(float velocity) {
        return static_cast<uint8_t>(std::clamp<long>(std::lround(velocity * 127.0f), 0, 127));
    }
    void addEvent(const OpEvent& ev) {
        if (nTagged_ < tagged_.size()) tagged_[nTagged_++] = Tagged{ev, order_++};
    }
    void addMidi(int32 at, uint8_t status, uint8_t d1, uint8_t d2) {
        OpEvent ev{};
        ev.offset = static_cast<uint32_t>(at);
        ev.kind = OP_EVENT_MIDI;
        ev.midi[0] = status;
        ev.midi[1] = d1;
        ev.midi[2] = d2;
        addEvent(ev);
    }
    // One point of a parameter queue: a plugin parameter (normalized event at
    // its offset), the bypass switch (kept apart: it cuts the block), or a
    // mapped MIDI controller (a MIDI message).
    void addPoint(ParamID id, int32 at, double value, int32 numSamples) {
        if (id == kBypassId) {
            if (nBypass_ < bypassPoints_.size())
                bypassPoints_[nBypass_++] = BypassPoint{std::clamp(at, 0, numSamples), order_++, value >= 0.5};
            return;
        }
        if (const int32 index = ids_.index(id); index >= 0) {
            OpEvent ev{};
            ev.offset = static_cast<uint32_t>(std::clamp(at, 0, numSamples));
            ev.kind = OP_EVENT_PARAM_NORMALIZED;
            ev.index = static_cast<uint32_t>(index);
            ev.value = static_cast<float>(std::clamp(value, 0.0, 1.0));
            addEvent(ev);
            return;
        }
        if (numSamples < 1) return;
        const int32 when = std::clamp(at, 0, numSamples - 1);
        const uint32_t slot = id - kMidiParamBase;
        const uint8_t channel = static_cast<uint8_t>(slot / kMidiControllers);
        const uint32_t controller = slot % kMidiControllers;
        if (controller < 128) {
            addMidi(when, static_cast<uint8_t>(0xB0 | channel), static_cast<uint8_t>(controller), to_midi7(value));
        } else if (controller == kAfterTouch) {
            addMidi(when, static_cast<uint8_t>(0xD0 | channel), to_midi7(value), 0);
        } else {
            const long bend = std::lround(std::clamp(value, 0.0, 1.0) * 16383.0);
            addMidi(when, static_cast<uint8_t>(0xE0 | channel), static_cast<uint8_t>(bend & 0x7F), static_cast<uint8_t>(bend >> 7));
        }
    }
    // Gathers the points of every queue, ordered by (offset, arrival). Offsets
    // are clamped into the block. If the queues carry more points than fit,
    // each queue keeps an even share of its own points (first, last and evenly
    // spaced between), so the final value of every parameter is always kept
    // and the ramp shape is approximated. Hosts send one queue per parameter,
    // so that does not occur in practice.
    void collectParams(IParameterChanges* queues, int32 numSamples) {
        if (!queues || tagged_.empty()) return;
        const uint32_t capacity = paramCapacity_;
        const int32 queueCount = queues->getParameterCount();
        uint64_t total = 0;
        uint32_t used = 0;
        for (int32 q = 0; q < queueCount; ++q) {
            IParamValueQueue* queue = queues->getParameterData(q);
            if (!queue || !relevant(queue->getParameterId())) continue;
            const int32 points = queue->getPointCount();
            if (points < 1) continue;
            total += static_cast<uint64_t>(points);
            ++used;
        }
        if (used == 0) return;
        const bool thin = total > capacity;
        const uint32_t share = thin ? std::max<uint32_t>(capacity / used, 1) : 0;
        uint32_t taken = 0;
        for (int32 q = 0; q < queueCount && taken < capacity; ++q) {
            IParamValueQueue* queue = queues->getParameterData(q);
            if (!queue) continue;
            const ParamID id = queue->getParameterId();
            if (!relevant(id)) continue;
            const int32 points = queue->getPointCount();
            if (points < 1) continue;
            const uint32_t keep = thin ? std::min<uint32_t>(share, static_cast<uint32_t>(points)) : static_cast<uint32_t>(points);
            for (uint32_t k = 0; k < keep && taken < capacity; ++k) {
                int32 index = static_cast<int32>(k);
                if (keep < static_cast<uint32_t>(points))
                    index = keep == 1 ? points - 1 : static_cast<int32>(static_cast<uint64_t>(k) * static_cast<uint64_t>(points - 1) / (keep - 1));
                int32 at = 0;
                ParamValue value = 0.0;
                if (queue->getPoint(index, at, value) != kResultTrue || !std::isfinite(value)) continue;
                addPoint(id, at, value, numSamples);
                ++taken;
            }
        }
    }
    // Notes (with the host's note id, ABI 7), note expression values for the
    // types the plugin lists, polyphonic pressure and legacy MIDI controller
    // events of the input event list. Note-on with velocity 0 is a note-off.
    // NoteOnEvent::tuning (cents at note-on) is not carried.
    void collectNotes(IEventList* events, int32 numSamples) {
        if (!noteIn_ || !events || numSamples < 1) return;
        const int32 count = events->getEventCount();
        uint32_t taken = 0;
        for (int32 i = 0; i < count && taken < kNoteCapacity; ++i) {
            Event e{};
            if (events->getEvent(i, e) != kResultOk) continue;
            const int32 at = std::clamp(e.sampleOffset, 0, numSamples - 1);
            const uint32_t before = nTagged_;
            OpEvent ev{};
            ev.offset = static_cast<uint32_t>(at);
            switch (e.type) {
            case Event::kNoteOnEvent:
                if (e.noteOn.pitch < 0 || e.noteOn.pitch > 127) break;
                ev.channel = static_cast<uint8_t>(std::clamp<int>(e.noteOn.channel, 0, 15));
                ev.note = static_cast<uint8_t>(e.noteOn.pitch);
                ev.index = noteId(e.noteOn.noteId);
                if (e.noteOn.velocity > 0.0f) {
                    ev.kind = OP_EVENT_NOTE_ON_ID;
                    ev.velocity = std::max<uint8_t>(velocity7(e.noteOn.velocity), 1);
                } else {
                    ev.kind = OP_EVENT_NOTE_OFF_ID;
                }
                addEvent(ev);
                break;
            case Event::kNoteOffEvent:
                if (e.noteOff.pitch < 0 || e.noteOff.pitch > 127) break;
                ev.kind = OP_EVENT_NOTE_OFF_ID;
                ev.channel = static_cast<uint8_t>(std::clamp<int>(e.noteOff.channel, 0, 15));
                ev.note = static_cast<uint8_t>(e.noteOff.pitch);
                ev.velocity = velocity7(e.noteOff.velocity);
                ev.index = noteId(e.noteOff.noteId);
                addEvent(ev);
                break;
            case Event::kNoteExpressionValueEvent: {
                // Addressed by the note id alone; an expression the plugin does
                // not list, or a value that is not a number, is dropped here.
                uint32_t id = 0;
                const NoteExpressionValueEvent& x = e.noteExpressionValue;
                if (!expressions_.idOf(x.typeId, id) || !std::isfinite(x.value) || x.noteId < 0) break;
                ev.kind = OP_EVENT_NOTE_EXPRESSION;
                ev.index = static_cast<uint32_t>(x.noteId);
                ev.midi[0] = static_cast<uint8_t>(id);
                ev.channel = OP_NOTE_ANY;
                ev.note = OP_NOTE_ANY;
                ev.value = static_cast<float>(expressions_.plain(id, x.value));
                addEvent(ev);
                break;
            }
            case Event::kPolyPressureEvent:
                if (e.polyPressure.pitch < 0 || e.polyPressure.pitch > 127) break;
                addMidi(at, static_cast<uint8_t>(0xA0 | std::clamp<int>(e.polyPressure.channel, 0, 15)), static_cast<uint8_t>(e.polyPressure.pitch),
                        velocity7(e.polyPressure.pressure));
                break;
            case Event::kLegacyMIDICCOutEvent: {
                const uint8_t ch = static_cast<uint8_t>(std::clamp<int>(e.midiCCOut.channel, 0, 15));
                const uint8_t v1 = static_cast<uint8_t>(e.midiCCOut.value & 0x7F);
                const uint8_t v2 = static_cast<uint8_t>(e.midiCCOut.value2 & 0x7F);
                const uint8_t number = e.midiCCOut.controlNumber;
                if (number < 128) addMidi(at, static_cast<uint8_t>(0xB0 | ch), number, v1);
                else if (number == kAfterTouch) addMidi(at, static_cast<uint8_t>(0xD0 | ch), v1, 0);
                else if (number == kPitchBend) addMidi(at, static_cast<uint8_t>(0xE0 | ch), v1, v2);
                else if (number == kCtrlProgramChange) addMidi(at, static_cast<uint8_t>(0xC0 | ch), v1, 0);
                else if (number == kCtrlPolyPressure) addMidi(at, static_cast<uint8_t>(0xA0 | ch), v1, v2);
                break;
            }
            default: break;
            }
            taken += nTagged_ - before;
        }
    }
    void collect(ProcessData& data, int32 numSamples) {
        nTagged_ = 0;
        nBypass_ = 0;
        order_ = 0;
        collectParams(data.inputParameterChanges, numSamples);
        collectNotes(data.inputEvents, numSamples);
        auto before = [](const Tagged& a, const Tagged& b) { return a.ev.offset != b.ev.offset ? a.ev.offset < b.ev.offset : a.order < b.order; };
        if (!std::is_sorted(tagged_.begin(), tagged_.begin() + nTagged_, before)) std::sort(tagged_.begin(), tagged_.begin() + nTagged_, before);
        auto bypassBefore = [](const BypassPoint& a, const BypassPoint& b) { return a.offset != b.offset ? a.offset < b.offset : a.order < b.order; };
        if (!std::is_sorted(bypassPoints_.begin(), bypassPoints_.begin() + nBypass_, bypassBefore))
            std::sort(bypassPoints_.begin(), bypassPoints_.begin() + nBypass_, bypassBefore);
    }
    // ProcessContext to the ABI transport. Quarter notes, tempo and signature
    // are passed when the host marks them valid. VST3 has no bar number and
    // no seconds: the bar start is the host's when valid, else the start of
    // the bar that holds the position under the current signature; the bar
    // number counts bars of that length from zero; seconds are samples over
    // the sample rate.
    void buildTransport(const ProcessContext& ctx) {
        OpTransport t{};
        if (ctx.state & ProcessContext::kPlaying) t.flags |= OP_TRANSPORT_PLAYING;
        if (ctx.state & ProcessContext::kRecording) t.flags |= OP_TRANSPORT_RECORDING;
        if ((ctx.state & ProcessContext::kTempoValid) && ctx.tempo > 0.0) {
            t.flags |= OP_TRANSPORT_HAS_TEMPO;
            t.tempo = ctx.tempo;
        }
        double barLength = 0.0;
        if ((ctx.state & ProcessContext::kTimeSigValid) && ctx.timeSigNumerator > 0 && ctx.timeSigDenominator > 0) {
            t.flags |= OP_TRANSPORT_HAS_SIGNATURE;
            t.numerator = static_cast<uint32_t>(ctx.timeSigNumerator);
            t.denominator = static_cast<uint32_t>(ctx.timeSigDenominator);
            barLength = 4.0 * ctx.timeSigNumerator / ctx.timeSigDenominator;
        }
        t.sample_pos = ctx.projectTimeSamples;
        const double rate = ctx.sampleRate > 0.0 ? ctx.sampleRate : processSetup.sampleRate;
        if (rate > 0.0) {
            t.flags |= OP_TRANSPORT_HAS_SECONDS;
            t.seconds = static_cast<double>(ctx.projectTimeSamples) / rate;
        }
        if ((ctx.state & ProcessContext::kProjectTimeMusicValid) && std::isfinite(ctx.projectTimeMusic)) {
            t.flags |= OP_TRANSPORT_HAS_BEATS;
            t.quarter_notes = ctx.projectTimeMusic;
            if ((ctx.state & ProcessContext::kBarPositionValid) && std::isfinite(ctx.barPositionMusic)) t.bar_start = ctx.barPositionMusic;
            else if (barLength > 0.0) t.bar_start = std::floor(ctx.projectTimeMusic / barLength) * barLength;
            if (barLength > 0.0) t.bar_number = static_cast<int64_t>(std::floor(t.bar_start / barLength + 1e-6));
        }
        transport_ = t;
    }
    // MIDI the plugin wrote in the chunk just processed, as VST3 events on the
    // output bus: notes as note on and off, polyphonic pressure as its event,
    // everything else as legacy MIDI CC events (the VST3 form of raw MIDI).
    void emitMidi(IEventList* out, int32 chunkStart) {
        if (!noteOut_) return;
        const uint32_t count = op_shell_midi_out(engine_, midi_, OP_MAX_MIDI_OUT);
        if (!out) return;
        for (uint32_t i = 0; i < count; ++i) {
            const OpMidiEvent& m = midi_[i];
            const uint8_t status = m.data[0];
            const uint8_t channel = status & 0x0F;
            Event e{};
            e.busIndex = 0;
            e.sampleOffset = chunkStart + static_cast<int32>(m.offset);
            e.ppqPosition = 0;
            e.flags = 0;
            switch (status & 0xF0) {
            case 0x90:
                if (m.data[2] != 0) {
                    e.type = Event::kNoteOnEvent;
                    e.noteOn.channel = channel;
                    e.noteOn.pitch = m.data[1];
                    e.noteOn.tuning = 0.0f;
                    e.noteOn.velocity = m.data[2] / 127.0f;
                    e.noteOn.length = 0;
                    e.noteOn.noteId = -1;
                    break;
                }
                [[fallthrough]];
            case 0x80:
                e.type = Event::kNoteOffEvent;
                e.noteOff.channel = channel;
                e.noteOff.pitch = m.data[1];
                e.noteOff.velocity = (status & 0xF0) == 0x80 ? m.data[2] / 127.0f : 0.0f;
                e.noteOff.noteId = -1;
                e.noteOff.tuning = 0.0f;
                break;
            case 0xA0:
                e.type = Event::kPolyPressureEvent;
                e.polyPressure.channel = channel;
                e.polyPressure.pitch = m.data[1];
                e.polyPressure.pressure = m.data[2] / 127.0f;
                e.polyPressure.noteId = -1;
                break;
            case 0xB0: legacy(e, m.data[1], channel, m.data[2], 0); break;
            case 0xC0: legacy(e, kCtrlProgramChange, channel, m.data[1], 0); break;
            case 0xD0: legacy(e, kAfterTouch, channel, m.data[1], 0); break;
            case 0xE0: legacy(e, kPitchBend, channel, m.data[1], m.data[2]); break;
            default: continue;
            }
            out->addEvent(e);
        }
    }
    static void legacy(Event& e, uint8_t number, uint8_t channel, uint8_t v1, uint8_t v2) {
        e.type = Event::kLegacyMIDICCOutEvent;
        e.midiCCOut.controlNumber = number;
        e.midiCCOut.channel = static_cast<int8>(channel);
        e.midiCCOut.value = static_cast<int8>(v1);
        e.midiCCOut.value2 = static_cast<int8>(v2);
    }
    // A block with no samples: the host only sets values. They reach the
    // engine and are held for the next block.
    void applyHeld(uint32_t from) {
        for (uint32_t i = from; i < nTagged_; ++i) {
            const OpEvent& ev = tagged_[i].ev;
            if (ev.kind != OP_EVENT_PARAM_NORMALIZED) continue;
            values_[ev.index].store(ev.value);
            if (engine_) op_shell_set_normalized(engine_, ev.index, ev.value);
        }
        if (nBypass_ > 0) bypass_.store(bypassPoints_[nBypass_ - 1].on);
    }

    // ---- bypass ------------------------------------------------------------
    // Pushes the input through a delay of the reported latency into dry_, so a
    // bypassed signal stays aligned with what the host compensates for. The
    // line always runs, so switching never starts from stale audio. Host
    // channel h of the output is bypassed to host channel h of the input;
    // outputs without an input are silent when bypassed.
    void captureDry(const AudioBusBuffers* input, int32 offset, uint32_t frames) {
        for (uint32_t h = 0; h < outCh_; ++h) {
            float* dry = dry_[h].data();
            if (!input || h >= inCh_) {
                std::memset(dry, 0, frames * sizeof(float));
                continue;
            }
            const float* in = input->channelBuffers32[h] + offset;
            if (delay_ == 0) {
                std::memcpy(dry, in, frames * sizeof(float));
                continue;
            }
            float* line = line_[h].data();
            size_t pos = linePos_;
            for (uint32_t i = 0; i < frames; ++i) {
                dry[i] = line[pos];
                line[pos] = in[i];
                if (++pos == delay_) pos = 0;
            }
        }
        if (delay_ != 0) linePos_ = (linePos_ + frames) % delay_;
    }
    // Crossfades the engine output towards the dry signal (or back). Fully
    // bypassed output is the dry signal exactly.
    void applyBypass(AudioBusBuffers& output, int32 offset, uint32_t frames, float target) {
        if (mix_ == target) {
            if (target == 0.0f) return;
            for (uint32_t h = 0; h < outCh_; ++h) std::memcpy(output.channelBuffers32[h] + offset, dry_[h].data(), frames * sizeof(float));
            return;
        }
        float mix = mix_;
        for (uint32_t i = 0; i < frames; ++i) {
            if (mix < target) mix = std::min(target, mix + mixStep_);
            else if (mix > target) mix = std::max(target, mix - mixStep_);
            for (uint32_t h = 0; h < outCh_; ++h) {
                float& sample = output.channelBuffers32[h][offset + i];
                sample = mix >= 1.0f ? dry_[h][i] : sample + (dry_[h][i] - sample) * mix;
            }
        }
        mix_ = mix;
    }
    // Allocates everything process needs; process never allocates.
    void prepareBypass() {
        delay_ = engine_ ? op_shell_latency(engine_) : 0;
        const size_t block = static_cast<size_t>(std::max(processSetup.maxSamplesPerBlock, 1));
        paramCapacity_ = static_cast<uint32_t>(std::max<size_t>(kMinChangeCapacity, static_cast<size_t>(count_) + 1));
        tagged_.assign(paramCapacity_ + kNoteCapacity, Tagged{});
        bypassPoints_.assign(paramCapacity_, BypassPoint{0, 0, false});
        dry_.assign(outCh_, std::vector<float>(block, 0.0f));
        line_.assign(outCh_, std::vector<float>());
        for (uint32_t h = 0; h < outCh_ && h < inCh_; ++h) line_[h].assign(delay_, 0.0f);
        linePos_ = 0;
        mixStep_ = static_cast<float>(1.0 / std::max(1.0, kBypassFadeSeconds * processSetup.sampleRate));
        mix_ = bypass_.load() ? 1.0f : 0.0f;
    }
    void clearBypassLine() {
        for (auto& line : line_) std::fill(line.begin(), line.end(), 0.0f);
        linePos_ = 0;
    }
    // ---- telling the host about changes (ABI 6) -----------------------------
    // A main-thread timer runs while an engine exists. Each tick asks the
    // engine what changed and hands it to the controller, which owns the host's
    // IComponentHandler and calls restartComponent. Never on the audio thread.
    void onTimer(Timer*) SMTG_OVERRIDE { pollChanges(); }
    void pollChanges() {
        if (!engine_) return;
        const uint32_t flags = op_shell_take_changes(engine_) & (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO);
        if (flags == 0) return;
        if (IPtr<IMessage> message = owned(allocateMessage())) {
            message->setMessageID(kChangesMessage);
            message->getAttributes()->setInt("flags", static_cast<int64>(flags));
            sendMessage(message);
        }
    }
    void startPolling() {
        if (!timer_) timer_ = Timer::create(this, kChangesPollMs);
    }
    void stopPolling() {
        if (timer_) {
            timer_->stop();
            timer_->release();
            timer_ = nullptr;
        }
    }
    Timer* timer_ = nullptr;
    // Tells the controller which engine an open editor should show. The pointer
    // only means something in this process, and the editor checks it is live.
    void announce(void* engine) {
#if OP_HAS_EDITOR
        if (IPtr<IMessage> message = owned(allocateMessage())) {
            message->setMessageID(kEngineMessage);
            message->getAttributes()->setInt("engine", static_cast<int64>(reinterpret_cast<intptr_t>(engine)));
            sendMessage(message);
        }
#else
        (void)engine;
#endif
    }
    // Gives the host's track to the engine, if there is one and the host has told.
    void applyTrack() {
        if (engine_ && track_.valid) op_shell_set_track_info(engine_, track_.nameOrNull(), track_.rgba, track_.flags);
    }
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    void* engine_ = nullptr;
    TrackState track_;
    uint32_t count_ = 0;
    ParamIds ids_;
    // The normalized default of each parameter: what a state that does not name it leaves.
    std::vector<double> defaults_;
    uint32_t channels_ = 2;  // engine bus width given to op_shell_create (the legacy path)
    std::unique_ptr<std::atomic<double>[]> values_;
    std::atomic<bool> bypass_{false};

    // Layouts as VST3 arrangements, the buses they imply, and the selection.
    
    std::vector<Entry> layouts_;
    bool hasIn_ = false, hasOut_ = false, hasSide_ = false;
    bool noteIn_ = false, noteOut_ = false;
    ShellExpressions expressions_;
    uint32_t layoutIndex_ = 0;
    uint32_t inCh_ = 0, outCh_ = 0, sideCh_ = 0;
    uint8_t inMap_[OP_MAX_CHANNELS] = {}, outMap_[OP_MAX_CHANNELS] = {}, sideMap_[OP_MAX_CHANNELS] = {};

    // Everything below is touched only by the audio thread after setActive.
    struct Tagged {
        OpEvent ev;
        uint32_t order;
    };
    struct BypassPoint {
        int32 offset;
        uint32_t order;
        bool on;
    };
    // Room for this many parameter points per block; a block never allocates.
    // One point per parameter always fits.
    static constexpr uint32_t kMinChangeCapacity = 4096;
    static constexpr uint32_t kNoteCapacity = OP_MAX_EVENTS;
    std::vector<Tagged> tagged_;
    std::vector<BypassPoint> bypassPoints_;
    uint32_t paramCapacity_ = 0, nTagged_ = 0, nBypass_ = 0, order_ = 0;
    OpEvent chunk_[OP_MAX_EVENTS] = {};
    OpMidiEvent midi_[OP_MAX_MIDI_OUT] = {};
    OpTransport transport_{};
    const float* inPtr_[OP_MAX_CHANNELS] = {};
    float* outPtr_[OP_MAX_CHANNELS] = {};
    const float* sidePtr_[OP_MAX_CHANNELS] = {};
    bool offline_ = false;
    std::vector<std::vector<float>> dry_;
    std::vector<std::vector<float>> line_;
    size_t delay_ = 0;
    size_t linePos_ = 0;
    float mix_ = 0.0f;
    float mixStep_ = 1.0f;
};

class Controller final : public EditController,
                         public IMidiMapping,
                         public ChannelContext::IInfoListener,
                         public IUnitInfo,
                         public INoteExpressionController,
                         public INoteExpressionPhysicalUIMapping {
public:
    static FUnknown* create(void*) { return static_cast<IEditController*>(new Controller); }
    // The units and the note expressions come from the library's static
    // description, so a host may ask for them before initialize.
    Controller() {
        OpShellInfo info{};
        if (load_info(info)) {
            units_ = shell_units(info.param_count);
            if (op_shell_note_ports() & OP_NOTE_PORT_INPUT) expressions_.load();
        }
    }
    // ---- INoteExpressionController (ABI 7): the expressions the plugin lists,
    // the same on every channel of the event bus; offered only when it lists
    // some, so a plugin without them is what it was.
    bool expressionBus(int32 busIndex, int16 channel) const { return busIndex == 0 && channel >= 0 && channel < 16 && expressions_.count > 0; }
    int32 PLUGIN_API getNoteExpressionCount(int32 busIndex, int16 channel) SMTG_OVERRIDE {
        return expressionBus(busIndex, channel) ? static_cast<int32>(expressions_.count) : 0;
    }
    tresult PLUGIN_API getNoteExpressionInfo(int32 busIndex, int16 channel, int32 index, NoteExpressionTypeInfo& out) SMTG_OVERRIDE {
        if (!expressionBus(busIndex, channel) || index < 0 || index >= static_cast<int32>(expressions_.count)) return kInvalidArgument;
        const uint32_t id = expressions_.order[index];
        const OpExpressionInfo& info = expressions_.info[id];
        std::memset(&out, 0, sizeof(out));
        out.typeId = ShellExpressions::typeOf(id);
        utf8_to_utf16(info.name, std::strlen(info.name), out.title, 128);
        utf8_to_utf16(info.short_name, std::strlen(info.short_name), out.shortTitle, 128);
        utf8_to_utf16(info.unit, std::strlen(info.unit), out.units, 128);
        out.unitId = kNoParentUnitId;
        out.valueDesc.minimum = 0.0;
        out.valueDesc.maximum = 1.0;
        out.valueDesc.defaultValue = expressions_.normalized(id, info.default_value);
        out.valueDesc.stepCount = 0;
        out.associatedParameterId = kNoParamId;
        out.flags = (info.flags & OP_EXPRESSION_BIPOLAR) ? NoteExpressionTypeInfo::kIsBipolar : 0;
        return kResultOk;
    }
    tresult PLUGIN_API getNoteExpressionStringByValue(int32 busIndex, int16 channel, NoteExpressionTypeID type, NoteExpressionValue normalized,
                                                      String128 string) SMTG_OVERRIDE {
        uint32_t id = 0;
        if (!string || !expressionBus(busIndex, channel) || !expressions_.idOf(type, id) || !std::isfinite(normalized)) return kInvalidArgument;
        char text[128];
        const size_t length = op_shell_expression_format(id, expressions_.plain(id, normalized), text, sizeof(text));
        utf8_to_utf16(text, std::min(length, sizeof(text) - 1), string, 128);
        return kResultOk;
    }
    tresult PLUGIN_API getNoteExpressionValueByString(int32 busIndex, int16 channel, NoteExpressionTypeID type, const TChar* string,
                                                      NoteExpressionValue& normalized) SMTG_OVERRIDE {
        uint32_t id = 0;
        if (!string || !expressionBus(busIndex, channel) || !expressions_.idOf(type, id)) return kInvalidArgument;
        double value = 0.0;
        if (!op_shell_expression_parse(id, utf16_to_utf8(string).c_str(), &value)) return kResultFalse;
        normalized = expressions_.normalized(id, value);
        return kResultOk;
    }
    // ---- INoteExpressionPhysicalUIMapping: an MPE-style controller's X
    // movement drives tuning, Y movement brightness and pressure pressure,
    // where the plugin lists them.
    tresult PLUGIN_API getPhysicalUIMapping(int32 busIndex, int16 channel, PhysicalUIMapList& list) SMTG_OVERRIDE {
        if (!expressionBus(busIndex, channel) || (list.count > 0 && !list.map)) return kInvalidArgument;
        for (uint32 i = 0; i < list.count; ++i) {
            uint32_t id = OP_EXPRESSION_COUNT;
            switch (list.map[i].physicalUITypeID) {
            case kPUIXMovement: id = OP_EXPRESSION_TUNING; break;
            case kPUIYMovement: id = OP_EXPRESSION_BRIGHTNESS; break;
            case kPUIPressure: id = OP_EXPRESSION_PRESSURE; break;
            default: break;
            }
            list.map[i].noteExpressionTypeID = expressions_.has(id) ? ShellExpressions::typeOf(id) : kInvalidTypeID;
        }
        return kResultOk;
    }
    // ---- IUnitInfo: one unit per group (see shell_units); offered only when
    // the plugin has groups, so a plugin without them is what it was. No
    // program lists.
    int32 PLUGIN_API getUnitCount() SMTG_OVERRIDE { return static_cast<int32>(units_.units.size()); }
    tresult PLUGIN_API getUnitInfo(int32 unitIndex, UnitInfo& info) SMTG_OVERRIDE {
        if (unitIndex < 0 || unitIndex >= getUnitCount()) return kInvalidArgument;
        const ShellUnit& unit = units_.units[static_cast<size_t>(unitIndex)];
        info.id = unit.id;
        info.parentUnitId = unit.parent;
        std::memset(info.name, 0, sizeof(info.name));
        utf8_to_utf16(unit.name.data(), unit.name.size(), info.name, 128);
        info.programListId = kNoProgramListId;
        return kResultOk;
    }
    int32 PLUGIN_API getProgramListCount() SMTG_OVERRIDE { return 0; }
    tresult PLUGIN_API getProgramListInfo(int32, ProgramListInfo&) SMTG_OVERRIDE { return kResultFalse; }
    tresult PLUGIN_API getProgramName(ProgramListID, int32, String128) SMTG_OVERRIDE { return kResultFalse; }
    tresult PLUGIN_API getProgramInfo(ProgramListID, int32, Steinberg::Vst::CString, String128) SMTG_OVERRIDE { return kResultFalse; }
    tresult PLUGIN_API hasProgramPitchNames(ProgramListID, int32) SMTG_OVERRIDE { return kResultFalse; }
    tresult PLUGIN_API getProgramPitchName(ProgramListID, int32, int16, String128) SMTG_OVERRIDE { return kResultFalse; }
    UnitID PLUGIN_API getSelectedUnit() SMTG_OVERRIDE { return selectedUnit_; }
    tresult PLUGIN_API selectUnit(UnitID unitId) SMTG_OVERRIDE {
        if (unitId < 0 || unitId >= getUnitCount()) return kResultFalse;
        selectedUnit_ = unitId;
        return kResultTrue;
    }
    tresult PLUGIN_API getUnitByBus(MediaType, BusDirection, int32, int32, UnitID&) SMTG_OVERRIDE { return kResultFalse; }
    tresult PLUGIN_API setUnitProgramData(int32, int32, IBStream*) SMTG_OVERRIDE { return kResultFalse; }
    // The host's channel: name and colour (kind is not part of VST3's channel
    // context, so the flags stay 0). A call lists what the host knows; a key
    // that is absent keeps what was said before. The processor gets it as a
    // message (the plugin object is its engine), the open editor directly.
    tresult PLUGIN_API setChannelContextInfos(IAttributeList* list) SMTG_OVERRIDE {
        if (!list) return kInvalidArgument;
        TChar name[256] = {};
        if (list->getString(ChannelContext::kChannelNameKey, name, sizeof(name)) == kResultTrue) track_.name = utf8_from_tchar(name);
        int64 color = 0;
        if (list->getInt(ChannelContext::kChannelColorKey, color) == kResultTrue) {
            // ARGB (ChannelContext::GetAlpha and friends) to 0xRRGGBBAA.
            const uint32_t argb = static_cast<uint32_t>(color);
            track_.rgba = (argb << 8) | (argb >> 24);
            track_.flags |= OP_TRACK_HAS_COLOUR;
        }
        track_.valid = true;
        sendTrack();
        return kResultOk;
    }
    // A host may tell the track before it connects the two halves.
    tresult PLUGIN_API connect(IConnectionPoint* other) SMTG_OVERRIDE {
        const tresult result = EditController::connect(other);
        if (result == kResultOk && track_.valid) sendTrack();
        return result;
    }
    // MIDI controllers, channel pressure and pitch bend arrive as hidden
    // parameters (see kMidiParamBase). Only the first event bus maps.
    tresult PLUGIN_API getMidiControllerAssignment(int32 busIndex, int16 channel, CtrlNumber number, ParamID& id) SMTG_OVERRIDE {
        if (!midiIn_ || busIndex != 0 || channel < 0 || channel > 15 || number < 0 || number >= static_cast<CtrlNumber>(kMidiControllers))
            return kResultFalse;
        id = kMidiParamBase + static_cast<ParamID>(channel) * kMidiControllers + static_cast<ParamID>(number);
        return kResultTrue;
    }
    OBJ_METHODS(Controller, EditController)
    DEFINE_INTERFACES
        DEF_INTERFACE(IMidiMapping)
        DEF_INTERFACE(ChannelContext::IInfoListener)
        if (!units_.units.empty()) {
            DEF_INTERFACE(IUnitInfo)
        }
        if (expressions_.count > 0) {
            DEF_INTERFACE(INoteExpressionController)
            DEF_INTERFACE(INoteExpressionPhysicalUIMapping)
        }
    END_DEFINE_INTERFACES(EditController)
    REFCOUNT_METHODS(EditController)
    tresult PLUGIN_API initialize(FUnknown* context) SMTG_OVERRIDE {
        tresult result = EditController::initialize(context);
        if (result != kResultOk) return result;
        OpShellInfo info{};
        if (!load_info(info)) return kResultFalse;
        ids_.load(info.param_count);
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            if (!op_shell_param(i, &desc)) return kResultFalse;
            String128 title{};
            String128 unit{};
            UString(title, 128).fromAscii(desc.name);
            UString(unit, 128).fromAscii(desc.unit);
            int32 steps = desc.steps > 1 ? static_cast<int32>(desc.steps - 1) : 0;
            int32 flags = (desc.flags & OP_PARAM_AUTOMATABLE) ? ParameterInfo::kCanAutomate : 0;
            auto* parameter = new ShellParameter(title, unit, ids_.id(i), i, desc.default_normalized, steps, flags);
            if (i < units_.unitOf.size()) parameter->setUnitID(units_.unitOf[i]);
            parameters.addParameter(parameter);
        }
        count_ = info.param_count;
        parameters.addParameter(new BypassParameter(kBypassId));
        midiIn_ = (op_shell_note_ports() & OP_NOTE_PORT_INPUT) != 0;
        if (midiIn_) {
            for (uint32_t channel = 0; channel < 16; ++channel) {
                for (uint32_t number = 0; number < kMidiControllers; ++number) {
                    char name[40];
                    if (number < 128) std::snprintf(name, sizeof(name), "MIDI Ch %u CC %u", channel + 1, number);
                    else std::snprintf(name, sizeof(name), "MIDI Ch %u %s", channel + 1, number == kAfterTouch ? "Pressure" : "Pitch Bend");
                    String128 title{};
                    UString(title, 128).fromAscii(name);
                    parameters.addParameter(title, nullptr, 0, number == kPitchBend ? 0.5 : 0.0, ParameterInfo::kIsHidden,
                                            kMidiParamBase + channel * kMidiControllers + number);
                }
            }
        }
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        // The same head the processor reads; nothing is set unless all of it reads.
        std::vector<double> loaded(count_);
        for (uint32_t i = 0; i < count_; ++i) {
            OpParamDesc desc{};
            loaded[i] = op_shell_param(i, &desc) ? desc.default_normalized : 0.0;
        }
        if (!read_state_head(reader, ids_, loaded, false)) return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) setParamNormalized(ids_.id(i), loaded[i]);
        // The plugin-state trailer after the flag belongs to the processor.
        int32 bypass = 0;
        if (!reader.readInt32(bypass)) bypass = 0;
        setParamNormalized(kBypassId, bypass != 0 ? 1.0 : 0.0);
        return kResultOk;
    }
    // The processor's messages: what changed (ABI 6) and, with an editor, the engine.
    tresult PLUGIN_API notify(IMessage* message) SMTG_OVERRIDE {
        const char* id = message ? message->getMessageID() : nullptr;
        if (id && std::strcmp(id, kChangesMessage) == 0) {
            int64 flags = 0;
            if (!message->getAttributes() || message->getAttributes()->getInt("flags", flags) != kResultOk) return kInvalidArgument;
            tellHost(static_cast<uint32_t>(flags));
            return kResultOk;
        }
#if OP_HAS_EDITOR
        if (id && std::strcmp(id, kEngineMessage) == 0) {
            int64 value = 0;
            if (message->getAttributes()->getInt("engine", value) == kResultOk) {
                engine_ = reinterpret_cast<void*>(static_cast<intptr_t>(value));
                if (editor_) op_editor_bind(editor_, engine_);
            }
            return kResultOk;
        }
#endif
        return EditController::notify(message);
    }
    // The plugin's latency or parameter info changed: one restartComponent with
    // every flag it needs. A new latency makes the host deactivate and activate
    // the component and read getLatencySamples; new parameter info makes it
    // read getParameterInfo and the values again. UI thread.
    void tellHost(uint32_t flags) {
        int32 restart = 0;
        if (flags & OP_CHANGE_LATENCY) restart |= kLatencyChanged;
        if (flags & OP_CHANGE_PARAM_INFO) restart |= kParamTitlesChanged | kParamValuesChanged;
        if (restart && componentHandler) componentHandler->restartComponent(restart);
    }
#if OP_HAS_EDITOR
    IPlugView* PLUGIN_API createView(FIDString name) SMTG_OVERRIDE;
    tresult PLUGIN_API setParamNormalized(ParamID tag, ParamValue value) SMTG_OVERRIDE {
        const tresult result = EditController::setParamNormalized(tag, value);
        if (const int32 index = ids_.index(tag); editor_ && result == kResultOk && index >= 0) {
            const auto at = static_cast<uint32_t>(index);
            op_editor_set_plain(editor_, at, static_cast<float>(op_shell_to_plain(at, value)));
        }
        return result;
    }
    // Called by the view.
    void* openEditor(void* parent, void* frameContext, OpEditorRequestResize request, OpEditorContextMenu menu) {
        closeEditor();
        OpEditorHost host{this, hostBegin, hostPerform, hostEnd};
        editor_ = op_editor_create(&host);
        if (!editor_) return nullptr;
        for (uint32_t i = 0; i < count_; ++i) {
            op_editor_set_plain(editor_, i, static_cast<float>(op_shell_to_plain(i, getParamNormalized(ids_.id(i)))));
        }
        op_editor_bind(editor_, engine_);
        op_editor_set_request_resize(editor_, frameContext, request);
        op_editor_set_context_menu(editor_, frameContext, menu);
        if (track_.valid) op_editor_set_track_info(editor_, track_.nameOrNull(), track_.rgba, track_.flags);
        if (!op_editor_attach(editor_, parent)) {
            closeEditor();
            return nullptr;
        }
        owner_ = frameContext;
        return editor_;
    }
    void* editor() const { return editor_; }
    // The editor asks for the host's context menu on parameter `index`: the
    // host's IComponentHandler3 builds it (automation, MIDI learn) and pops
    // it up at (x, y) in `view`. False when the host has none or cannot show it.
    bool popupHostMenu(IPlugView* view, ParamID index, int32 x, int32 y) {
        if (!componentHandler || !view) return false;
        FUnknownPtr<IComponentHandler3> handler(componentHandler);
        if (!handler) return false;
        if (index >= count_) return false;
        ParamID id = ids_.id(index);
        IContextMenu* menu = handler->createContextMenu(view, &id);
        if (!menu) return false;
        const tresult shown = menu->popup(x, y);
        menu->release();
        return shown == kResultOk;
    }
    // The editor belongs to the view that opened it last. A host that opens a
    // second view replaces the first view's editor, so the first view must not
    // close, resize or read the second view's editor.
    void* editorOf(const void* view) const { return editor_ && owner_ == view ? editor_ : nullptr; }
    void closeEditorOf(const void* view) {
        if (editor_ && owner_ == view) closeEditor();
    }
    void closeEditor() {
        if (editor_) {
            op_editor_destroy(editor_);
            editor_ = nullptr;
        }
        owner_ = nullptr;
    }

private:
    // The editor names a parameter by its ABI index; the host by its ID.
    static void hostBegin(void* context, uint32_t index) {
        auto* self = static_cast<Controller*>(context);
        if (index < self->count_) self->EditController::beginEdit(self->ids_.id(index));
    }
    static void hostPerform(void* context, uint32_t index, double plain) {
        auto* self = static_cast<Controller*>(context);
        if (index >= self->count_) return;
        const ParamValue normalized = op_shell_to_normalized(index, plain);
        self->EditController::setParamNormalized(self->ids_.id(index), normalized);
        self->EditController::performEdit(self->ids_.id(index), normalized);
    }
    static void hostEnd(void* context, uint32_t index) {
        auto* self = static_cast<Controller*>(context);
        if (index < self->count_) self->EditController::endEdit(self->ids_.id(index));
    }
    void* editor_ = nullptr;
    const void* owner_ = nullptr;
    void* engine_ = nullptr;
#else
    IPlugView* PLUGIN_API createView(FIDString) SMTG_OVERRIDE { return nullptr; }
#endif

private:
    // Tell the processor (by message) and the open editor what the host said.
    void sendTrack() {
        if (IPtr<IMessage> message = owned(allocateMessage())) {
            message->setMessageID(kTrackMessage);
            IAttributeList* attributes = message->getAttributes();
            if (!track_.name.empty()) {
                TChar name[256] = {};
                tchar_from_utf8(track_.name, name, 256);
                attributes->setString("name", name);
            }
            attributes->setInt("rgba", static_cast<int64>(track_.rgba));
            attributes->setInt("flags", static_cast<int64>(track_.flags));
            sendMessage(message);
        }
#if OP_HAS_EDITOR
        if (editor_) op_editor_set_track_info(editor_, track_.nameOrNull(), track_.rgba, track_.flags);
#endif
    }
    TrackState track_;
    uint32_t count_ = 0;
    ParamIds ids_;
    bool midiIn_ = false;
    ShellUnits units_;
    UnitID selectedUnit_ = kRootUnitId;
    ShellExpressions expressions_;
};

#if OP_HAS_EDITOR
// The NSView host. The controller owns the editor, so a view that outlives its
// window still closes cleanly. Not final, and its helpers are protected: a platform
// file that includes this one may derive its own view and define
// Controller::createView itself (OP_VST3_OWN_EDITOR_VIEW; native/vst3/win/OpenPluginVST3Win.cpp
// adds IPlugViewContentScaleSupport that way).
class EditorView : public CPluginView {
public:
    explicit EditorView(Controller* controller) : controller_(controller) {
        uint32_t width = 0;
        uint32_t height = 0;
        op_editor_size(&width, &height);
        rect = ViewRect(0, 0, static_cast<int32>(width), static_cast<int32>(height));
        if (!op_editor_size_range(&minW_, &minH_, &maxW_, &maxH_, &aspectW_, &aspectH_)) {
            minW_ = maxW_ = width;
            minH_ = maxH_ = height;
        }
    }
    ~EditorView() override {
        if (open_ && controller_) controller_->closeEditorOf(this);
    }
    tresult PLUGIN_API isPlatformTypeSupported(FIDString type) SMTG_OVERRIDE {
        return type && std::strcmp(type, kPlatformTypeNSView) == 0 ? kResultTrue : kResultFalse;
    }
    tresult PLUGIN_API attached(void* parent, FIDString type) SMTG_OVERRIDE {
        if (!parent || isPlatformTypeSupported(type) != kResultTrue || !controller_) return kResultFalse;
        if (!controller_->openEditor(parent, this, requestResize, showHostMenu)) return kResultFalse;
        open_ = true;
        // The editor opens at its own size; bring it to the size the host last set.
        resizeEditor(static_cast<uint32_t>(rect.getWidth()), static_cast<uint32_t>(rect.getHeight()));
        return CPluginView::attached(parent, type);
    }
    tresult PLUGIN_API removed() SMTG_OVERRIDE {
        if (open_ && controller_) controller_->closeEditorOf(this);
        open_ = false;
        return CPluginView::removed();
    }
    tresult PLUGIN_API canResize() SMTG_OVERRIDE { return minW_ == maxW_ && minH_ == maxH_ ? kResultFalse : kResultTrue; }
    tresult PLUGIN_API getSize(ViewRect* size) SMTG_OVERRIDE {
        if (!size) return kInvalidArgument;
        uint32_t width = 0;
        uint32_t height = 0;
        if (void* editor = ownEditor(); editor && op_editor_get_size(editor, &width, &height)) {
            rect = ViewRect(rect.left, rect.top, rect.left + static_cast<int32>(width), rect.top + static_cast<int32>(height));
        }
        *size = rect;
        return kResultOk;
    }
    tresult PLUGIN_API onSize(ViewRect* newSize) SMTG_OVERRIDE {
        if (!newSize) return kInvalidArgument;
        uint32_t width = static_cast<uint32_t>(newSize->getWidth() > 0 ? newSize->getWidth() : 1);
        uint32_t height = static_cast<uint32_t>(newSize->getHeight() > 0 ? newSize->getHeight() : 1);
        adjust(width, height);
        // Without an editor yet, attached() applies this size.
        rect = ViewRect(newSize->left, newSize->top, newSize->left + static_cast<int32>(width), newSize->top + static_cast<int32>(height));
        resizeEditor(width, height);
        return kResultOk;
    }
    tresult PLUGIN_API checkSizeConstraint(ViewRect* size) SMTG_OVERRIDE {
        if (!size) return kInvalidArgument;
        uint32_t width = static_cast<uint32_t>(size->getWidth() > 0 ? size->getWidth() : 1);
        uint32_t height = static_cast<uint32_t>(size->getHeight() > 0 ? size->getHeight() : 1);
        adjust(width, height);
        size->right = size->left + static_cast<int32>(width);
        size->bottom = size->top + static_cast<int32>(height);
        return kResultOk;
    }

protected:
    // This view's editor, or null when it is closed or another view replaced it.
    void* ownEditor() const { return open_ && controller_ ? controller_->editorOf(this) : nullptr; }
    // The editor's own adjustment when it is open. Before that only the
    // bounds are known, so the aspect is not applied until the editor opens.
    void adjust(uint32_t& width, uint32_t& height) const {
        if (void* editor = ownEditor(); editor && op_editor_adjust_size(editor, &width, &height)) return;
        width = width < minW_ ? minW_ : (width > maxW_ ? maxW_ : width);
        height = height < minH_ ? minH_ : (height > maxH_ ? maxH_ : height);
    }
    void resizeEditor(uint32_t width, uint32_t height) {
        if (void* editor = ownEditor()) op_editor_resize(editor, width, height);
    }
    // The editor asks for a size: the frame decides, then the editor takes it.
    static uint32_t requestResize(void* context, uint32_t width, uint32_t height) {
        auto* self = static_cast<EditorView*>(context);
        if (!self->plugFrame) return 0;
        ViewRect asked(self->rect.left, self->rect.top, self->rect.left + static_cast<int32>(width), self->rect.top + static_cast<int32>(height));
        if (self->plugFrame->resizeView(self, &asked) != kResultTrue) return 0;
        self->rect = asked;
        self->resizeEditor(width, height);
        return 1;
    }
    // The editor asks for the host's parameter menu (right-click, Control-click).
    static uint32_t showHostMenu(void* context, uint32_t index, int32_t x, int32_t y) {
        auto* self = static_cast<EditorView*>(context);
        return self->controller_ && self->controller_->popupHostMenu(static_cast<IPlugView*>(self), index, x, y) ? 1 : 0;
    }
    IPtr<Controller> controller_;
    bool open_ = false;
    uint32_t minW_ = 0, minH_ = 0, maxW_ = 0, maxH_ = 0, aspectW_ = 0, aspectH_ = 0;
};

#ifndef OP_VST3_OWN_EDITOR_VIEW
IPlugView* PLUGIN_API Controller::createView(FIDString name) {
    if (!name || std::strcmp(name, ViewType::kEditor) != 0) return nullptr;
    return new EditorView(this);
}
#endif
#endif
}

bool InitModule() { return true; }
bool DeinitModule() { return true; }

extern "C" SMTG_EXPORT_SYMBOL IPluginFactory* GetPluginFactory() {
    if (!gPluginFactory) {
        OpShellInfo info{};
        if (!openplugin_vst::load_info(info)) return nullptr;
        PFactoryInfo factoryInfo(info.vendor, "", "", PFactoryInfo::kNoFlags);
        gPluginFactory = new CPluginFactory(factoryInfo);
        TUID processor{};
        TUID controller{};
        std::memcpy(processor, info.processor_cid, 16);
        std::memcpy(controller, info.controller_cid, 16);
        const std::string category = openplugin_vst::category_for(info.category, op_shell_kind());
        PClassInfo2 processorClass(processor, PClassInfo::kManyInstances, kVstAudioEffectClass, info.name, kDistributable, category.c_str(), info.vendor, info.version, kVstVersionString);
        PClassInfo2 controllerClass(controller, PClassInfo::kManyInstances, kVstComponentControllerClass, info.name, 0, "", info.vendor, info.version, kVstVersionString);
        gPluginFactory->registerClass(&processorClass, openplugin_vst::Processor::create);
        gPluginFactory->registerClass(&controllerClass, openplugin_vst::Controller::create);
    } else {
        gPluginFactory->addRef();
    }
    return gPluginFactory;
}
