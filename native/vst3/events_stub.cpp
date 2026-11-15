// SPDX-License-Identifier: MIT
// A stand-in plugin ABI (ABI 2) for the VST3 events proof. Test builds only; never
// shipped. It does what makes the shell's edge observable:
//   - Notes: out0 = in0 + sidechain0 + a level that follows the held note,
//     (note + 1) / 128, switching at the event's sample offset.
//   - MIDI out: every note-on and note-off is echoed an octave up at the same
//     offset; raw MIDI (controllers, pitch bend, pressure) is echoed unchanged.
//   - Mode parameter (index 0, normalized): at 0.5 or more every output
//     channel i passes its input only if the input holds the tag of the speaker
//     the layout names for channel i, 0.01 * (role + 1); otherwise it is -1.
//   - The whole plugin state is an EventsProbe: the last transport, offline
//     flag, bus widths and event count, so the host test reads what arrived.
// Variants (STUB_VARIANT): 0 effect with a sidechain and surround layouts,
// 1 instrument, 2 note effect (no audio at all).
#include "events_probe.h"
#include <cmath>
#include <cstring>
#include <new>

#ifndef STUB_VARIANT
#define STUB_VARIANT 0
#endif

namespace {
struct Stub {
    float mode = 0.0f;
    int note = -1;
    uint32_t selected = 0;
    bool pending_offline = false;
    EventsProbe probe{};
    OpMidiEvent midi[OP_MAX_MIDI_OUT];
    uint32_t midi_count = 0;
};

void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
struct Spec {
    OpBus in, out, side;
    const char* name;
};
OpBus bus(uint32_t kind, uint32_t channels, const uint8_t* roles) {
    OpBus b{};
    b.kind = kind;
    b.channels = channels;
    std::memset(b.roles, OP_ROLE_NONE, sizeof(b.roles));
    if (roles)
        for (uint32_t i = 0; i < channels; ++i) b.roles[i] = roles[i];
    return b;
}
const uint8_t kStereoRoles[] = {0, 1};
const uint8_t k51Roles[] = {0, 1, 2, 3, 4, 5};
const uint8_t k71Roles[] = {0, 1, 2, 3, 9, 10, 4, 5};  // FL FR FC LFE SL SR BL BR
const uint8_t k714Roles[] = {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17};
OpBus none() { return bus(OP_BUS_ABSENT, 0, nullptr); }
OpBus stereo() { return bus(OP_BUS_STEREO, 2, kStereoRoles); }
OpBus mono() { return bus(OP_BUS_MONO, 1, nullptr); }
OpBus s51() { return bus(OP_BUS_SURROUND, 6, k51Roles); }
OpBus s71() { return bus(OP_BUS_SURROUND, 8, k71Roles); }
OpBus s714() { return bus(OP_BUS_SURROUND, 12, k714Roles); }

#if STUB_VARIANT == 0
const char* kName = "Events Stub";
const char* kCategory = "Fx|Tools";
constexpr uint32_t kKind = OP_KIND_EFFECT;
constexpr uint32_t kPorts = OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT;
const Spec* specs(uint32_t& count) {
    static const Spec table[] = {
        {stereo(), stereo(), stereo(), "Stereo + sidechain"}, {stereo(), stereo(), none(), "Stereo"}, {mono(), mono(), none(), "Mono"},
        {s51(), s51(), none(), "5.1"},                          {s71(), s71(), none(), "7.1"},          {s714(), s714(), none(), "7.1.4"},
    };
    count = sizeof(table) / sizeof(table[0]);
    return table;
}
#elif STUB_VARIANT == 1
const char* kName = "Synth Stub";
const char* kCategory = "Instrument";
constexpr uint32_t kKind = OP_KIND_INSTRUMENT;
constexpr uint32_t kPorts = OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT;
const Spec* specs(uint32_t& count) {
    static const Spec table[] = {{none(), stereo(), none(), "Stereo out"}, {none(), s51(), none(), "5.1 out"}};
    count = sizeof(table) / sizeof(table[0]);
    return table;
}
#else
const char* kName = "Note Stub";
const char* kCategory = "Fx";
constexpr uint32_t kKind = OP_KIND_NOTE_EFFECT;
constexpr uint32_t kPorts = OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT;
const Spec* specs(uint32_t& count) {
    static const Spec table[] = {{none(), none(), none(), "Notes only"}};
    count = sizeof(table) / sizeof(table[0]);
    return table;
}
#endif

void echo(Stub& s, uint32_t offset, uint8_t a, uint8_t b, uint8_t c) {
    if (s.midi_count >= OP_MAX_MIDI_OUT) return;
    OpMidiEvent& m = s.midi[s.midi_count++];
    m.offset = offset;
    m.data[0] = a;
    m.data[1] = b;
    m.data[2] = c;
    m.data[3] = 0;
}
}  // namespace

extern "C" {
uint32_t op_shell_abi_version(void) { return OP_SHELL_ABI; }
uint32_t op_shell_kind(void) { return kKind; }
uint32_t op_shell_note_ports(void) { return kPorts; }
uint32_t op_shell_layout_count(void) {
    uint32_t n = 0;
    specs(n);
    return n;
}
uint32_t op_shell_layout(uint32_t index, OpLayout* out) {
    uint32_t n = 0;
    const Spec* table = specs(n);
    if (!out || index >= n) return 0;
    std::memset(out, 0, sizeof(*out));
    out->input = table[index].in;
    out->output = table[index].out;
    out->sidechain = table[index].side;
    put(out->name, sizeof(out->name), table[index].name);
    return 1;
}
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_SHELL_ABI;
    out->param_count = 1;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x30 + STUB_VARIANT * 0x10 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0xA0 + STUB_VARIANT * 0x10 + i);
    }
    put(out->name, sizeof(out->name), kName);
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), kCategory);
    put(out->id, sizeof(out->id), kName);
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index != 0 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 1.0;
    out->default_plain = 0.0;
    out->default_normalized = 0.0;
    out->flags = OP_PARAM_AUTOMATABLE;
    put(out->name, sizeof(out->name), "Mode");
    put(out->id, sizeof(out->id), "mode");
    return 1;
}
double op_shell_to_plain(uint32_t, double normalized) { return normalized; }
double op_shell_to_normalized(uint32_t, double plain) { return plain; }
void* op_shell_create(double, uint32_t, uint32_t) { return new (std::nothrow) Stub; }
void op_shell_destroy(void* instance) { delete static_cast<Stub*>(instance); }
void op_shell_reset(void* instance) {
    if (instance) static_cast<Stub*>(instance)->note = -1;
}
uint32_t op_shell_latency(void*) { return 0; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index != 0) return 0;
    static_cast<Stub*>(instance)->mode = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t) { return instance ? static_cast<Stub*>(instance)->mode : 0.0f; }
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void*, const float*, const float*, float*, float*, uint32_t) { return 0; }
uint32_t op_shell_set_layout(void* instance, uint32_t index) {
    uint32_t n = 0;
    specs(n);
    if (!instance || index >= n) return 0;
    static_cast<Stub*>(instance)->selected = index;
    return 1;
}
uint32_t op_shell_layout_selected(void* instance) { return instance ? static_cast<Stub*>(instance)->selected : 0; }
void op_shell_set_offline(void* instance, uint32_t offline) {
    if (instance) static_cast<Stub*>(instance)->pending_offline = offline != 0;
}
uint32_t op_shell_midi_out(void* instance, OpMidiEvent* out, uint32_t capacity) {
    if (!instance || !out) return 0;
    Stub& s = *static_cast<Stub*>(instance);
    const uint32_t n = s.midi_count < capacity ? s.midi_count : capacity;
    std::memcpy(out, s.midi, n * sizeof(OpMidiEvent));
    return n;
}
uint32_t op_shell_midi_dropped(const void*) { return 0; }

uint32_t op_shell_process_events(void* instance, const OpProcess* a) {
    if (!instance || !a || a->struct_size < sizeof(OpProcess)) return 0;
    Stub& s = *static_cast<Stub*>(instance);
    uint32_t n = 0;
    const Spec* table = specs(n);
    const Spec& spec = table[s.selected];
    s.midi_count = 0;
    EventsProbe& p = s.probe;
    p.magic = kEventsProbeMagic;
    p.offline = s.pending_offline ? 1 : 0;
    if (a->transport) {
        p.have_transport = 1;
        p.transport = *a->transport;
    }
    p.sidechain_channels = a->sidechain ? a->sidechain_channels : 0;
    p.input_channels = a->inputs ? a->input_channels : 0;
    p.output_channels = a->outputs ? a->output_channels : 0;
    p.event_count = a->event_count;
    p.frames = a->frames;
    p.layout = s.selected;
    ++p.blocks;
    uint32_t next = 0;
    for (uint32_t i = 0; i < a->frames; ++i) {
        while (next < a->event_count && a->events[next].offset <= i) {
            const OpEvent& e = a->events[next++];
            if (e.kind == OP_EVENT_NOTE_ON) {
                s.note = e.note;
                echo(s, e.offset, static_cast<uint8_t>(0x90 | e.channel), static_cast<uint8_t>(e.note + 12), e.velocity);
            } else if (e.kind == OP_EVENT_NOTE_OFF) {
                if (s.note == e.note) s.note = -1;
                echo(s, e.offset, static_cast<uint8_t>(0x80 | e.channel), static_cast<uint8_t>(e.note + 12), e.velocity);
            } else if (e.kind == OP_EVENT_MIDI) {
                echo(s, e.offset, e.midi[0], e.midi[1], e.midi[2]);
            } else if (e.kind == OP_EVENT_PARAM_NORMALIZED || e.kind == OP_EVENT_PARAM_PLAIN) {
                if (e.index == 0) s.mode = e.value;
            }
        }
        const float level = s.note >= 0 ? (static_cast<float>(s.note) + 1.0f) / 128.0f : 0.0f;
        for (uint32_t c = 0; c < a->output_channels && a->outputs; ++c) {
            float v = a->inputs && c < a->input_channels && a->inputs[c] ? a->inputs[c][i] : 0.0f;
            if (s.mode >= 0.5f) {
                const float expected = spec.out.roles[c] == OP_ROLE_NONE ? v : 0.01f * (static_cast<float>(spec.out.roles[c]) + 1.0f);
                v = std::fabs(v - expected) < 1e-6f ? v : -1.0f;
            } else if (c == 0) {
                if (a->sidechain && a->sidechain_channels > 0 && a->sidechain[0]) v += a->sidechain[0][i];
                v += level;
            }
            a->outputs[c][i] = v;
        }
    }
    return 1;
}

uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance) return OP_STATE_FAILED;
    if (needed) *needed = sizeof(EventsProbe);
    if (!out || capacity < sizeof(EventsProbe)) return OP_STATE_TOO_SMALL;
    std::memcpy(out, &static_cast<Stub*>(instance)->probe, sizeof(EventsProbe));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t*, uint32_t length) { return instance && length == sizeof(EventsProbe); }
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
