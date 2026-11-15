// SPDX-License-Identifier: MIT
// Proves the AAX process callback carries shell ABI 2: MIDI in and out at sample
// offsets, parameter changes as events, the transport, the sidechain and
// Pro Tools' channel order. It calls the real callback (openplugin_aax::process)
// with a fake MIDI node and a fake AAX_ITransport over the stand-in ABI in
// proof_stub.cpp, which records what it received. Not a Pro Tools test.
//
//   ctest --test-dir build-shell -R aax_events_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "host_fakes.h"
#include "proof_stub.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

using namespace openplugin_aax;

static unsigned long g_checks = 0;
static void require(bool ok, const char* message) {
    ++g_checks;
    if (!ok) {
        std::fprintf(stderr, "AAX events proof failed: %s\n", message);
        std::exit(1);
    }
}
static bool near(float a, float b) { return std::fabs(a - b) < 1e-5f; }

static OpBus bus(uint32_t kind, std::vector<uint8_t> roles) {
    OpBus b{};
    b.kind = kind;
    b.channels = static_cast<uint32_t>(roles.size());
    std::memset(b.roles, OP_ROLE_NONE, sizeof b.roles);
    for (size_t i = 0; i < roles.size(); ++i) b.roles[i] = roles[i];
    return b;
}
static OpBus monoBus() { return bus(OP_BUS_MONO, {OP_ROLE_NONE}); }
static OpLayout layout(OpBus in, OpBus out, OpBus side = OpBus{}) {
    OpLayout l{};
    l.input = in;
    l.output = out;
    l.sidechain = side;
    std::strcpy(l.name, "proof");
    return l;
}
static void configure(uint32_t kind, uint32_t ports, std::vector<OpLayout> layouts) {
    op_stub_configure(kind, ports, static_cast<uint32_t>(layouts.size()), layouts.data());
}

// One AAX buffer set and a context over it.
struct Rig {
    ProcessState state;
    PacketBuffer packet;
    std::vector<std::vector<float>> in, out;
    std::vector<float*> inPtr, outPtr;
    int32_t frames = 0;
    int32_t sideChain = 0;
    FakeMidiNode midiIn, midiOut, transportNode;
    FakeTransport transport;
    ProcessState* statePtr = nullptr;
    Context context{};
    bool useMidiIn = false, useMidiOut = false, useTransport = false, useSideChain = false;

    // inChannels includes the sidechain channel after the main ones when there is one.
    Rig(AAX_EStemFormat inStem, AAX_EStemFormat outStem, uint32_t inChannels, uint32_t outChannels, int32_t blockFrames) : frames(blockFrames) {
        require(state.prepareStems(48000.0, inStem, outStem), "prepare");
        packet = PacketBuffer(op_stub_param_count());
        packet.plain()[0] = 0.5f;
        in.assign(inChannels, std::vector<float>(static_cast<size_t>(blockFrames), 0.f));
        out.assign(outChannels, std::vector<float>(static_cast<size_t>(blockFrames), 0.f));
        for (auto& c : in) inPtr.push_back(c.data());
        for (auto& c : out) outPtr.push_back(c.data());
        midiOut.reserve(1024);
        transportNode.transport = &transport;
        statePtr = &state;
    }
    void process() {
        context = Context{inPtr.data(), outPtr.data(), &frames, packet.get(), &statePtr,
                          useMidiIn ? &midiIn : nullptr, useMidiOut ? &midiOut : nullptr,
                          useTransport ? &transportNode : nullptr, useSideChain ? &sideChain : nullptr};
        Context* contexts[] = {&context};
        midiOut.posted.clear();
        openplugin_aax::process(contexts, contexts + 1);
    }
    StubSeen seen() {
        StubSeen s;
        op_stub_seen(state.engine, &s);
        return s;
    }
};

static void instrumentAndMidi() {
    // An instrument: no audio input, stereo out, notes in and out.
    configure(OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT,
              {layout(OpBus{}, bus(OP_BUS_STEREO, {0, 1}))});
    Rig rig(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 2, 2, 512);
    require(rig.state.in.engine == 0 && rig.state.out.engine == 2, "an instrument has no engine input and two outputs");
    rig.useMidiIn = rig.useMidiOut = true;
    rig.midiIn.setInput({
        midiPacket(100, 3, 0x90, 60, 127),   // note on
        midiPacket(150, 3, 0xB0, 7, 64),     // control change
        midiPacket(200, 3, 0x80, 60, 40),    // note off
        midiPacket(300, 3, 0x91, 64, 64),    // note on, channel 1
        midiPacket(350, 2, 0xC0, 5),         // program change (two bytes)
        midiPacket(400, 3, 0x91, 64, 0),     // velocity-0 note on is a note off
        midiPacket(450, 3, 0xF0, 1, 2),      // SysEx start: not carried
        midiPacket(460, 3, 0x90, 200, 1),    // data byte over 127: not carried
        midiPacket(470, 4, 0x90, 1, 1),      // four bytes: not carried
        midiPacket(480, 0, 0x90),            // empty: not carried
        midiPacket(9999, 3, 0xE0, 0, 64),    // late: lands on the last sample
    });
    rig.process();
    const StubSeen seen = rig.seen();
    require(seen.refused == 0 && seen.calls == 1, "one accepted call");
    // Sample-accurate delivery: the engine saw each event at its offset.
    struct Want { uint64_t at; uint32_t kind; uint8_t channel, note, velocity; };
    const Want want[] = {{100, OP_EVENT_NOTE_ON, 0, 60, 127}, {150, OP_EVENT_MIDI, 0, 0, 0}, {200, OP_EVENT_NOTE_OFF, 0, 60, 40},
                         {300, OP_EVENT_NOTE_ON, 1, 64, 64}, {350, OP_EVENT_MIDI, 0, 0, 0}, {400, OP_EVENT_NOTE_OFF, 1, 64, 0},
                         {511, OP_EVENT_MIDI, 0, 0, 0}};
    uint32_t notes = 0;
    for (uint32_t i = 0; i < seen.logCount; ++i)
        if (seen.log[i].kind != OP_EVENT_PARAM_PLAIN) ++notes;
    require(notes == 7, "seven events carried, four packets refused");
    uint32_t k = 0;
    for (uint32_t i = 0; i < seen.logCount; ++i) {
        const StubLogEvent& e = seen.log[i];
        if (e.kind == OP_EVENT_PARAM_PLAIN) continue;
        require(e.position == want[k].at && e.kind == want[k].kind, "event at its offset with its kind");
        if (e.kind != OP_EVENT_MIDI) require(e.channel == want[k].channel && e.note == want[k].note && e.velocity == want[k].velocity, "note fields");
        ++k;
    }
    require(seen.log[1].midi[0] == 0xB0 && seen.log[1].midi[1] == 7 && seen.log[1].midi[2] == 64, "control change bytes");
    // The audio follows the notes: level from the note on's sample.
    const float level = 1.f, level2 = 64.f / 127.f;
    for (uint32_t i = 0; i < 512; ++i) {
        float want = 0.f;
        if (i >= 100 && i < 200) want = level;
        if (i >= 300 && i < 400) want = level2;
        require(near(rig.out[0][i], want) && near(rig.out[1][i], want), "instrument audio starts and stops at the note offsets");
    }
    // MIDI the plugin produced goes to the output node at the same offsets.
    const auto& p = rig.midiOut.posted;
    require(p.size() == 7, "seven packets posted to the MIDI output node");
    const uint32_t at[] = {100, 150, 200, 300, 350, 400, 511};
    const uint8_t status[] = {0x90, 0xB0, 0x80, 0x91, 0xC0, 0x81, 0xE0};
    const uint8_t length[] = {3, 3, 3, 3, 2, 3, 3};
    for (size_t i = 0; i < p.size(); ++i) {
        require(p[i].mTimestamp == at[i], "output timestamp is the sample offset");
        require(p[i].mData[0] == status[i] && p[i].mLength == length[i], "output status and length");
        require((p[i].mTimestamp == 0) == (p[i].mIsImmediate != 0), "immediate only at offset 0");
    }
    require(p[0].mData[1] == 72 && p[0].mData[2] == 127, "the note went out an octave up");
    require(p[4].mData[1] == 5 && p[4].mData[2] == 0, "two-byte message keeps its data");
    // A host with no MIDI input node still runs, and one without an output node loses the output quietly.
    rig.useMidiIn = false;
    rig.useMidiOut = false;
    rig.process();
    require(rig.seen().refused == 0 && rig.midiOut.posted.empty(), "no nodes, no MIDI, audio still runs");
    // A node with no buffer, and a node that lists more packets than the event list holds.
    rig.useMidiIn = true;
    rig.midiIn.nullBuffer = true;
    rig.process();
    require(rig.seen().refused == 0, "a MIDI node without a buffer is ignored");
    rig.midiIn.nullBuffer = false;
    std::vector<AAX_CMidiPacket> many;
    for (uint32_t i = 0; i < 3000; ++i) many.push_back(midiPacket(i % 512, 3, 0xB0, 1, i % 128));
    rig.midiIn.setInput(many);
    rig.useMidiOut = true;
    rig.process();
    require(rig.seen().refused == 0 && rig.midiOut.posted.size() == 256, "3000 packets are capped by the event list; the output pages 64 at a time (256 posted)");
    std::puts("AAX events proof: instrument MIDI in at sample offsets (note on/off, velocity-0 off, CC, program change, late stamp clamped; SysEx, bad bytes, four-byte and empty packets refused), audio from the note's sample, MIDI out at the same offsets with right lengths, missing nodes and oversized lists safe");
}

static void splitBlock() {
    configure(OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, {layout(OpBus{}, bus(OP_BUS_STEREO, {0, 1}))});
    // 5000 frames: pieces of 2048, 2048 and 904 inside one callback.
    Rig rig(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 2, 2, 5000);
    rig.useMidiIn = rig.useMidiOut = true;
    rig.midiIn.setInput({
        midiPacket(0, 3, 0x90, 60, 127),
        midiPacket(2047, 3, 0x80, 60, 0),
        midiPacket(2048, 3, 0x90, 61, 127),
        midiPacket(3000, 3, 0x80, 61, 0),
        midiPacket(4096, 3, 0x90, 62, 127),
        midiPacket(4999, 3, 0x80, 62, 0),
    });
    rig.process();
    const StubSeen seen = rig.seen();
    require(seen.calls == 3 && seen.refused == 0, "three pieces");
    const uint64_t at[] = {0, 2047, 2048, 3000, 4096, 4999};
    uint32_t n = 0;
    for (uint32_t i = 0; i < seen.logCount; ++i) {
        if (seen.log[i].kind == OP_EVENT_PARAM_PLAIN) continue;
        require(n < 6 && seen.log[i].position == at[n], "events keep their place in the AAX buffer across pieces");
        ++n;
    }
    require(n == 6, "all six events reached the engine");
    for (uint32_t i = 0; i < 5000; ++i) {
        const bool on = i < 2047 || (i >= 2048 && i < 3000) || (i >= 4096 && i < 4999);
        require(near(rig.out[0][i], on ? 1.f : 0.f), "audio edges at the AAX offsets across piece boundaries");
    }
    require(rig.midiOut.posted.size() == 6, "six packets out");
    for (size_t i = 0; i < 6; ++i) require(rig.midiOut.posted[i].mTimestamp == at[i], "output timestamps are relative to the AAX buffer, not the piece");
    std::puts("AAX events proof: a 5000-frame callback split in three pieces keeps every note at its AAX offset in and out");
}

static void parameterEvents() {
    configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}))});
    Rig rig(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 2, 2, 256);
    for (auto& c : rig.in) std::fill(c.begin(), c.end(), 0.5f);
    rig.packet.plain()[0] = 0.8f;
    rig.process();
    StubSeen seen = rig.seen();
    uint32_t params = 0;
    for (uint32_t i = 0; i < seen.logCount; ++i)
        if (seen.log[i].kind == OP_EVENT_PARAM_PLAIN) {
            ++params;
            require(seen.log[i].position == 0 && seen.log[i].index == 0 && seen.log[i].value == 0.8f, "the packet value is an event at the start of the block");
        }
    require(params == 1, "one parameter event");
    require(near(rig.out[0][0], 0.4f) && near(rig.out[0][255], 0.4f), "the value applies from the first sample");
    // The same value again is not sent again.
    const uint32_t before = seen.logCount;
    rig.process();
    require(rig.seen().logCount == before, "an unchanged value makes no event");
    rig.packet.plain()[0] = 0.3f;
    rig.process();
    require(rig.seen().logCount == before + 1 && near(rig.out[0][0], 0.15f), "a changed value is one event");
    // The engine's own value is the reference: if it drifted (a block lost to a state call) the next block repairs it.
    op_shell_set_plain(rig.state.engine, 0, 0.9f);
    rig.process();
    require(near(rig.out[0][0], 0.15f), "a value the engine lost is sent again");
    // Non-finite and oversized packets still silence the block.
    rig.packet.plain()[0] = std::nanf("");
    rig.process();
    require(rig.out[0][0] == 0.f && rig.out[1][255] == 0.f, "a non-finite packet is silence");
    std::puts("AAX events proof: controls packet values reach the engine as parameter events at the start of the block, only when they differ, and heal a lost value");
}

static void transportCases() {
    configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}))});
    Rig rig(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 2, 2, 512);
    rig.useTransport = true;
    rig.transport.ticks = static_cast<int64_t>(6.5 * kTicksPerQuarter);
    rig.transport.sample = 123456;
    rig.process();
    StubSeen seen = rig.seen();
    const uint32_t all = OP_TRANSPORT_PLAYING | OP_TRANSPORT_HAS_TEMPO | OP_TRANSPORT_HAS_SIGNATURE | OP_TRANSPORT_HAS_BEATS | OP_TRANSPORT_HAS_SECONDS;
    require(seen.transportCalls == 1 && seen.transport.flags == all, "playing transport with everything the SDK gives");
    require(seen.transport.tempo == 120.0 && seen.transport.numerator == 3 && seen.transport.denominator == 4, "tempo and meter");
    require(seen.transport.sample_pos == 123456 && near(static_cast<float>(seen.transport.seconds), 123456.f / 48000.f), "position in samples, and seconds");
    require(seen.transport.quarter_notes == 6.5, "6.5 quarter notes from 960000 ticks per quarter");
    require(seen.transport.bar_number == 2 && seen.transport.bar_start == 6.0, "bar 2 of 3/4 starts at quarter 6");
    require(rig.transport.laterCalls == 0, "only the version 1 transport methods the shell uses were called");
    // Stopped: no sample position (the SDK documents it for playback only).
    rig.transport.playing = false;
    rig.process();
    seen = rig.seen();
    require((seen.transport.flags & OP_TRANSPORT_PLAYING) == 0 && (seen.transport.flags & OP_TRANSPORT_HAS_SECONDS) == 0, "a stopped transport has no sample position");
    require((seen.transport.flags & OP_TRANSPORT_HAS_BEATS) != 0, "but still the tick position");
    // A split callback: the second piece's position advances by the first piece's length.
    rig.transport.playing = true;
    rig.frames = 3000;
    for (auto& c : rig.in) c.assign(3000, 0.f);
    for (auto& c : rig.out) c.assign(3000, 0.f);
    rig.inPtr = {rig.in[0].data(), rig.in[1].data()};
    rig.outPtr = {rig.out[0].data(), rig.out[1].data()};
    rig.process();
    seen = rig.seen();
    require(seen.calls >= 2 && seen.frames == 3000 - 2048, "second piece");
    require(seen.transport.sample_pos == 123456 + 2048, "second piece position = start + piece offset");
    require(std::fabs(seen.transport.quarter_notes - (6.5 + 2048 * 120.0 / (60.0 * 48000.0))) < 1e-9, "second piece quarter note advances at the tempo");
    // A host that answers nothing: the block still runs, with no known fields.
    rig.frames = 512;
    for (auto& c : rig.in) c.assign(512, 0.f);
    for (auto& c : rig.out) c.assign(512, 0.f);
    rig.inPtr = {rig.in[0].data(), rig.in[1].data()};
    rig.outPtr = {rig.out[0].data(), rig.out[1].data()};
    rig.transport.available = false;
    rig.process();
    seen = rig.seen();
    require(seen.transport.flags == 0, "a host that answers nothing gives an empty transport");
    // No transport node: the ABI keeps the previous transport (NULL).
    rig.useTransport = false;
    const uint32_t missing = rig.seen().transportMissing;
    rig.process();
    require(rig.seen().transportMissing == missing + 1, "no transport node, no transport");
    // A node whose GetTransport() is null.
    rig.useTransport = true;
    rig.transportNode.transport = nullptr;
    const uint32_t missing2 = rig.seen().transportMissing;
    rig.process();
    require(rig.seen().transportMissing == missing2 + 1, "a node without a transport passes none");
    std::puts("AAX events proof: transport read through the transport node with version 1 calls only (tempo, meter, playing, ticks, samples), converted to the ABI struct, advanced across split pieces, empty or absent safely");
}

static void sidechain() {
    configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}), monoBus())});
    // Pro Tools gives the sidechain as the third input channel of a stereo plugin.
    Rig rig(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 3, 2, 128);
    require(rig.state.sidechain, "the layout has a sidechain");
    std::fill(rig.in[0].begin(), rig.in[0].end(), 0.5f);
    std::fill(rig.in[1].begin(), rig.in[1].end(), 0.5f);
    std::fill(rig.in[2].begin(), rig.in[2].end(), 0.25f);
    rig.useSideChain = true;
    rig.sideChain = 2;
    rig.process();
    StubSeen seen = rig.seen();
    require(seen.sidechainChannels == 1 && near(seen.sidechain0, 0.25f), "the third channel reaches the engine as the sidechain");
    require(near(rig.out[0][0], 0.5f * 0.5f + 0.25f) && near(rig.out[1][0], 0.25f), "the engine used it (and stereo is left alone)");
    // Not connected: the field is 0.
    rig.sideChain = 0;
    rig.process();
    seen = rig.seen();
    require(seen.sidechainChannels == 0 && near(rig.out[0][0], 0.25f), "no sidechain connected: the engine gets none");
    // A host that never fills the field behaves the same.
    rig.useSideChain = false;
    rig.process();
    require(rig.seen().sidechainChannels == 0, "no sidechain field");
    // A plugin without a sidechain bus never gets one, even if a host offers a channel.
    configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}))});
    Rig plain(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 3, 2, 128);
    plain.useSideChain = true;
    plain.sideChain = 2;
    plain.process();
    require(plain.seen().refused == 0 && plain.seen().sidechainChannels == 0, "a layout without a sidechain ignores the host's channel");
    std::puts("AAX events proof: the side-chain channel (index from the side-chain field) reaches the engine as the sidechain when connected, never when not or when the layout has none");
}

// One surround case. aax[p] is the canonical channel index (in the SDK order of the layout's roles)
// that sits at position p in Pro Tools' order; the expectations are written out here, not derived
// from the shell's table.
static void stem(const char* name, AAX_EStemFormat format, std::vector<uint8_t> canonicalRoles, std::vector<uint32_t> canonicalIndexAtAaxPosition) {
    const OpBus b = bus(OP_BUS_SURROUND, canonicalRoles);
    configure(OP_KIND_EFFECT, 0, {layout(b, b)});
    const uint32_t n = static_cast<uint32_t>(canonicalRoles.size());
    Rig rig(format, format, n, n, 64);
    op_stub_tag(1);
    require(rig.state.in.engine == n && rig.state.out.engine == n, name);
    for (uint32_t p = 0; p < n; ++p) std::fill(rig.in[p].begin(), rig.in[p].end(), 0.01f * static_cast<float>(p + 1));
    rig.process();
    op_stub_tag(0);
    const StubSeen seen = rig.seen();
    require(seen.refused == 0 && seen.inputChannels == n && seen.outputChannels == n, name);
    for (uint32_t p = 0; p < n; ++p) {
        const uint32_t k = canonicalIndexAtAaxPosition[p];
        // The engine's input k is the AAX channel at the position that holds role k.
        require(near(seen.input0[k], 0.01f * static_cast<float>(p + 1)), "the engine sees each role at its canonical index");
        // And its output k lands back on the same AAX position.
        require(near(rig.out[p][0], 0.5f * 0.01f * static_cast<float>(p + 1) + 100.f * static_cast<float>(k + 1)), "output returns to Pro Tools' order");
    }
    std::printf("AAX events proof: %s stem, Pro Tools order to canonical order and back\n", name);
}

static void stems() {
    // Canonical (SDK, CLAP) role order, written out: FL 0, FR 1, FC 2, LFE 3, BL 4, BR 5, SL 9, SR 10, TFL 12, TFR 14, TBL 15, TBR 17.
    // 5.1: Pro Tools L C R Ls Rs LFE; canonical FL FR FC LFE BL BR.
    stem("5.1", AAX_eStemFormat_5_1, {0, 1, 2, 3, 4, 5}, {0, 2, 1, 4, 5, 3});
    // 7.1: Pro Tools L C R Lss Rss Lsr Rsr LFE; canonical FL FR FC LFE SL SR BL BR.
    stem("7.1", AAX_eStemFormat_7_1_DTS, {0, 1, 2, 3, 9, 10, 4, 5}, {0, 2, 1, 4, 5, 6, 7, 3});
    // 7.1.4: Pro Tools L C R Lss Rss Lsr Rsr LFE Ltf Rtf Ltr Rtr; canonical FL FR FC LFE SL SR BL BR TFL TFR TBL TBR.
    stem("7.1.4", AAX_eStemFormat_7_1_4, {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17}, {0, 2, 1, 4, 5, 6, 7, 3, 8, 9, 10, 11});
    // Smaller ones.
    stem("LCR", AAX_eStemFormat_LCR, {0, 1, 2}, {0, 2, 1});
    stem("quad", AAX_eStemFormat_Quad, {0, 1, 4, 5}, {0, 1, 2, 3});
    stem("5.0", AAX_eStemFormat_5_0, {0, 1, 2, 4, 5}, {0, 2, 1, 3, 4});
    stem("6.1", AAX_eStemFormat_6_1, {0, 1, 2, 3, 4, 5, 8}, {0, 2, 1, 4, 6, 5, 3});
    stem("5.1.4", AAX_eStemFormat_5_1_4, {0, 1, 2, 3, 4, 5, 12, 14, 15, 17}, {0, 2, 1, 4, 5, 3, 6, 7, 8, 9});
    // Stereo and mono are not reordered, and the 7.1 layout is not the SDK's 5.1 stem.
    configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5}), bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5}))});
    ProcessState wrong;
    require(!wrong.prepareStems(48000.0, AAX_eStemFormat_7_1_DTS, AAX_eStemFormat_7_1_DTS), "a stem the plugin does not offer is refused");
    require(!wrong.prepareStems(48000.0, AAX_eStemFormat_5_1, AAX_eStemFormat_Stereo), "a stem pair the plugin does not offer is refused");
    // A bus with the right size and the wrong roles has no stem (two LFEs).
    OpBus twoLfe = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 3, 5});
    require(stemForBus(twoLfe) == nullptr, "duplicate roles match no stem");
    require(stemForBus(bus(OP_BUS_DISCRETE, {OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE})) == nullptr, "a discrete bus has no stem");
    std::puts("AAX events proof: stems the plugin does not offer, duplicate roles and discrete buses are refused");
}

static void differentBuses() {
    // Mono in, stereo out: AAX allows different stems; the engine sees one input and two outputs.
    configure(OP_KIND_EFFECT, 0, {layout(monoBus(), bus(OP_BUS_STEREO, {0, 1}))});
    Rig rig(AAX_eStemFormat_Mono, AAX_eStemFormat_Stereo, 1, 2, 64);
    std::fill(rig.in[0].begin(), rig.in[0].end(), 0.5f);
    rig.process();
    StubSeen seen = rig.seen();
    require(seen.refused == 0 && seen.inputChannels == 1 && seen.outputChannels == 2, "mono in, stereo out");
    // A note effect: no audio output; audio passes through untouched and MIDI is the point.
    configure(OP_KIND_NOTE_EFFECT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, {layout(OpBus{}, OpBus{})});
    Rig fx(AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo, 2, 2, 64);
    fx.useMidiIn = fx.useMidiOut = true;
    for (uint32_t i = 0; i < 64; ++i) {
        fx.in[0][i] = 0.001f * static_cast<float>(i);
        fx.in[1][i] = -0.002f * static_cast<float>(i);
    }
    fx.midiIn.setInput({midiPacket(10, 3, 0x90, 60, 100), midiPacket(20, 3, 0x80, 60, 0)});
    fx.process();
    seen = fx.seen();
    require(seen.refused == 0 && seen.inputChannels == 0 && seen.outputChannels == 0, "a note effect has no engine audio");
    for (uint32_t i = 0; i < 64; ++i) require(fx.out[0][i] == fx.in[0][i] && fx.out[1][i] == fx.in[1][i], "a note effect passes audio through");
    require(fx.midiOut.posted.size() == 2 && fx.midiOut.posted[0].mTimestamp == 10 && fx.midiOut.posted[1].mTimestamp == 20, "and posts its MIDI");
    // Bypass does not stop MIDI: the engine keeps running.
    fx.packet.get()->bypass = 1;
    fx.process();
    fx.process();
    require(fx.midiOut.posted.size() == 2, "MIDI still flows while bypassed");
    std::puts("AAX events proof: mono-in stereo-out, and a note effect (audio passes through, MIDI in and out, MIDI continues in bypass)");
}

static void layoutsAndIds() {
    // An effect with stereo and 5.1 (with a sidechain): components, merge and IDs.
    OpLayout stereo = layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}));
    OpLayout stereoSide = layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}), monoBus());
    OpLayout fiveOne = layout(bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5}), bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5}));
    OpLayout hexa = layout(bus(OP_BUS_DISCRETE, {OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE}), bus(OP_BUS_STEREO, {0, 1}));
    configure(OP_KIND_EFFECT, 0, {stereo, fiveOne, stereoSide, hexa});
    OpShellInfo info{};
    require(op_shell_info(&info), "info");
    const auto configs = buildConfigs(info);
    // stereo (the later layout with a sidechain replaces layout 0), 5.1, and the legacy mono engine; the discrete layout is not offered.
    require(configs.size() == 3, "stereo, 5.1 and legacy mono; the discrete layout is not offered");
    require(configs[0].input == AAX_eStemFormat_Stereo && configs[0].sidechain && configs[0].layout == 2, "the sidechain layout stands for stereo");
    require(configs[1].input == AAX_eStemFormat_5_1 && !configs[1].sidechain && configs[1].layout == 1, "5.1");
    require(configs[2].input == AAX_eStemFormat_Mono && configs[2].layout == -1 && configs[2].pluginId == info.plugin_id_mono, "ABI 1 mono keeps the mono ID");
    require(configs[0].pluginId == info.plugin_id_stereo, "stereo keeps the stereo ID");
    require(configs[1].pluginId != 0 && configs[1].pluginId != info.plugin_id_mono && configs[1].pluginId != info.plugin_id_stereo, "5.1 has an ID of its own");
    for (const Config& c : configs)
        for (int byte = 0; byte < 4; ++byte) require(((c.pluginId >> (8 * byte)) & 0x20) != 0, "every ID byte has bit 0x20 set");
    // Deterministic.
    require(buildConfigs(info)[1].pluginId == configs[1].pluginId, "IDs are stable");
    // A plugin that declares mono itself does not also get the legacy engine, and an instrument never does.
    configure(OP_KIND_EFFECT, 0, {layout(monoBus(), monoBus()), stereo});
    require(buildConfigs(info).size() == 2 && buildConfigs(info)[0].layout == 0, "declared mono and stereo");
    configure(OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT, {layout(OpBus{}, bus(OP_BUS_STEREO, {0, 1}))});
    const auto inst = buildConfigs(info);
    require(inst.size() == 1 && inst[0].input == AAX_eStemFormat_Stereo && inst[0].output == AAX_eStemFormat_Stereo, "an instrument is described stereo in, stereo out");
    // Selecting the layout the config names.
    ProcessState s;
    require(s.prepareStems(48000.0, AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo) && s.in.engine == 0 && s.out.engine == 2, "prepare selects the instrument layout");
    // The legacy path with the default (stereo) plugin: stereo through the layout, mono through the channel-count engine.
    configure(OP_KIND_EFFECT, 0, {stereo});
    ProcessState legacy;
    require(legacy.prepare(48000.0, 1) && legacy.in.engine == 1 && legacy.out.engine == 1, "mono through the ABI 1 engine");
    require(legacy.prepare(48000.0, 2) && legacy.in.engine == 2, "stereo through its layout");
    require(!legacy.prepare(48000.0, 3) && !legacy.prepare(0.0, 2), "bad channel count and rate refused");
    // Library versions: newer is accepted, older is not.
    op_stub_set_abi(OP_SHELL_ABI + 1);
    require(abiOk() && legacy.prepare(48000.0, 2), "a newer library is accepted");
    op_stub_set_abi(OP_SHELL_ABI - 1);
    require(!abiOk() && !legacy.prepare(48000.0, 2), "an older library is refused");
    op_stub_set_abi(OP_SHELL_ABI);
    std::puts("AAX events proof: one component per stem pair from the layouts, sidechain layout merged, IDs derived and stable, instrument stems, ABI 1 mono kept, library version check is >=");
}

int main() {
    instrumentAndMidi();
    splitBlock();
    parameterEvents();
    transportCases();
    sidechain();
    stems();
    differentBuses();
    layoutsAndIds();
    std::printf("AAX events proof: %lu checks. MIDI in/out, parameter events, transport, sidechain, stems in Pro Tools order, note effect. Not a Pro Tools test: PASS\n", g_checks);
    return 0;
}
