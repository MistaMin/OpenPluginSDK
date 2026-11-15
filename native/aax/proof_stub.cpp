// SPDX-License-Identifier: MIT
// A stand-in for a plugin's op_shell_* ABI, used only by the AAX bypass,
// parameter-count and allocation proofs. The SDK example reports no latency and
// has two parameters, so it cannot show a delayed bypass or a large packet.
// This one reports OP_STUB_LATENCY samples, declares OP_STUB_PARAMS parameters,
// and outputs input * (p0 + 0.5 * p[last]) (p0 alone with one parameter). It is
// not a plugin and is never shipped.
//
// It also has a plugin-owned chunk, saved and loaded through op_shell_save_state
// and op_shell_load_state, to prove the AAX state chunk carries it. A non-empty
// chunk scales the output by 1 + chunk[0] / 512, so the chunk is audible. The
// blob is: u32 'STUB', u32 parameter count, f32 x count, u32 chunk length,
// chunk bytes. A load checks all of it first; a chunk starting with 0xFF is
// refused, as a plugin's load_state may refuse one. op_stub_* are test hooks.
#include "op_shell.h"
#include "proof_stub.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cctype>
#include <cstring>
#include <new>
#include <string>
#include <vector>

#ifndef OP_STUB_PARAMS
#define OP_STUB_PARAMS 1
#endif
#ifndef OP_STUB_LATENCY
#define OP_STUB_LATENCY 37
#endif

namespace {
constexpr uint32_t kParams = OP_STUB_PARAMS;
constexpr uint32_t kLatency = OP_STUB_LATENCY;
// ABI 2 state shared by every instance: what the plugin declares.
struct Declared {
    uint32_t kind = OP_KIND_EFFECT;
    uint32_t ports = 0;
    std::vector<OpLayout> layouts;
    uint32_t abi = OP_SHELL_ABI;
    bool tag = false;
    char category[128] = "Fx";
    Declared() {
        OpLayout stereo{};
        stereo.input.kind = stereo.output.kind = OP_BUS_STEREO;
        stereo.input.channels = stereo.output.channels = 2;
        for (auto* bus : {&stereo.input, &stereo.output}) {
            std::memset(bus->roles, OP_ROLE_NONE, sizeof bus->roles);
            bus->roles[0] = 0;
            bus->roles[1] = 1;
        }
        std::strcpy(stereo.name, "Stereo");
        layouts.push_back(stereo);
    }
};
Declared& declared() {
    static Declared d;
    return d;
}
constexpr uint32_t kMaxMidi = 256;
struct Stub {
    uint32_t channels = 2;       // op_shell_create's channel count (the ABI 1 engine)
    int32_t layout = -1;         // selected layout, -1 until op_shell_set_layout
    float amp = 0.f;             // note level
    StubSeen seen{};
    OpMidiEvent midi[kMaxMidi];
    uint32_t midiCount = 0;
    std::vector<float> plain;
    std::vector<uint8_t> chunk;
    uint32_t growOnQuery = 0;  // bytes the chunk grows by right after the next size query
    uint32_t loads = 0;
    Stub() : plain(kParams) {}
};
void put(char* dst, size_t size, const char* src) {
    std::memset(dst, 0, size);
    std::strncpy(dst, src, size - 1);
}
constexpr uint32_t kBlobMagic = 0x42555453u;  // 'S','T','U','B'
size_t blobSize(const Stub& stub) { return 8 + 4 * static_cast<size_t>(kParams) + 4 + stub.chunk.size(); }
void put32(uint8_t* out, uint32_t v) { std::memcpy(out, &v, 4); }  // host byte order: the blob never leaves the process
uint32_t get32(const uint8_t* in) { uint32_t v; std::memcpy(&v, in, 4); return v; }
double defaultFor(uint32_t index) { return index == 0 ? 0.5 : 0.25 + 0.001 * index; }
}

extern "C" {
uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->abi = declared().abi;
    out->param_count = kParams;
    out->manufacturer_id = 0x6f706d31;
    out->product_id = 0x6f707031;
    out->plugin_id_mono = 0x6f706d6f;
    out->plugin_id_stereo = 0x6f707374;
    for (int i = 0; i < 16; ++i) {
        out->processor_cid[i] = static_cast<uint8_t>(0x10 + i);
        out->controller_cid[i] = static_cast<uint8_t>(0x80 + i);
    }
    put(out->name, sizeof(out->name), "AAX Proof Stub");
    put(out->vendor, sizeof(out->vendor), "OpenPlugin proofs");
    put(out->version, sizeof(out->version), "0.0.0");
    put(out->category, sizeof(out->category), declared().category);
    put(out->id, sizeof(out->id), "proof.aax-stub");
    return 1;
}
uint32_t op_shell_param(uint32_t index, OpParamDesc* out) {
    if (index >= kParams || !out) return 0;
    std::memset(out, 0, sizeof(*out));
    out->min = 0.0;
    out->max = 1.0;
    out->default_plain = defaultFor(index);
    out->default_normalized = defaultFor(index);
    out->flags = OP_PARAM_AUTOMATABLE;
#ifdef OP_STUB_TEXT
    // The text proof: 0 Gain -24..24 dB, 1 a three-way choice, 2 a toggle, 3 a value whose text is multi-byte.
    if (index == 0) { out->min = -24.0; out->max = 24.0; out->default_plain = out->default_normalized = 0.0; std::strcpy(out->unit, "dB"); }
    if (index == 1) { out->max = 2.0; out->steps = 3; out->flags |= OP_PARAM_DISCRETE; out->default_plain = out->default_normalized = 0.0; }
    if (index == 2) { out->steps = 2; out->flags |= OP_PARAM_DISCRETE; out->default_plain = out->default_normalized = 0.0; }
    if (index == 3) { out->default_plain = out->default_normalized = 0.5; }
#endif
    std::snprintf(out->name, sizeof(out->name), "Param %u", index);
    std::snprintf(out->id, sizeof(out->id), "p%u", index);
    return 1;
}
#ifdef OP_STUB_TEXT
double op_shell_to_plain(uint32_t index, double normalized) { return index == 0 ? -24.0 + 48.0 * normalized : index == 1 ? 2.0 * normalized : normalized; }
double op_shell_to_normalized(uint32_t index, double plain) { return index == 0 ? (plain + 24.0) / 48.0 : index == 1 ? plain / 2.0 : plain; }
#else
double op_shell_to_plain(uint32_t, double normalized) { return normalized; }
double op_shell_to_normalized(uint32_t, double plain) { return plain; }
#endif
void* op_shell_create(double, uint32_t, uint32_t channels) {
    auto* stub = new (std::nothrow) Stub;
    if (stub) {
        stub->channels = channels;
        for (uint32_t i = 0; i < kParams; ++i) stub->plain[i] = static_cast<float>(defaultFor(i));
    }
    return stub;
}
void op_shell_destroy(void* instance) { delete static_cast<Stub*>(instance); }
void op_shell_reset(void*) {}
uint32_t op_shell_latency(void*) { return kLatency; }
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain) {
    if (!instance || index >= kParams) return 0;
    static_cast<Stub*>(instance)->plain[index] = plain;
    return 1;
}
float op_shell_get_plain(void* instance, uint32_t index) {
    return instance && index < kParams ? static_cast<Stub*>(instance)->plain[index] : 0.0f;
}
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized) { return op_shell_set_plain(instance, index, normalized); }
float op_shell_get_normalized(void* instance, uint32_t index) { return op_shell_get_plain(instance, index); }
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames) {
    if (!instance || !in_left || !out_left) return 0;
    const Stub& stub = *static_cast<Stub*>(instance);
    float gain = kParams > 1 ? stub.plain[0] + 0.5f * stub.plain[kParams - 1] : stub.plain[0];
    if (!stub.chunk.empty()) gain *= 1.f + static_cast<float>(stub.chunk[0]) / 512.f;
    for (uint32_t i = 0; i < frames; ++i) out_left[i] = in_left[i] * gain;
    if (in_right && out_right)
        for (uint32_t i = 0; i < frames; ++i) out_right[i] = in_right[i] * gain;
    return 1;
}
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed) {
    if (!instance || (!out && capacity > 0)) return OP_STATE_FAILED;
    Stub& stub = *static_cast<Stub*>(instance);
    const size_t size = blobSize(stub);
    if (needed) *needed = static_cast<uint32_t>(size);
    if (!out || capacity < size) {
        if (!out && stub.growOnQuery) {
            stub.chunk.insert(stub.chunk.end(), stub.growOnQuery, 0x11);
            stub.growOnQuery = 0;
        }
        return OP_STATE_TOO_SMALL;
    }
    put32(out, kBlobMagic);
    put32(out + 4, kParams);
    for (uint32_t i = 0; i < kParams; ++i) std::memcpy(out + 8 + 4 * i, &stub.plain[i], 4);
    put32(out + 8 + 4 * kParams, static_cast<uint32_t>(stub.chunk.size()));
    if (!stub.chunk.empty()) std::memcpy(out + 12 + 4 * kParams, stub.chunk.data(), stub.chunk.size());
    return OP_STATE_OK;
}
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || !data || length < 12 + 4 * kParams) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    if (get32(data) != kBlobMagic || get32(data + 4) != kParams) return 0;
    const uint32_t chunkLength = get32(data + 8 + 4 * kParams);
    if (length - (12 + 4 * kParams) != chunkLength) return 0;
    std::vector<float> plain(kParams);
    for (uint32_t i = 0; i < kParams; ++i) {
        std::memcpy(&plain[i], data + 8 + 4 * i, 4);
        if (!(plain[i] >= 0.f && plain[i] <= 1.f)) return 0;
    }
    const uint8_t* chunk = data + 12 + 4 * kParams;
    if (chunkLength > 0 && chunk[0] == 0xFF) return 0;
    stub.plain = plain;
    stub.chunk.assign(chunk, chunk + chunkLength);
    ++stub.loads;
    return 1;
}
// Test hooks, not part of the shell ABI.
uint32_t op_stub_set_chunk(void* instance, const uint8_t* data, uint32_t length) {
    if (!instance || (!data && length)) return 0;
    static_cast<Stub*>(instance)->chunk.assign(data, data + length);
    return 1;
}
uint32_t op_stub_chunk(void* instance, uint8_t* out, uint32_t capacity) {
    if (!instance) return 0;
    const Stub& stub = *static_cast<Stub*>(instance);
    if (out && capacity >= stub.chunk.size() && !stub.chunk.empty()) std::memcpy(out, stub.chunk.data(), stub.chunk.size());
    return static_cast<uint32_t>(stub.chunk.size());
}
uint32_t op_stub_loads(void* instance) { return instance ? static_cast<Stub*>(instance)->loads : 0; }
void op_stub_grow_on_query(void* instance, uint32_t bytes) {
    if (instance) static_cast<Stub*>(instance)->growOnQuery = bytes;
}
// ---- ABI 2 ----
uint32_t op_shell_abi_version(void) { return declared().abi; }
uint32_t op_shell_kind(void) { return declared().kind; }
uint32_t op_shell_note_ports(void) { return declared().ports; }
uint32_t op_shell_layout_count(void) { return static_cast<uint32_t>(declared().layouts.size()); }
uint32_t op_shell_layout(uint32_t index, OpLayout* out) {
    if (!out || index >= declared().layouts.size()) return 0;
    *out = declared().layouts[index];
    return 1;
}
uint32_t op_shell_set_layout(void* instance, uint32_t index) {
    if (!instance || index >= declared().layouts.size()) return 0;
    static_cast<Stub*>(instance)->layout = static_cast<int32_t>(index);
    return 1;
}
uint32_t op_shell_layout_selected(void* instance) {
    return instance && static_cast<Stub*>(instance)->layout > 0 ? static_cast<uint32_t>(static_cast<Stub*>(instance)->layout) : 0;
}
void op_shell_set_offline(void* instance, uint32_t offline) {
    if (instance) static_cast<Stub*>(instance)->seen.offline = offline;
}
uint32_t op_shell_midi_dropped(const void*) { return 0; }
uint32_t op_shell_midi_out(void* instance, OpMidiEvent* out, uint32_t capacity) {
    if (!instance) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    const uint32_t n = std::min(capacity, stub.midiCount);
    for (uint32_t i = 0; i < n; ++i) out[i] = stub.midi[i];
    // What is not read stays for the next call; shift the rest down.
    for (uint32_t i = n; i < stub.midiCount; ++i) stub.midi[i - n] = stub.midi[i];
    stub.midiCount -= n;
    return n;
}
// A note-driven stand-in: note on sets a level (velocity / 127) from its sample, note off clears it;
// the level is added to every output channel. A note or raw MIDI event is echoed on the MIDI output (notes
// an octave up). The first sidechain channel is added to output 0.
uint32_t op_shell_process_events(void* instance, const OpProcess* args) {
    if (!instance || !args || args->struct_size < sizeof(OpProcess)) return 0;
    Stub& stub = *static_cast<Stub*>(instance);
    StubSeen& seen = stub.seen;
    OpBus in{}, out{}, side{};
    if (stub.layout >= 0) {
        in = declared().layouts[stub.layout].input;
        out = declared().layouts[stub.layout].output;
        side = declared().layouts[stub.layout].sidechain;
    } else {
        in.kind = out.kind = OP_BUS_STEREO;
        in.channels = out.channels = stub.channels;
    }
    ++seen.calls;
    seen.frames = args->frames;
    seen.inputChannels = args->input_channels;
    seen.outputChannels = args->output_channels;
    seen.sidechainChannels = args->sidechain ? args->sidechain_channels : 0;
    if (args->input_channels != in.channels || args->output_channels != out.channels ||
        (args->sidechain && side.kind == OP_BUS_ABSENT)) {
        ++seen.refused;
        return 0;
    }
    if (args->transport) {
        ++seen.transportCalls;
        seen.transport = *args->transport;
    } else {
        ++seen.transportMissing;
    }
    for (uint32_t k = 0; k < OP_STUB_CHANNELS; ++k) seen.input0[k] = k < args->input_channels ? args->inputs[k][0] : 0.f;
    seen.sidechain0 = args->sidechain && args->sidechain_channels ? args->sidechain[0][0] : 0.f;
    stub.midiCount = 0;
    const auto record = [&](const OpEvent& e) {
        if (seen.logCount >= OP_STUB_LOG) return;
        StubLogEvent& l = seen.log[seen.logCount++];
        l.position = seen.frameCounter + e.offset;
        l.kind = e.kind;
        l.index = e.index;
        l.value = e.value;
        l.channel = e.channel;
        l.note = e.note;
        l.velocity = e.velocity;
        std::memcpy(l.midi, e.midi, 4);
    };
    const auto echo = [&](uint32_t offset, uint8_t a, uint8_t b, uint8_t c) {
        if (stub.midiCount < kMaxMidi) {
            OpMidiEvent& m = stub.midi[stub.midiCount++];
            m.offset = offset;
            m.data[0] = a;
            m.data[1] = b;
            m.data[2] = c;
            m.data[3] = 0;
        }
    };
    for (uint32_t i = 0; i < args->event_count; ++i) record(args->events[i]);
    for (uint32_t i = 0; i < args->frames; ++i) {
        // Events at sample i, in the order they were given.
        for (uint32_t e = 0; e < args->event_count; ++e) {
            const OpEvent& ev = args->events[e];
            if (ev.offset != i) continue;
            switch (ev.kind) {
            case OP_EVENT_NOTE_ON:
                stub.amp = static_cast<float>(ev.velocity) / 127.f;
                echo(i, static_cast<uint8_t>(0x90 | ev.channel), static_cast<uint8_t>(ev.note + 12), ev.velocity);
                break;
            case OP_EVENT_NOTE_OFF:
                stub.amp = 0.f;
                echo(i, static_cast<uint8_t>(0x80 | ev.channel), static_cast<uint8_t>(ev.note + 12), ev.velocity);
                break;
            case OP_EVENT_MIDI: echo(i, ev.midi[0], ev.midi[1], ev.midi[2]); break;
            case OP_EVENT_PARAM_PLAIN:
            case OP_EVENT_PARAM_NORMALIZED:
                if (ev.index < kParams) stub.plain[ev.index] = ev.value;
                break;
            }
        }
        float gain = kParams > 1 ? stub.plain[0] + 0.5f * stub.plain[kParams - 1] : stub.plain[0];
        if (!stub.chunk.empty()) gain *= 1.f + static_cast<float>(stub.chunk[0]) / 512.f;
        for (uint32_t k = 0; k < args->output_channels; ++k) {
            float v = (k < args->input_channels ? args->inputs[k][i] * gain : 0.f) + stub.amp;
            if (declared().tag) v += 100.f * static_cast<float>(k + 1);
            if (k == 0 && args->sidechain && args->sidechain_channels) v += args->sidechain[0][i];
            args->outputs[k][i] = v;
        }
    }
    seen.frameCounter += args->frames;
    return 1;
}
void op_stub_configure(uint32_t kind, uint32_t ports, uint32_t count, const OpLayout* layouts) {
    declared().kind = kind;
    declared().ports = ports;
    declared().layouts.assign(layouts, layouts + count);
}
void op_stub_set_abi(uint32_t version) { declared().abi = version; }
void op_stub_tag(uint32_t on) { declared().tag = on != 0; }
void op_stub_set_category(const char* category) { put(declared().category, sizeof declared().category, category); }
void op_stub_seen(void* instance, StubSeen* out) {
    if (instance && out) *out = static_cast<Stub*>(instance)->seen;
}
uint32_t op_stub_param_count(void) { return kParams; }
// ---- ABI 3: parameter text. Plain text "%.1f" except in the text proof (see op_shell_param above). ----
size_t op_shell_param_format(uint32_t index, double plain, char* buf, size_t cap) {
    if (index >= kParams) {
        if (buf && cap) buf[0] = '\0';
        return 0;
    }
    if (!std::isfinite(plain)) plain = defaultFor(index);
    char text[64];
#ifdef OP_STUB_TEXT
    static const char* const kChoice[] = {"Hall", "Plate", "Room"};
    if (index == 0) std::snprintf(text, sizeof text, "%.1f dB", plain);
    else if (index == 1) std::snprintf(text, sizeof text, "%s", kChoice[std::min(2, std::max(0, static_cast<int>(std::lround(plain))))]);
    else if (index == 2) std::snprintf(text, sizeof text, "%s", plain >= 0.5 ? "On" : "Off");
    else std::snprintf(text, sizeof text, "\xE6\x97\xA5\xE6\x9C\xAC\xE8\xAA\x9E %.1f", plain);  // "日本語 0.5"
#else
    std::snprintf(text, sizeof text, "%.3f", plain);
#endif
    const size_t length = std::strlen(text);
    if (buf && cap) {
        size_t n = std::min(length, cap - 1);
        while (n > 0 && n < length && (static_cast<unsigned char>(text[n]) & 0xC0) == 0x80) --n;
        std::memcpy(buf, text, n);
        buf[n] = '\0';
    }
    return length;
}
int op_shell_param_parse(uint32_t index, const char* text, double* out_plain) {
    if (!text || !out_plain || index >= kParams) return 0;
#ifdef OP_STUB_TEXT
    if (index == 1 || index == 2) {
        static const char* const kChoice[] = {"hall", "plate", "room"};
        static const char* const kToggle[] = {"off", "on"};
        std::string lower(text);
        for (auto& c : lower) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        const int count = index == 1 ? 3 : 2;
        for (int i = 0; i < count; ++i)
            if (lower == (index == 1 ? kChoice[i] : kToggle[i])) { *out_plain = i; return 1; }
        return 0;
    }
#endif
    char* end = nullptr;
    const double value = std::strtod(text, &end);
    if (end == text || !std::isfinite(value)) return 0;
    while (*end == ' ') ++end;
    if (*end != '\0' && std::strcmp(end, "dB") != 0) return 0;
#ifdef OP_STUB_TEXT
    *out_plain = index == 0 ? std::min(24.0, std::max(-24.0, value)) : std::min(1.0, std::max(0.0, value));
#else
    *out_plain = std::min(1.0, std::max(0.0, value));
#endif
    return 1;
}
#if OP_HAS_TAIL
double op_shell_tail_seconds(void*) { return 0.0; }
#endif
}
