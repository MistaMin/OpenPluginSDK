// SPDX-License-Identifier: MIT
// A stand-in for a Rust plugin that implements all of shell ABI 2, for the Audio Unit events
// proof (events_host.cpp) only. It is linked into test bundles and never into a product.
//
// OP_PROBE_KIND: 0 effect, 1 instrument, 2 music effect (an effect with MIDI in and out).
// OP_PROBE_ABI:  the library version it reports (default OP_SHELL_ABI), to test the version check.
//
// Parameters: Gain (0..4, default 1) and Mode (0 or 1).
// Mode 0, per sample and channel c:
//     out[c] = in[c] * Gain + 0.5 * sidechain[0] + L
//   where L = 0.1 * (sum of the velocities of the held notes) / 127. Gain and notes change at
//   the sample offset of their event, so a step in the output shows the sample the event hit.
// Mode 1: out[c] = 0.01 * (role of output channel c + 1), the CLAP surround id of the channel in
//   the selected layout, so the proof sees which role each canonical index carries.
// Notes are echoed to the MIDI output an octave up, and raw MIDI as it came, at their offsets.
// Everything the plugin saw is in the ProbeReport (probe.h).
#include "op_shell.h"
#include "probe.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>

#ifndef OP_PROBE_KIND
#define OP_PROBE_KIND 0
#endif
#ifndef OP_PROBE_ABI
#define OP_PROBE_ABI OP_SHELL_ABI
#endif

namespace {
ProbeReport gReport;

constexpr uint8_t kStereo[] = {0, 1};
constexpr uint8_t k51[] = {0, 1, 2, 3, 4, 5};
constexpr uint8_t k71[] = {0, 1, 2, 3, 9, 10, 4, 5};
constexpr uint8_t k714[] = {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17};

OpBus bus(uint32_t kind, uint32_t channels, const uint8_t* roles) {
    OpBus b{};
    b.kind = kind;
    b.channels = channels;
    std::memset(b.roles, OP_ROLE_NONE, sizeof(b.roles));
    for (uint32_t i = 0; roles && i < channels; ++i) b.roles[i] = roles[i];
    return b;
}
OpBus stereo() { return bus(OP_BUS_STEREO, 2, kStereo); }
OpBus mono() { return bus(OP_BUS_MONO, 1, nullptr); }
OpBus s51() { return bus(OP_BUS_SURROUND, 6, k51); }
OpBus s71() { return bus(OP_BUS_SURROUND, 8, k71); }
OpBus s714() { return bus(OP_BUS_SURROUND, 12, k714); }
OpBus absent() { return bus(OP_BUS_ABSENT, 0, nullptr); }

struct LayoutDef { const char* name; OpBus in, out, side; };
#if OP_PROBE_KIND == 1
const LayoutDef* layouts(uint32_t& count) {
    static const LayoutDef table[] = {
        {"stereo", absent(), stereo(), absent()},
        {"5.1", absent(), s51(), absent()},
        {"mono", absent(), mono(), absent()},
    };
    count = sizeof(table) / sizeof(table[0]);
    return table;
}
#else
const LayoutDef* layouts(uint32_t& count) {
    static const LayoutDef table[] = {
        {"stereo+sidechain", stereo(), stereo(), stereo()},
        {"stereo", stereo(), stereo(), absent()},
        {"mono", mono(), mono(), absent()},
        {"5.1", s51(), s51(), absent()},
        {"7.1", s71(), s71(), absent()},
        {"7.1.4", s714(), s714(), absent()},
        {"stereo to 5.1", stereo(), s51(), absent()},
    };
    count = sizeof(table) / sizeof(table[0]);
    return table;
}
#endif

void copy(char* dst, size_t size, const char* text) {
    std::strncpy(dst, text, size - 1);
    dst[size - 1] = 0;
}

struct Engine {
    float gain = 1.0f;
    float mode = 0.0f;
    uint8_t held[16][128] = {};
    uint32_t layout = 0;
    OpMidiEvent midi[OP_MAX_MIDI_OUT];
    uint32_t midi_count = 0;
    uint32_t midi_read = 0;
    bool offline = false;
};
}

extern "C" {
const ProbeReport* op_probe_report(void) { return &gReport; }
void op_probe_reset(void) { std::memset(&gReport, 0, sizeof(gReport)); }

// ABI 3 parameter text, plain numbers: the proofs here do not look at it.
size_t op_shell_param_format(uint32_t, double plain, char* buf, size_t cap) {
    char text[64];
    const int length = std::snprintf(text, sizeof(text), "%g", plain);
    if (buf && cap > 0) std::snprintf(buf, cap, "%s", text);
    return length < 0 ? 0 : static_cast<size_t>(length);
}
int op_shell_param_parse(uint32_t, const char* text, double* out) {
    if (!text || !out) return 0;
    char* end = nullptr;
    const double value = std::strtod(text, &end);
    if (end == text || *end) return 0;
    *out = value;
    return 1;
}
uint32_t op_shell_abi_version(void) { return OP_PROBE_ABI; }
// ABI 6: the probes never change their latency or parameter info.
uint32_t op_shell_take_changes(void*) { return 0; }
uint32_t op_shell_kind(void) { return OP_PROBE_KIND == 1 ? OP_KIND_INSTRUMENT : OP_KIND_EFFECT; }
uint32_t op_shell_note_ports(void) {
#if OP_PROBE_KIND == 0
    return 0;
#elif OP_PROBE_KIND == 1
    return OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT;
#else
    return OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT;
#endif
}
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = OP_PROBE_ABI;
    out->param_count = 2;
    copy(out->name, sizeof(out->name), "Probe");
    copy(out->vendor, sizeof(out->vendor), "OpenPlugin");
    copy(out->version, sizeof(out->version), "1");
    copy(out->id, sizeof(out->id), "test.probe");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index > 1 || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    if (index == 0) {
        out->min = 0.0;
        out->max = 4.0;
        out->default_plain = 1.0;
        out->default_normalized = 0.25;
        copy(out->name, sizeof(out->name), "Gain");
        copy(out->id, sizeof(out->id), "gain");
    } else {
        out->min = 0.0;
        out->max = 1.0;
        out->default_plain = 0.0;
        out->default_normalized = 0.0;
        out->steps = 2;
        out->flags = OP_PARAM_DISCRETE | OP_PARAM_AUTOMATABLE;
        copy(out->name, sizeof(out->name), "Mode");
        copy(out->id, sizeof(out->id), "mode");
    }
    return 1;
}
double op_shell_to_plain(uint32_t index, double normalized) { return index == 0 ? normalized * 4.0 : normalized; }
double op_shell_to_normalized(uint32_t index, double plain) { return index == 0 ? plain / 4.0 : plain; }
void* op_shell_create(double, uint32_t, uint32_t) {
    ++gReport.created;
    return new (std::nothrow) Engine();
}
void op_shell_destroy(void* instance) { delete static_cast<Engine*>(instance); }
void op_shell_reset(void* instance) {
    if (!instance) return;
    auto* e = static_cast<Engine*>(instance);
    std::memset(e->held, 0, sizeof(e->held));
    e->midi_count = e->midi_read = 0;
}
uint32_t op_shell_latency(void*) { return 0; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index > 1) return 0;
    (index == 0 ? static_cast<Engine*>(instance)->gain : static_cast<Engine*>(instance)->mode) = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t index) {
    if (!instance || index > 1) return 0.0f;
    return index == 0 ? static_cast<Engine*>(instance)->gain : static_cast<Engine*>(instance)->mode;
}
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, index == 0 ? normalized * 4.0f : normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return index == 0 ? op_shell_get_plain(instance, 0) / 4.0f : op_shell_get_plain(instance, index); }

uint32_t op_shell_layout_count(void) {
    uint32_t count = 0;
    layouts(count);
    return count;
}
uint32_t op_shell_layout(uint32_t index, OpLayout* out) {
    uint32_t count = 0;
    const LayoutDef* table = layouts(count);
    if (index >= count || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->input = table[index].in;
    out->output = table[index].out;
    out->sidechain = table[index].side;
    copy(out->name, sizeof(out->name), table[index].name);
    return 1;
}
uint32_t op_shell_set_layout(void* instance, uint32_t index) {
    if (!instance || index >= op_shell_layout_count()) return 0;
    static_cast<Engine*>(instance)->layout = index;
    return 1;
}
uint32_t op_shell_layout_selected(void* instance) { return instance ? static_cast<Engine*>(instance)->layout : 0; }
void op_shell_set_offline(void* instance, uint32_t offline) {
    if (instance) static_cast<Engine*>(instance)->offline = offline != 0;
    gReport.offline = offline;
}

uint32_t op_shell_process_events(void* instance, const OpProcess* args) {
    if (!instance || !args || args->struct_size < sizeof(OpProcess) || !args->outputs || args->event_count > OP_MAX_EVENTS) return 0;
    auto* e = static_cast<Engine*>(instance);
    ++gReport.blocks;
    gReport.last_frames = args->frames;
    gReport.input_channels = args->input_channels;
    gReport.output_channels = args->output_channels;
    gReport.sidechain_channels = args->sidechain_channels;
    gReport.layout_selected = e->layout;
    gReport.have_transport = args->transport != nullptr;
    if (args->transport) gReport.transport = *args->transport;
    std::memset(gReport.input_first, 0, sizeof(gReport.input_first));
    std::memset(gReport.sidechain_first, 0, sizeof(gReport.sidechain_first));
    for (uint32_t c = 0; args->inputs && c < args->input_channels && c < kProbeMaxChannels; ++c) gReport.input_first[c] = args->frames ? args->inputs[c][0] : 0.0f;
    for (uint32_t c = 0; args->sidechain && c < args->sidechain_channels && c < kProbeMaxChannels; ++c) gReport.sidechain_first[c] = args->frames ? args->sidechain[c][0] : 0.0f;
    e->midi_count = e->midi_read = 0;

    // The events in offset order, and kept in the report.
    static thread_local OpEvent sorted[OP_MAX_EVENTS];
    std::memcpy(sorted, args->events, args->event_count * sizeof(OpEvent));
    std::stable_sort(sorted, sorted + args->event_count, [](const OpEvent& a, const OpEvent& b) { return a.offset < b.offset; });
    for (uint32_t i = 0; i < args->event_count && gReport.event_total < kProbeMaxEvents; ++i) {
        gReport.events[gReport.event_total] = args->events[i];
        gReport.event_block[gReport.event_total] = gReport.blocks;
        ++gReport.event_total;
    }

    uint32_t layoutCount = 0;
    const LayoutDef* table = layouts(layoutCount);
    const OpBus& outBus = table[e->layout].out;
    uint32_t next = 0;
    float level = 0.0f;
    for (int ch = 0; ch < 16; ++ch) for (int n = 0; n < 128; ++n) level += 0.1f * e->held[ch][n] / 127.0f;
    for (uint32_t i = 0; i < args->frames; ++i) {
        while (next < args->event_count && sorted[next].offset <= i) {
            const OpEvent& ev = sorted[next++];
            if (ev.offset >= args->frames) continue;
            if (ev.kind == OP_EVENT_PARAM_PLAIN) {
                op_shell_set_plain(instance, ev.index, ev.value);
            } else if (ev.kind == OP_EVENT_NOTE_ON && ev.channel < 16 && ev.note < 128) {
                level -= 0.1f * e->held[ev.channel][ev.note] / 127.0f;
                e->held[ev.channel][ev.note] = ev.velocity;
                level += 0.1f * ev.velocity / 127.0f;
                if (e->midi_count < OP_MAX_MIDI_OUT) e->midi[e->midi_count++] = {ev.offset, {static_cast<uint8_t>(0x90 | ev.channel), static_cast<uint8_t>(ev.note + 12), ev.velocity, 0}};
            } else if (ev.kind == OP_EVENT_NOTE_OFF && ev.channel < 16 && ev.note < 128) {
                level -= 0.1f * e->held[ev.channel][ev.note] / 127.0f;
                e->held[ev.channel][ev.note] = 0;
                if (e->midi_count < OP_MAX_MIDI_OUT) e->midi[e->midi_count++] = {ev.offset, {static_cast<uint8_t>(0x80 | ev.channel), static_cast<uint8_t>(ev.note + 12), ev.velocity, 0}};
            } else if (ev.kind == OP_EVENT_MIDI) {
                if (e->midi_count < OP_MAX_MIDI_OUT) e->midi[e->midi_count++] = {ev.offset, {ev.midi[0], ev.midi[1], ev.midi[2], 0}};
            }
        }
        for (uint32_t c = 0; c < args->output_channels; ++c) {
            float value;
            if (e->mode >= 0.5f) {
                const uint8_t role = c < outBus.channels && outBus.roles[c] != OP_ROLE_NONE ? outBus.roles[c] : static_cast<uint8_t>(c);
                value = 0.01f * (role + 1);
            } else {
                value = level;
                if (args->inputs && c < args->input_channels) value += args->inputs[c][i] * e->gain;
                if (args->sidechain && args->sidechain_channels > 0) value += 0.5f * args->sidechain[0][i];
            }
            args->outputs[c][i] = value;
        }
    }
    return 1;
}
uint32_t op_shell_midi_out(void* instance, OpMidiEvent* out, uint32_t capacity) {
    if (!instance || !out) return 0;
    auto* e = static_cast<Engine*>(instance);
    const uint32_t n = std::min(capacity, e->midi_count - e->midi_read);
    std::memcpy(out, e->midi + e->midi_read, n * sizeof(OpMidiEvent));
    e->midi_read += n;
    return n;
}
uint32_t op_shell_midi_dropped(const void*) { return 0; }

// State: both parameters.
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    if (needed) *needed = 2 * sizeof(float);
    if (!out || capacity < 2 * sizeof(float)) return OP_STATE_TOO_SMALL;
    auto* e = static_cast<Engine*>(instance);
    std::memcpy(out, &e->gain, sizeof(float));
    std::memcpy(out + sizeof(float), &e->mode, sizeof(float));
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length != 2 * sizeof(float)) return 0;
    auto* e = static_cast<Engine*>(instance);
    std::memcpy(&e->gain, data, sizeof(float));
    std::memcpy(&e->mode, data + sizeof(float), sizeof(float));
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
