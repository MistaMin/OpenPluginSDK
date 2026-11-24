// SPDX-License-Identifier: MIT
// AAX native process callback on shell ABI 2.
//
// Every block goes through op_shell_process_events:
//  - Plain parameter values arrive in the controls packet. AAX gives a packet
//    no sample offset (a data port is applied when its callback starts), so a
//    value that differs from the engine's becomes a parameter event at offset 0,
//    the start of the block. Non-finite values silence the block.
//  - MIDI from the local input node becomes note and MIDI events at the
//    packet's own timestamp (a sample offset in the buffer). MIDI the plugin
//    produces is posted to the local output node at its offset.
//  - The transport is read from the transport node (AAX_ITransport, version 1
//    methods only) once per callback, with no lock, and passed with the block.
//  - A sidechain, when the host connects one, is the extra input channel that
//    the side-chain field points at.
//  - Channels are reordered at the edge: the engine sees the SDK's canonical
//    order, the host's buffers stay in Pro Tools' film order. Only pointers
//    move; no sample is copied for it.
// Bypass outputs the input delayed by the reported latency, crossfaded over
// 10 ms, and the engine keeps running while bypassed.
#include "OpenPluginAAX.h"
#include <algorithm>
#include <cmath>
#include <cstring>
namespace openplugin_aax {
namespace {
// CLAP surround ids, the SDK's canonical roles (openplugin_core::layout).
constexpr uint8_t FL = 0, FR = 1, FC = 2, LFE = 3, BL = 4, BR = 5, BC = 8, SL = 9, SR = 10, TFL = 12, TFR = 14, TBL = 15, TBR = 17,
                  TSL = 18, TSR = 19, FLW = 20, FRW = 21;  // SDK-only roles (layout.rs): top side (Ltm/Lts) and front wides (Lw/Rw)
constexpr uint8_t NO = OP_ROLE_NONE;
// The order of each row is the AAX order from the comments on AAX_EStemFormat in
// AAX_Enums.h of AAX SDK 2.8.1: Pro Tools' film order, L C R Ls Rs LFE for 5.1,
// L C R Lss Rss Lsr Rsr LFE for 7.1 and L C R Lss Rss Lsr Rsr LFE Ltf Rtf Ltr Rtr
// for 7.1.4. Ls and Rs are the SDK's back pair (BL, BR) on 5.x; Lss and Rss are
// the side pair (SL, SR) and Lsr and Rsr the back pair on 7.x. The LFE sits last
// on 5.1 and eighth on 7.1, not fourth as in the SDK order. The Atmos stems keep
// the same rule: the top pairs follow the LFE (Ltf Rtf, then Ltm Rtm or Lts Rts,
// then Ltr Rtr) and the 9.x stems put the wides (Lw Rw) after R. The top-middle
// and top-side pairs (Ltm Rtm, Lts Rts) are the SDK's top side roles TSL and TSR,
// Lw and Rw are FLW and FRW. A stem with a channel that has no role (SDDS Lc and
// Rc) is not in the table, so a bus that needs it is not offered.
const Stem kStems[] = {
    {AAX_eStemFormat_Mono, OP_BUS_MONO, 1, {NO}, "mono"},
    {AAX_eStemFormat_Stereo, OP_BUS_STEREO, 2, {FL, FR}, "stereo"},
    {AAX_eStemFormat_LCR, OP_BUS_SURROUND, 3, {FL, FC, FR}, "LCR"},
    {AAX_eStemFormat_Quad, OP_BUS_SURROUND, 4, {FL, FR, BL, BR}, "quad"},
    {AAX_eStemFormat_5_0, OP_BUS_SURROUND, 5, {FL, FC, FR, BL, BR}, "5.0"},
    {AAX_eStemFormat_5_1, OP_BUS_SURROUND, 6, {FL, FC, FR, BL, BR, LFE}, "5.1"},
    {AAX_eStemFormat_6_1, OP_BUS_SURROUND, 7, {FL, FC, FR, BL, BC, BR, LFE}, "6.1"},
    {AAX_eStemFormat_7_1_DTS, OP_BUS_SURROUND, 8, {FL, FC, FR, SL, SR, BL, BR, LFE}, "7.1"},
    {AAX_eStemFormat_5_1_4, OP_BUS_SURROUND, 10, {FL, FC, FR, BL, BR, LFE, TFL, TFR, TBL, TBR}, "5.1.4"},
    {AAX_eStemFormat_7_1_4, OP_BUS_SURROUND, 12, {FL, FC, FR, SL, SR, BL, BR, LFE, TFL, TFR, TBL, TBR}, "7.1.4"},
    {AAX_eStemFormat_5_0_2, OP_BUS_SURROUND, 7, {FL, FC, FR, BL, BR, TSL, TSR}, "5.0.2"},
    {AAX_eStemFormat_5_1_2, OP_BUS_SURROUND, 8, {FL, FC, FR, BL, BR, LFE, TSL, TSR}, "5.1.2"},
    {AAX_eStemFormat_5_0_4, OP_BUS_SURROUND, 9, {FL, FC, FR, BL, BR, TFL, TFR, TBL, TBR}, "5.0.4"},
    {AAX_eStemFormat_7_0_2, OP_BUS_SURROUND, 9, {FL, FC, FR, SL, SR, BL, BR, TSL, TSR}, "7.0.2"},
    {AAX_eStemFormat_7_1_2, OP_BUS_SURROUND, 10, {FL, FC, FR, SL, SR, BL, BR, LFE, TSL, TSR}, "7.1.2"},
    {AAX_eStemFormat_7_0_4, OP_BUS_SURROUND, 11, {FL, FC, FR, SL, SR, BL, BR, TFL, TFR, TBL, TBR}, "7.0.4"},
    {AAX_eStemFormat_7_0_6, OP_BUS_SURROUND, 13, {FL, FC, FR, SL, SR, BL, BR, TFL, TFR, TSL, TSR, TBL, TBR}, "7.0.6"},
    {AAX_eStemFormat_7_1_6, OP_BUS_SURROUND, 14, {FL, FC, FR, SL, SR, BL, BR, LFE, TFL, TFR, TSL, TSR, TBL, TBR}, "7.1.6"},
    {AAX_eStemFormat_9_0_4, OP_BUS_SURROUND, 13, {FL, FC, FR, FLW, FRW, SL, SR, BL, BR, TFL, TFR, TBL, TBR}, "9.0.4"},
    {AAX_eStemFormat_9_1_4, OP_BUS_SURROUND, 14, {FL, FC, FR, FLW, FRW, SL, SR, BL, BR, LFE, TFL, TFR, TBL, TBR}, "9.1.4"},
    {AAX_eStemFormat_9_0_6, OP_BUS_SURROUND, 15, {FL, FC, FR, FLW, FRW, SL, SR, BL, BR, TFL, TFR, TSL, TSR, TBL, TBR}, "9.0.6"},
    {AAX_eStemFormat_9_1_6, OP_BUS_SURROUND, 16, {FL, FC, FR, FLW, FRW, SL, SR, BL, BR, LFE, TFL, TFR, TSL, TSR, TBL, TBR}, "9.1.6"},
    {AAX_eStemFormat_Ambi_1_ACN, OP_BUS_AMBISONIC, 4, {NO}, "HOA1"},
    {AAX_eStemFormat_Ambi_2_ACN, OP_BUS_AMBISONIC, 9, {NO}, "HOA2"},
    {AAX_eStemFormat_Ambi_3_ACN, OP_BUS_AMBISONIC, 16, {NO}, "HOA3"},
};
uint64_t roleMask(const uint8_t* roles, uint32_t count) {
    uint64_t mask = 0;
    for (uint32_t i = 0; i < count; ++i) {
        if (roles[i] >= 63) return 0;
        mask |= 1ull << roles[i];
    }
    return mask;
}
bool hasRoles(uint32_t kind) { return kind == OP_BUS_STEREO || kind == OP_BUS_SURROUND; }
uint32_t mix32(uint32_t h, uint32_t v) {
    for (int i = 0; i < 4; ++i) {
        h ^= (v >> (8 * i)) & 0xFF;
        h *= 16777619u;
    }
    return h;
}
}
const Stem* findStem(AAX_EStemFormat format) {
    for (const Stem& stem : kStems)
        if (stem.format == format) return &stem;
    return nullptr;
}
const Stem* stemForBus(const OpBus& bus) {
    if (bus.kind == OP_BUS_ABSENT || bus.channels == 0 || bus.channels > kMaxChannels) return nullptr;
    const uint64_t want = hasRoles(bus.kind) ? roleMask(bus.roles, bus.channels) : 0;
    if (hasRoles(bus.kind) && want == 0) return nullptr;
    for (const Stem& stem : kStems) {
        if (stem.kind != bus.kind || stem.channels != bus.channels) continue;
        if (!hasRoles(bus.kind) || roleMask(stem.roles, stem.channels) == want) return &stem;
    }
    return nullptr;
}
bool channelMap(const OpBus& bus, const Stem& stem, uint8_t* map) {
    if (!map || bus.channels != stem.channels || bus.channels > kMaxChannels) return false;
    bool used[kMaxChannels] = {};
    for (uint32_t k = 0; k < bus.channels; ++k) {
        uint32_t found = kMaxChannels;
        if (!hasRoles(bus.kind)) {
            found = k;
        } else {
            for (uint32_t p = 0; p < stem.channels; ++p)
                if (stem.roles[p] == bus.roles[k]) found = p;
        }
        if (found >= stem.channels || used[found]) return false;
        used[found] = true;
        map[k] = static_cast<uint8_t>(found);
    }
    return true;
}
std::vector<Config> buildConfigs(const OpShellInfo& info) {
    std::vector<Config> list;
    const auto add = [&](AAX_EStemFormat input, AAX_EStemFormat output, int32_t layout, bool sidechain) {
        for (Config& have : list) {
            if (have.input != input || have.output != output) continue;
            // The same stems from two layouts: the one with a sidechain describes more.
            if (sidechain && !have.sidechain) {
                have.layout = layout;
                have.sidechain = true;
            }
            return;
        }
        Config config;
        config.input = input;
        config.output = output;
        config.layout = layout;
        config.sidechain = sidechain;
        list.push_back(config);
    };
    const uint32_t count = std::min<uint32_t>(op_shell_layout_count(), 64);
    for (uint32_t i = 0; i < count; ++i) {
        OpLayout layout{};
        if (!op_shell_layout(i, &layout)) continue;
        const bool hasIn = layout.input.kind != OP_BUS_ABSENT;
        const bool hasOut = layout.output.kind != OP_BUS_ABSENT;
        const Stem* in = hasIn ? stemForBus(layout.input) : nullptr;
        const Stem* out = hasOut ? stemForBus(layout.output) : nullptr;
        // A layout with a bus that has no AAX stem (discrete, 9.1.6 and so on) is not offered.
        if ((hasIn && !in) || (hasOut && !out)) continue;
        const bool side = layout.sidechain.kind != OP_BUS_ABSENT;
        if (hasOut) {
            // An instrument has no audio input: AAX still wants an input stem, the same as the output.
            add(in ? in->format : out->format, out->format, static_cast<int32_t>(i), side);
        } else if (hasIn) {
            // A note effect passes its audio through.
            add(in->format, in->format, static_cast<int32_t>(i), side);
        } else {
            add(AAX_eStemFormat_Mono, AAX_eStemFormat_Mono, static_cast<int32_t>(i), side);
            add(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, static_cast<int32_t>(i), side);
        }
    }
    // Before ABI 2 every plugin was offered as mono and stereo, whatever it declared: the engine takes
    // one channel and runs a narrow bus. An effect that declares stereo and not mono keeps that.
    bool stereo = false, mono = false;
    for (const Config& c : list) {
        stereo = stereo || (c.input == AAX_eStemFormat_Stereo && c.output == AAX_eStemFormat_Stereo);
        mono = mono || (c.input == AAX_eStemFormat_Mono && c.output == AAX_eStemFormat_Mono);
    }
    if (stereo && !mono && op_shell_kind() == OP_KIND_EFFECT) add(AAX_eStemFormat_Mono, AAX_eStemFormat_Mono, -1, false);
    // Plug-in IDs: the ABI's own for mono and stereo, a hash of the stereo ID and the stems for the rest
    // (bit 0x20 set in every byte, as for the others, so no byte is zero).
    for (Config& c : list) {
        if (c.input == AAX_eStemFormat_Mono && c.output == AAX_eStemFormat_Mono) {
            c.pluginId = info.plugin_id_mono;
        } else if (c.input == AAX_eStemFormat_Stereo && c.output == AAX_eStemFormat_Stereo) {
            c.pluginId = info.plugin_id_stereo;
        } else {
            uint32_t h = mix32(2166136261u, info.plugin_id_stereo);
            h = mix32(h, static_cast<uint32_t>(c.input));
            h = mix32(h, static_cast<uint32_t>(c.output));
            c.pluginId = h | 0x20202020u;
        }
    }
    // A collision between two components (or with mono and stereo) would make the host treat them as one.
    for (size_t i = 0; i < list.size(); ++i) {
        for (size_t j = 0; j < i; ++j) {
            while (list[i].pluginId == list[j].pluginId) list[i].pluginId = (list[i].pluginId + 1) | 0x20202020u;
        }
    }
    return list;
}
bool midiToEvent(const AAX_CMidiPacket& packet, uint32_t frames, OpEvent* out) {
    if (!out || frames == 0 || packet.mLength == 0 || packet.mLength > 3) return false;
    const uint8_t status = packet.mData[0];
    if (!(status & 0x80) || status == 0xF0 || status == 0xF7) return false;
    uint8_t data[2] = {0, 0};
    for (uint32_t i = 1; i < packet.mLength; ++i) data[i - 1] = packet.mData[i];
    if ((data[0] | data[1]) & 0x80) return false;
    *out = OpEvent{};
    // The timestamp is relative to the start of the buffer. A late one lands on the last sample
    // rather than being dropped, so a note off is never lost.
    out->offset = std::min(packet.mTimestamp, frames - 1);
    const uint8_t type = status & 0xF0;
    if (type == 0x90 && data[1] > 0) {
        out->kind = OP_EVENT_NOTE_ON;
        out->channel = status & 0x0F;
        out->note = data[0];
        out->velocity = data[1];
    } else if (type == 0x90 || type == 0x80) {
        out->kind = OP_EVENT_NOTE_OFF;
        out->channel = status & 0x0F;
        out->note = data[0];
        out->velocity = type == 0x80 ? data[1] : 0;
    } else {
        out->kind = OP_EVENT_MIDI;
        out->midi[0] = status;
        out->midi[1] = data[0];
        out->midi[2] = data[1];
    }
    return true;
}
AAX_CMidiPacket midiFromEvent(const OpMidiEvent& event, uint32_t offset) {
    AAX_CMidiPacket packet{};
    const uint8_t status = event.data[0];
    const uint8_t type = status & 0xF0;
    packet.mTimestamp = offset;
    packet.mLength = (type == 0xC0 || type == 0xD0 || status == 0xF1 || status == 0xF3) ? 2u : status >= 0xF8 ? 1u : 3u;
    for (uint32_t i = 0; i < 4; ++i) packet.mData[i] = i < packet.mLength ? event.data[i] : 0;
    packet.mIsImmediate = offset == 0 ? 1 : 0;
    return packet;
}
namespace {
void fillBars(OpTransport& t) {
    if (!(t.flags & OP_TRANSPORT_HAS_BEATS)) return;
    const double num = (t.flags & OP_TRANSPORT_HAS_SIGNATURE) ? t.numerator : 4.0;
    const double den = (t.flags & OP_TRANSPORT_HAS_SIGNATURE) ? t.denominator : 4.0;
    // One meter for the whole timeline: AAX gives the meter at the current position only.
    const double bar = 4.0 * num / den;
    const double index = std::floor(t.quarter_notes / bar);
    t.bar_number = static_cast<int64_t>(index);
    t.bar_start = index * bar;
}
}
bool readTransport(AAX_IMIDINode* node, double rate, OpTransport* out) {
    if (!node || !out) return false;
    AAX_ITransport* transport = node->GetTransport();
    if (!transport) return false;
    OpTransport t{};
    bool playing = false;
    if (transport->IsTransportPlaying(&playing) != AAX_SUCCESS) playing = false;
    if (playing) t.flags |= OP_TRANSPORT_PLAYING;
    double tempo = 0.0;
    if (transport->GetCurrentTempo(&tempo) == AAX_SUCCESS && std::isfinite(tempo) && tempo > 0.0) {
        t.tempo = tempo;
        t.flags |= OP_TRANSPORT_HAS_TEMPO;
    }
    int32_t numerator = 0, denominator = 0;
    if (transport->GetCurrentMeter(&numerator, &denominator) == AAX_SUCCESS && numerator > 0 && denominator > 0) {
        t.numerator = static_cast<uint32_t>(numerator);
        t.denominator = static_cast<uint32_t>(denominator);
        t.flags |= OP_TRANSPORT_HAS_SIGNATURE;
    }
    int64_t ticks = 0;
    if (transport->GetCurrentTickPosition(&ticks) == AAX_SUCCESS) {
        t.quarter_notes = static_cast<double>(ticks) / kTicksPerQuarter;
        t.flags |= OP_TRANSPORT_HAS_BEATS;
    }
    // The SDK documents the sample location as valid during playback only.
    int64_t sample = 0;
    if (playing && rate > 0.0 && transport->GetCurrentNativeSampleLocation(&sample) == AAX_SUCCESS) {
        t.sample_pos = sample;
        t.seconds = static_cast<double>(sample) / rate;
        t.flags |= OP_TRANSPORT_HAS_SECONDS;
    }
    fillBars(t);
    *out = t;
    return true;
}
ProcessState::~ProcessState() {
    if (engine) op_shell_destroy(engine);
}
bool ProcessState::prepare(double rate, uint32_t channelCount) {
    if (channelCount != 1 && channelCount != 2) return false;
    const AAX_EStemFormat stem = channelCount == 1 ? AAX_eStemFormat_Mono : AAX_eStemFormat_Stereo;
    return prepareStems(rate, stem, stem);
}
bool ProcessState::prepareStems(double rate, AAX_EStemFormat inputStem, AAX_EStemFormat outputStem) {
    if (!std::isfinite(rate) || rate < 8000.0 || rate > 768000.0) return false;
    OpShellInfo info{};
    if (!abiOk() || !op_shell_info(&info) || info.param_count == 0 || info.param_count > kMaxParams) return false;
    Config config;
    bool found = false;
    for (const Config& candidate : buildConfigs(info)) {
        if (candidate.input == inputStem && candidate.output == outputStem) {
            config = candidate;
            found = true;
            break;
        }
    }
    const Stem* inStem = findStem(inputStem);
    const Stem* outStem = findStem(outputStem);
    if (!found || !inStem || !outStem) return false;
    Bus nextIn, nextOut;
    nextIn.aax = inStem->channels;
    nextOut.aax = outStem->channels;
    bool nextSide = false;
    bool nextNoteOnly = false;
    uint32_t createChannels = 2;
    OpLayout layout{};
    if (config.layout < 0) {
        // The ABI 1 engine: one or two channels, in and out, in the host's order (there is no other).
        nextIn.engine = nextIn.aax;
        nextOut.engine = nextOut.aax;
        for (uint32_t k = 0; k < kMaxChannels; ++k) nextIn.map[k] = nextOut.map[k] = static_cast<uint8_t>(k);
        createChannels = nextOut.aax;
    } else {
        if (!op_shell_layout(static_cast<uint32_t>(config.layout), &layout)) return false;
        if (layout.input.kind != OP_BUS_ABSENT) {
            nextIn.engine = layout.input.channels;
            if (!channelMap(layout.input, *inStem, nextIn.map)) return false;
        }
        if (layout.output.kind != OP_BUS_ABSENT) {
            nextOut.engine = layout.output.channels;
            if (!channelMap(layout.output, *outStem, nextOut.map)) return false;
        } else {
            nextNoteOnly = true;
        }
        nextSide = layout.sidechain.kind != OP_BUS_ABSENT;
    }
    void* next = op_shell_create(rate, kMaxBlock, createChannels);
    if (!next) return false;
    if (config.layout >= 0 && !op_shell_set_layout(next, static_cast<uint32_t>(config.layout))) {
        op_shell_destroy(next);
        return false;
    }
    if (engine) op_shell_destroy(engine);
    engine = next;
    in = nextIn;
    out = nextOut;
    sidechain = nextSide;
    noteOnly = nextNoteOnly;
    notePorts = op_shell_note_ports();
    channels = out.aax;
    sampleRate = static_cast<uint32_t>(rate);
    paramCount = info.param_count;
    latency = op_shell_latency(engine);
    delayCap = std::min(op_shell_max_latency(engine), kMaxBypassDelay);
    latency = std::min(latency, delayCap);
    dry.assign(out.aax, std::vector<float>());
    line.assign(out.aax, std::vector<float>());
    for (uint32_t c = 0; c < out.aax; ++c) {
        dry[c].assign(kMaxBlock, 0.f);
        if (delayCap > 0) line[c].assign(static_cast<size_t>(delayCap) + 1, 0.f);
    }
    events.assign(OP_MAX_EVENTS, OpEvent{});
    pieceEvents.assign(OP_MAX_EVENTS, OpEvent{});
    held.assign(paramCount, 0.f);
    check.assign(paramCount, 0);
    engineWritten();
    linePos = 0;
    mixStep = static_cast<float>(1.0 / std::max(1.0, kBypassFadeSeconds * rate));
    mix = 0.f;
    mixPending = true;
    prepared.store(true, std::memory_order_release);
    return true;
}
void ProcessState::reset() {
    if (engine) op_shell_reset(engine);
    for (auto& l : line) std::fill(l.begin(), l.end(), 0.f);
    linePos = 0;
    mixPending = true;
}
namespace {
// The AAX input channel c, or null when the host gave none (an instrument's input).
const float* inputChannel(const ProcessState& state, const Context* context, uint32_t c) {
    return context->input && c < state.in.aax ? context->input[c] : nullptr;
}
// Pushes the input through the latency delay into state.dry. The line always
// runs, so switching bypass on, or a new latency, never starts from stale
// audio: a new latency only moves the read point. Taken before the engine runs
// because the host may give the same buffer for input and output.
void captureDry(ProcessState& state, Context* context, unsigned offset, unsigned count) {
    // The latency now (ABI 9; not the host's read), within the line sized in prepare().
    state.latency = std::min(op_shell_latency_now(state.engine), state.delayCap);
    const size_t size = static_cast<size_t>(state.delayCap) + 1;
    for (uint32_t c = 0; c < state.out.aax; ++c) {
        const float* in = inputChannel(state, context, c);
        if (in) in += offset;
        float* dry = state.dry[c].data();
        if (state.delayCap == 0) {
            if (in) std::memcpy(dry, in, count * sizeof(float));
            else std::fill_n(dry, count, 0.f);
            continue;
        }
        float* line = state.line[c].data();
        size_t pos = state.linePos;
        size_t read = pos >= state.latency ? pos - state.latency : pos + size - state.latency;
        for (unsigned i = 0; i < count; ++i) {
            line[pos] = in ? in[i] : 0.f;
            dry[i] = line[read];
            if (++pos == size) pos = 0;
            if (++read == size) read = 0;
        }
    }
    if (state.delayCap != 0) state.linePos = (state.linePos + count) % size;
}
// Moves the engine output towards the dry signal (or back). Fully bypassed
// output is the dry signal exactly.
void applyBypass(ProcessState& state, Context* context, unsigned offset, unsigned count, float target) {
    if (state.mix == target) {
        if (target == 0.f) return;
        for (uint32_t c = 0; c < state.out.aax; ++c) std::memcpy(context->output[c] + offset, state.dry[c].data(), count * sizeof(float));
        return;
    }
    float mix = state.mix;
    for (unsigned i = 0; i < count; ++i) {
        if (mix < target) mix = std::min(target, mix + state.mixStep);
        else mix = std::max(target, mix - state.mixStep);
        for (uint32_t c = 0; c < state.out.aax; ++c) {
            float& out = context->output[c][offset + i];
            const float dry = state.dry[c][i];
            out = mix >= 1.f ? dry : out + (dry - out) * mix;
        }
    }
    state.mix = mix;
}
// MIDI the plugin produced in the piece that started at `offset`, posted to the output node.
void postMidi(ProcessState& state, AAX_IMIDINode* node, unsigned offset) {
    OpMidiEvent events[64];
    for (;;) {
        const uint32_t n = op_shell_midi_out(state.engine, events, 64);
        if (node)
            for (uint32_t i = 0; i < n; ++i) {
                AAX_CMidiPacket packet = midiFromEvent(events[i], offset + events[i].offset);
                node->PostMIDIPacket(&packet);
            }
        if (n < 64) break;
    }
}
}
void AAX_CALLBACK process(Context* const begin[], const void* end) {
    for (auto current = begin; current < end; ++current) {
        Context* context = *current;
        if (!context || !context->frames || *context->frames <= 0 || !context->state || !*context->state || !context->output) continue;
        ProcessState& state = **context->state;
        const auto frames = static_cast<unsigned>(*context->frames);
        const auto silence = [&] {
            for (uint32_t channel = 0; channel < state.out.aax; ++channel) {
                if (context->output[channel]) std::fill_n(context->output[channel], frames, 0.f);
            }
        };
        if (!state.engine || !context->controls || state.out.aax == 0 || state.events.empty()) {
            silence();
            continue;
        }
        bool wired = true;
        for (uint32_t c = 0; c < state.out.aax; ++c) wired = wired && context->output[c];
        // An engine that reads audio needs every input channel it maps.
        if (state.in.engine > 0) {
            wired = wired && context->input;
            for (uint32_t c = 0; wired && c < state.in.aax; ++c) wired = context->input[c] != nullptr;
        }
        if (!wired) {
            silence();
            continue;
        }
        const Packet& packet = *context->controls;
        const float* plain = packet.plain();
        const bool bypass = packet.bypass != 0;
        // The port holds paramCount values (its size in Describe), so a larger count is refused.
        bool valid = packet.count <= state.paramCount;
        for (uint32_t i = 0; valid && i < packet.count; ++i) valid = std::isfinite(plain[i]);
        // A refused packet silences the block, unless the signal is bypassed:
        // that does not depend on the values.
        if (!valid && !bypass) {
            silence();
            continue;
        }
        // Events for the whole AAX buffer. MIDI first: it has the room it needs.
        uint32_t events = 0;
        // Fields Describe did not register hold undefined values in a real host, so each
        // optional field is read only when this plugin registered it.
        AAX_IMIDINode* const midiIn = (state.notePorts & OP_NOTE_PORT_INPUT) ? context->midiIn : nullptr;
        AAX_IMIDINode* const midiOut = (state.notePorts & OP_NOTE_PORT_OUTPUT) ? context->midiOut : nullptr;
        const int32_t* const sideChainIndex = state.sidechain ? context->sideChain : nullptr;
        if (midiIn) {
            if (const AAX_CMidiStream* stream = midiIn->GetNodeBuffer(); stream && stream->mBuffer) {
                for (uint32_t i = 0; i < stream->mBufferSize && events < OP_MAX_EVENTS; ++i) {
                    if (midiToEvent(stream->mBuffer[i], frames, &state.events[events])) ++events;
                }
            }
        }
        if (valid) {
            const bool resync = state.resync.exchange(false, std::memory_order_acquire);
            for (uint32_t i = 0; i < packet.count; ++i) {
                // The engine's own value is the reference, so a block lost to a state call heals on
                // the next one. It is asked only where it may differ from what was last seen there.
                if (resync || state.check[i]) {
                    state.held[i] = op_shell_get_plain(state.engine, i);
                    state.check[i] = 0;
                }
                if (state.held[i] == plain[i]) continue;
                state.held[i] = plain[i];
                if (events < OP_MAX_EVENTS) {
                    OpEvent& event = state.events[events++];
                    event = OpEvent{};
                    event.offset = 0;
                    event.kind = OP_EVENT_PARAM_PLAIN;
                    event.index = i;
                    event.value = plain[i];
                    state.check[i] = 1;
                } else {
                    // More changes than the event list holds (the first block of a large plugin): set directly.
                    op_shell_set_plain(state.engine, i, plain[i]);
                }
            }
        }
        OpTransport transport{};
        const bool haveTransport = context->transport && readTransport(context->transport, state.sampleRate, &transport);
        // The host's sidechain: the extra channel after the main inputs; an index of 0 means none.
        const float* sidechain[1] = {nullptr};
        if (sideChainIndex && context->input && *sideChainIndex > 0 && *sideChainIndex < 64) {
            sidechain[0] = context->input[*sideChainIndex];
        }
        const float target = bypass ? 1.f : 0.f;
        if (state.mixPending) {
            state.mix = target;
            state.mixPending = false;
        }
        bool failed = false;
        for (unsigned offset = 0; offset < frames;) {
            const auto count = std::min(kMaxBlock, frames - offset);
            captureDry(state, context, offset, count);
            const float* inPointers[kMaxChannels];
            float* outPointers[kMaxChannels];
            for (uint32_t k = 0; k < state.in.engine; ++k) { const float* ch = context->input[state.in.map[k]]; inPointers[k] = ch ? ch + offset : nullptr; }
            for (uint32_t k = 0; k < state.out.engine; ++k) { float* ch = context->output[state.out.map[k]]; outPointers[k] = ch ? ch + offset : nullptr; }
            // The piece's events, with offsets from the piece's start.
            uint32_t pieceCount = 0;
            for (uint32_t i = 0; i < events; ++i) {
                const OpEvent& event = state.events[i];
                if (event.offset < offset || event.offset >= offset + count) continue;
                state.pieceEvents[pieceCount] = event;
                state.pieceEvents[pieceCount++].offset = event.offset - offset;
            }
            OpTransport pieceTransport = transport;
            if (offset > 0 && (transport.flags & OP_TRANSPORT_PLAYING)) {
                if (transport.flags & OP_TRANSPORT_HAS_SECONDS) {
                    pieceTransport.sample_pos += offset;
                    pieceTransport.seconds += static_cast<double>(offset) / state.sampleRate;
                }
                if ((transport.flags & OP_TRANSPORT_HAS_BEATS) && (transport.flags & OP_TRANSPORT_HAS_TEMPO))
                    pieceTransport.quarter_notes += offset * transport.tempo / (60.0 * state.sampleRate);
                fillBars(pieceTransport);
            }
            OpProcess args{};
            args.struct_size = sizeof(OpProcess);
            args.frames = count;
            args.inputs = state.in.engine ? inPointers : nullptr;
            args.input_channels = state.in.engine;
            args.output_channels = state.out.engine;
            args.outputs = state.out.engine ? outPointers : nullptr;
            // AAX carries a single mono side-chain channel; the engine's other sidechain channels read as silence.
            args.sidechain = state.sidechain && sidechain[0] ? sidechain : nullptr;
            args.sidechain_channels = args.sidechain ? 1 : 0;
            args.event_count = pieceCount;
            args.events = state.pieceEvents.data();
            args.transport = haveTransport ? &pieceTransport : nullptr;
            if (!op_shell_process_events(state.engine, &args)) {
                failed = true;
                // Output that is fully bypassed does not need the engine.
                if (!bypass || state.mix != 1.f) break;
            } else {
                if (state.noteOnly) {
                    for (uint32_t c = 0; c < state.out.aax; ++c) {
                        const float* in = inputChannel(state, context, c);
                        if (!in) std::fill_n(context->output[c] + offset, count, 0.f);
                        else if (in != context->output[c]) std::memmove(context->output[c] + offset, in + offset, count * sizeof(float));
                    }
                }
                if (midiOut) postMidi(state, midiOut, offset);
            }
            applyBypass(state, context, offset, count, target);
            offset += count;
        }
        if (failed && !(bypass && state.mix == 1.f)) silence();
    }
}
}
