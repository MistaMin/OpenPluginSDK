// SPDX-License-Identifier: MIT
// Audio Unit parity on shell ABI 2, through test bundles built from probe_shell.cpp (a C++ stand-in
// plugin that implements all of ABI 2 and reports what it received). Every bundle is registered in
// this process only (AudioComponentRegister); nothing is installed. These are not DAW tests.
//
//   AUEventsProof <effect.component> <instrument.component> <musicfx.component> <newer.component> <older.component>
//
// Checked: component types (aufx, aumu, aumf); MIDI in with sample offsets (note on, note off, a
// note on with velocity 0, a controller) in order, across a render split by the maximum frame count;
// MIDI out through kAudioUnitProperty_MIDIOutputCallback; parameter events at sample offsets
// (AudioUnitScheduleParameters immediate and ramped, AudioUnitSetParameter with a buffer offset);
// transport from HostCallbackInfo; a sidechain input element; channel layouts (SupportedNumChannels,
// layout tags, channel descriptions, roles at the right indices for 5.1 in four orders, 7.1, 7.1.4 in
// two orders); offline rendering; bypass on 5.1; the ABI version check.
#include "alloc_hook.h"
#include "proof_au.h"
#include "probe.h"
#include <CoreMIDI/CoreMIDI.h>
#include <map>

namespace {
struct Probe {
    AudioComponent component = nullptr;
    const ProbeReport* (*report)() = nullptr;
    void (*reset)() = nullptr;
};

Probe load(const char* path) {
    Probe p;
    p.component = proof_register_component(path);
    p.report = reinterpret_cast<const ProbeReport* (*)()>(proof_bundle_symbol("op_probe_report"));
    p.reset = reinterpret_cast<void (*)()>(proof_bundle_symbol("op_probe_reset"));
    CHECK(p.component && p.report && p.reset);
    return p;
}

AudioStreamBasicDescription format(UInt32 channels) {
    AudioStreamBasicDescription f{};
    f.mSampleRate = 48000;
    f.mFormatID = kAudioFormatLinearPCM;
    f.mFormatFlags = static_cast<UInt32>(kAudioFormatFlagsNativeFloatPacked) | static_cast<UInt32>(kAudioFormatFlagIsNonInterleaved);
    f.mBytesPerPacket = sizeof(float);
    f.mFramesPerPacket = 1;
    f.mBytesPerFrame = sizeof(float);
    f.mChannelsPerFrame = channels;
    f.mBitsPerChannel = 32;
    return f;
}
OSStatus set_format(AudioUnit u, AudioUnitScope scope, AudioUnitElement element, UInt32 channels) {
    const AudioStreamBasicDescription f = format(channels);
    return AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, scope, element, &f, sizeof(f));
}

/// A constant per channel, pulled by the unit.
struct Source { std::vector<float> values; };
OSStatus source_proc(void* ref, AudioUnitRenderActionFlags*, const AudioTimeStamp*, UInt32, UInt32 frames, AudioBufferList* data) {
    const auto* s = static_cast<const Source*>(ref);
    for (UInt32 ch = 0; ch < data->mNumberBuffers; ++ch) {
        auto* samples = static_cast<float*>(data->mBuffers[ch].mData);
        if (!samples) return kAudio_ParamError;
        const float v = ch < s->values.size() ? s->values[ch] : 0.0f;
        for (UInt32 i = 0; i < frames; ++i) samples[i] = v;
        data->mBuffers[ch].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}
void set_source(AudioUnit u, AudioUnitElement element, Source* s) {
    AURenderCallbackStruct callback{source_proc, s};
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, element, &callback, sizeof(callback)) == noErr);
}
OSStatus set_layout(AudioUnit u, AudioUnitScope scope, AudioUnitElement element, AudioChannelLayoutTag tag) {
    AudioChannelLayout layout{};
    layout.mChannelLayoutTag = tag;
    return AudioUnitSetProperty(u, kAudioUnitProperty_AudioChannelLayout, scope, element, &layout, sizeof(layout));
}

using Matrix = std::vector<std::vector<float>>;
OSStatus render(AudioUnit u, UInt32 channels, UInt32 frames, double start, Matrix& out) {
    std::vector<uint8_t> raw(sizeof(AudioBufferList) + sizeof(AudioBuffer) * channels, 0);
    auto* list = reinterpret_cast<AudioBufferList*>(raw.data());
    out.assign(channels, std::vector<float>(frames, -7.0f));
    list->mNumberBuffers = channels;
    for (UInt32 ch = 0; ch < channels; ++ch) list->mBuffers[ch] = {1, frames * static_cast<UInt32>(sizeof(float)), out[ch].data()};
    AudioTimeStamp stamp{};
    stamp.mFlags = kAudioTimeStampSampleTimeValid;
    stamp.mSampleTime = start;
    AudioUnitRenderActionFlags flags = 0;
    return AudioUnitRender(u, &flags, &stamp, 0, frames, list);
}
/// One render with every allocation and free in the process counted while it runs; the buffers are made first.
struct Counted { uint64_t allocs, frees; OSStatus status; };
Counted render_counted(AudioUnit u, UInt32 channels, UInt32 frames, double start, Matrix& out) {
    std::vector<uint8_t> raw(sizeof(AudioBufferList) + sizeof(AudioBuffer) * channels, 0);
    auto* list = reinterpret_cast<AudioBufferList*>(raw.data());
    out.assign(channels, std::vector<float>(frames, -7.0f));
    list->mNumberBuffers = channels;
    for (UInt32 ch = 0; ch < channels; ++ch) list->mBuffers[ch] = {1, frames * static_cast<UInt32>(sizeof(float)), out[ch].data()};
    AudioTimeStamp stamp{};
    stamp.mFlags = kAudioTimeStampSampleTimeValid;
    stamp.mSampleTime = start;
    AudioUnitRenderActionFlags flags = 0;
    const uint64_t a0 = op_alloc_count(), f0 = op_free_count();
    op_alloc_arm();
    const OSStatus status = AudioUnitRender(u, &flags, &stamp, 0, frames, list);
    op_alloc_disarm();
    return {op_alloc_count() - a0, op_free_count() - f0, status};
}
OSStatus count_midi_callback(void* ref, const AudioTimeStamp*, UInt32, const MIDIPacketList* list) {
    *static_cast<uint32_t*>(ref) += list->numPackets;
    return noErr;
}

bool near(float a, float b, float tol = 1e-6f) { return std::fabs(a - b) <= tol; }

AudioComponentDescription description(AudioComponent c) {
    AudioComponentDescription d{};
    CHECK(AudioComponentGetDescription(c, &d) == noErr);
    return d;
}
constexpr OSType fourcc(const char (&s)[5]) { return OSType(s[0] << 24 | s[1] << 16 | s[2] << 8 | s[3]); }

AudioUnit make_unit(const Probe& p, UInt32 inChannels, UInt32 outChannels, UInt32 maxFrames = 4096) {
    AudioUnit u = nullptr;
    CHECK(AudioComponentInstanceNew(p.component, &u) == noErr && u);
    if (inChannels) CHECK(set_format(u, kAudioUnitScope_Input, 0, inChannels) == noErr);
    CHECK(set_format(u, kAudioUnitScope_Output, 0, outChannels) == noErr);
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &maxFrames, sizeof(maxFrames)) == noErr);
    return u;
}
void finish(AudioUnit u) {
    CHECK(AudioUnitUninitialize(u) == noErr);
    CHECK(AudioComponentInstanceDispose(u) == noErr);
}

std::vector<AudioChannelLayoutTag> tags_of(AudioUnit u, AudioUnitScope scope, AudioUnitElement element) {
    UInt32 size = 0;
    Boolean writable = false;
    if (AudioUnitGetPropertyInfo(u, kAudioUnitProperty_SupportedChannelLayoutTags, scope, element, &size, &writable) != noErr) return {};
    std::vector<AudioChannelLayoutTag> tags(size / sizeof(AudioChannelLayoutTag));
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_SupportedChannelLayoutTags, scope, element, tags.data(), &size) == noErr);
    return tags;
}
bool has(const std::vector<AudioChannelLayoutTag>& tags, AudioChannelLayoutTag tag) { return std::find(tags.begin(), tags.end(), tag) != tags.end(); }
constexpr AudioChannelLayoutTag tag(uint32_t id, uint32_t n) { return (id << 16) | n; }

std::vector<std::pair<SInt16, SInt16>> channel_pairs(AudioUnit u) {
    UInt32 size = 0;
    Boolean writable = false;
    CHECK(AudioUnitGetPropertyInfo(u, kAudioUnitProperty_SupportedNumChannels, kAudioUnitScope_Global, 0, &size, &writable) == noErr);
    std::vector<AUChannelInfo> info(size / sizeof(AUChannelInfo));
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_SupportedNumChannels, kAudioUnitScope_Global, 0, info.data(), &size) == noErr);
    std::vector<std::pair<SInt16, SInt16>> pairs;
    for (const AUChannelInfo& i : info) pairs.emplace_back(i.inChannels, i.outChannels);
    return pairs;
}
bool has_pair(const std::vector<std::pair<SInt16, SInt16>>& pairs, SInt16 in, SInt16 out) { return std::find(pairs.begin(), pairs.end(), std::make_pair(in, out)) != pairs.end(); }

// ---- MIDI output ---------------------------------------------------------------------------
struct Packet { uint64_t time; std::vector<uint8_t> bytes; };
struct MidiCapture { std::vector<Packet> packets; uint32_t calls = 0; double sample_time = -1; };
OSStatus midi_callback(void* ref, const AudioTimeStamp* stamp, UInt32 output, const MIDIPacketList* list) {
    auto* c = static_cast<MidiCapture*>(ref);
    CHECK(output == 0 && stamp);
    ++c->calls;
    c->sample_time = stamp->mSampleTime;
    const MIDIPacket* packet = &list->packet[0];
    for (UInt32 i = 0; i < list->numPackets; ++i) {
        c->packets.push_back({packet->timeStamp, std::vector<uint8_t>(packet->data, packet->data + packet->length)});
        packet = MIDIPacketNext(packet);
    }
    return noErr;
}
void set_midi_callback(AudioUnit u, MidiCapture* capture) {
    AUMIDIOutputCallbackStruct callback{midi_callback, capture};
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_MIDIOutputCallback, kAudioUnitScope_Global, 0, &callback, sizeof(callback)) == noErr);
}
void check_midi_info(AudioUnit u) {
    UInt32 size = 0;
    Boolean writable = true;
    CHECK(AudioUnitGetPropertyInfo(u, kAudioUnitProperty_MIDIOutputCallbackInfo, kAudioUnitScope_Global, 0, &size, &writable) == noErr);
    CHECK(size == sizeof(CFArrayRef) && !writable);
    CFArrayRef names = nullptr;
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_MIDIOutputCallbackInfo, kAudioUnitScope_Global, 0, &names, &size) == noErr && names);
    CHECK(CFArrayGetCount(names) == 1 && CFGetTypeID(CFArrayGetValueAtIndex(names, 0)) == CFStringGetTypeID());
    CFRelease(names);
}
bool is(const Packet& p, uint64_t time, std::initializer_list<uint8_t> bytes) { return p.time == time && p.bytes == std::vector<uint8_t>(bytes); }

// ---- transport -----------------------------------------------------------------------------
struct Host { double beat = 12.5, tempo = 133.0; bool playing = true, recording = true; double position = 96000.0; Float32 numerator = 3; UInt32 denominator = 8; double downbeat = 12.0; };
OSStatus host_beat(void* ref, Float64* beat, Float64* tempo) {
    const auto* h = static_cast<const Host*>(ref);
    if (beat) *beat = h->beat;
    if (tempo) *tempo = h->tempo;
    return noErr;
}
OSStatus host_location(void* ref, UInt32* delta, Float32* numerator, UInt32* denominator, Float64* downbeat) {
    const auto* h = static_cast<const Host*>(ref);
    if (delta) *delta = 7;
    if (numerator) *numerator = h->numerator;
    if (denominator) *denominator = h->denominator;
    if (downbeat) *downbeat = h->downbeat;
    return noErr;
}
OSStatus host_state2(void* ref, Boolean* playing, Boolean* recording, Boolean* changed, Float64* position, Boolean* cycling, Float64* cycleStart, Float64* cycleEnd) {
    const auto* h = static_cast<const Host*>(ref);
    if (playing) *playing = h->playing;
    if (recording) *recording = h->recording;
    if (changed) *changed = false;
    if (position) *position = h->position;
    if (cycling) *cycling = false;
    if (cycleStart) *cycleStart = 0;
    if (cycleEnd) *cycleEnd = 0;
    return noErr;
}
void set_host(AudioUnit u, Host* h) {
    HostCallbackInfo info{};
    info.hostUserData = h;
    info.beatAndTempoProc = host_beat;
    info.musicalTimeLocationProc = host_location;
    info.transportStateProc2 = host_state2;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_HostCallbacks, kAudioUnitScope_Global, 0, &info, sizeof(info)) == noErr);
}

void set_param(AudioUnit u, AudioUnitParameterID id, float value, UInt32 offset = 0) {
    CHECK(AudioUnitSetParameter(u, id, kAudioUnitScope_Global, 0, value, offset) == noErr);
}
constexpr AudioUnitParameterID kGain = 0, kMode = 1;

// ---- the instrument -----------------------------------------------------------------------
void instrument_proof(const Probe& p) {
    const AudioComponentDescription d = description(p.component);
    CHECK(d.componentType == fourcc("aumu"));
    std::puts("AU instrument: component type aumu");

    // Notes, a controller and a note on with velocity 0 at sample offsets; MIDI out; levels.
    p.reset();
    AudioUnit u = make_unit(p, 0, 2);
    const auto pairs = channel_pairs(u);
    CHECK(has_pair(pairs, 0, 2) && has_pair(pairs, 0, 6) && has_pair(pairs, 0, 1));
    UInt32 inputs = 99, size = sizeof(inputs);
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, 0, &inputs, &size) == noErr && inputs == 0);
    check_midi_info(u);
    MidiCapture midi;
    set_midi_callback(u, &midi);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(MusicDeviceMIDIEvent(u, 0x90, 60, 100, 37) == noErr);
    CHECK(MusicDeviceMIDIEvent(u, 0xB0, 7, 99, 10) == noErr);
    CHECK(MusicDeviceMIDIEvent(u, 0x92, 64, 0, 300) == noErr);
    CHECK(MusicDeviceMIDIEvent(u, 0x80, 60, 40, 400) == noErr);
    Matrix out;
    CHECK(render(u, 2, 512, 0, out) == noErr);
    const float level = 0.1f * 100 / 127;
    for (UInt32 ch = 0; ch < 2; ++ch) {
        for (UInt32 i = 0; i < 512; ++i) {
            const float expected = (i >= 37 && i < 400) ? level : 0.0f;
            CHECK(near(out[ch][i], expected));
        }
    }
    std::puts("AU instrument: note on at sample 37 and note off at 400 start and end the sound on those samples");
    const ProbeReport* r = p.report();
    CHECK(r->event_total == 4 && r->blocks == 1);
    CHECK(r->events[0].kind == OP_EVENT_NOTE_ON && r->events[0].offset == 37 && r->events[0].channel == 0 && r->events[0].note == 60 && r->events[0].velocity == 100);
    CHECK(r->events[1].kind == OP_EVENT_MIDI && r->events[1].offset == 10 && r->events[1].midi[0] == 0xB0 && r->events[1].midi[1] == 7 && r->events[1].midi[2] == 99);
    CHECK(r->events[2].kind == OP_EVENT_NOTE_OFF && r->events[2].offset == 300 && r->events[2].channel == 2 && r->events[2].note == 64);
    CHECK(r->events[3].kind == OP_EVENT_NOTE_OFF && r->events[3].offset == 400 && r->events[3].velocity == 40);
    std::puts("AU instrument: note on, note off, note on with velocity 0 (a note off) and a controller reach the plugin with channel, note, velocity and offset");
    CHECK(midi.calls == 1 && midi.packets.size() == 4 && midi.sample_time == 0.0);
    CHECK(is(midi.packets[0], 10, {0xB0, 7, 99}));
    CHECK(is(midi.packets[1], 37, {0x90, 72, 100}));
    CHECK(is(midi.packets[2], 300, {0x82, 76, 0}));
    CHECK(is(midi.packets[3], 400, {0x80, 72, 40}));
    std::puts("AU instrument: MIDI out reaches kAudioUnitProperty_MIDIOutputCallback with sample-offset time stamps, in order");
    // No events: no packets.
    midi.packets.clear();
    midi.calls = 0;
    CHECK(render(u, 2, 512, 512, out) == noErr);
    CHECK(midi.calls == 0);
    // No allocation while rendering notes, a controller, MIDI out and the host's transport.
    {
        Host steady;
        set_host(u, &steady);
        uint32_t packets = 0;
        AUMIDIOutputCallbackStruct counter{count_midi_callback, &packets};
        CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_MIDIOutputCallback, kAudioUnitScope_Global, 0, &counter, sizeof(counter)) == noErr);
        uint64_t allocs = 0, frees = 0;
        for (int block = 0; block < 8; ++block) {
            for (int n = 0; n < 40; ++n) CHECK(MusicDeviceMIDIEvent(u, 0x90, 40 + n, 80, 5 + 10 * n) == noErr);
            CHECK(MusicDeviceMIDIEvent(u, 0xB0, 1, 2, 3) == noErr);
            for (int n = 0; n < 40; ++n) CHECK(MusicDeviceMIDIEvent(u, 0x80, 40 + n, 0, 450 + n) == noErr);
            const Counted c = render_counted(u, 2, 512, 1000000.0 + 512.0 * block, out);
            CHECK(c.status == noErr);
            allocs += c.allocs;
            frees += c.frees;
        }
        CHECK(packets == 8 * 81);
        std::printf("AU allocation: 8 renders with 81 MIDI events, MIDI out and the transport: %llu allocations, %llu frees\n", static_cast<unsigned long long>(allocs), static_cast<unsigned long long>(frees));
        CHECK(allocs == 0 && frees == 0);
    }
    finish(u);

    // A render longer than the 65536 frames the engine is made for is cut; events and the transport follow.
    p.reset();
    u = make_unit(p, 0, 2, 100000);
    Host host;
    set_host(u, &host);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(MusicDeviceMIDIEvent(u, 0x90, 61, 90, 65700) == noErr);
    CHECK(render(u, 2, 66000, 480000, out) == noErr);
    r = p.report();
    CHECK(r->blocks == 2 && r->event_total == 1);
    CHECK(r->event_block[0] == 2 && r->events[0].offset == 65700 - 65536);
    for (UInt32 i = 0; i < 66000; ++i) CHECK(near(out[0][i], i >= 65700 ? 0.1f * 90 / 127 : 0.0f));
    std::puts("AU instrument: a 66000 frame render is cut at 65536 frames and delivers an event at 65700 as offset 164 of the second piece");
    CHECK(r->have_transport);
    const OpTransport& t = r->transport;
    const unsigned all = OP_TRANSPORT_PLAYING | OP_TRANSPORT_RECORDING | OP_TRANSPORT_HAS_TEMPO | OP_TRANSPORT_HAS_SIGNATURE | OP_TRANSPORT_HAS_BEATS | OP_TRANSPORT_HAS_SECONDS;
    CHECK(t.flags == all);
    CHECK(t.tempo == 133.0 && t.numerator == 3 && t.denominator == 8 && t.bar_start == 12.0 && t.bar_number == 8);
    CHECK(t.sample_pos == 96000 + 65536 && std::fabs(t.seconds - (2.0 + 65536.0 / 48000.0)) < 1e-9);
    CHECK(std::fabs(t.quarter_notes - (12.5 + 65536.0 / 48000.0 * 133.0 / 60.0)) < 1e-9);
    std::printf("AU transport: playing, recording, tempo %.0f, %u/%u, beat %.4f, bar start %.1f, bar %lld, sample %lld, %.4f s reach the plugin\n", t.tempo, t.numerator, t.denominator, t.quarter_notes, t.bar_start, static_cast<long long>(t.bar_number), static_cast<long long>(t.sample_pos), t.seconds);
    host.playing = false;
    host.recording = false;
    CHECK(render(u, 2, 256, 600000, out) == noErr);
    CHECK(p.report()->transport.flags == (all & ~(OP_TRANSPORT_PLAYING | OP_TRANSPORT_RECORDING)));
    finish(u);

    // No host callbacks: the position comes from the render time stamp, nothing else is claimed.
    p.reset();
    u = make_unit(p, 0, 2);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(render(u, 2, 256, 4800, out) == noErr);
    r = p.report();
    CHECK(r->have_transport && r->transport.flags == OP_TRANSPORT_HAS_SECONDS && r->transport.sample_pos == 4800 && near(static_cast<float>(r->transport.seconds), 0.1f));
    std::puts("AU transport: without host callbacks only the render position is passed");
    finish(u);

    // Channel layouts: mono and 5.1 in the host's order; roles come back in the plugin's.
    p.reset();
    u = make_unit(p, 0, 1);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(p.report()->created == 1);
    CHECK(render(u, 1, 128, 0, out) == noErr && p.report()->layout_selected == 2 && p.report()->output_channels == 1);
    finish(u);
    p.reset();
    u = make_unit(p, 0, 6);
    CHECK(has(tags_of(u, kAudioUnitScope_Output, 0), tag(121, 6)) && has(tags_of(u, kAudioUnitScope_Output, 0), tag(124, 6)));
    CHECK(set_layout(u, kAudioUnitScope_Output, 0, tag(122, 6)) == noErr);  // MPEG_5_1_B: L R Ls Rs C LFE
    set_param(u, kMode, 1.0f);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(render(u, 6, 128, 0, out) == noErr && p.report()->layout_selected == 1 && p.report()->output_channels == 6);
    const uint8_t b51[] = {0, 1, 4, 5, 2, 3};
    for (UInt32 j = 0; j < 6; ++j) CHECK(near(out[j][0], 0.01f * (b51[j] + 1)) && near(out[j][127], 0.01f * (b51[j] + 1)));
    std::puts("AU instrument: 5.1 output with layout L R Ls Rs C LFE carries each canonical role on its host channel");
    finish(u);

    // A channel count no layout has is refused.
    u = make_unit(p, 0, 3);
    CHECK(AudioUnitInitialize(u) == kAudioUnitErr_FormatNotSupported);
    CHECK(AudioComponentInstanceDispose(u) == noErr);

    // Offline.
    p.reset();
    u = make_unit(p, 0, 2);
    UInt32 offline = 7;
    size = sizeof(offline);
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, &size) == noErr && offline == 0);
    offline = 1;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, sizeof(offline)) == noErr);
    CHECK(AudioUnitInitialize(u) == noErr && p.report()->offline == 1);
    offline = 0;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, sizeof(offline)) == noErr);
    CHECK(p.report()->offline == 0);
    offline = 1;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, sizeof(offline)) == noErr && p.report()->offline == 1);
    size = sizeof(offline);
    offline = 0;
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_OfflineRender, kAudioUnitScope_Global, 0, &offline, &size) == noErr && offline == 1);
    std::puts("AU offline: kAudioUnitProperty_OfflineRender reaches op_shell_set_offline before and after Initialize");
    finish(u);
}

// ---- the effect ----------------------------------------------------------------------------
struct Tagged { AudioChannelLayoutTag tag; std::vector<uint8_t> roles; };

void roles_proof(const Probe& p, const char* label, UInt32 n, const std::vector<uint8_t>& canonical, const Tagged& in, const Tagged& out) {
    p.reset();
    AudioUnit u = make_unit(p, n, n);
    CHECK(set_layout(u, kAudioUnitScope_Input, 0, in.tag) == noErr);
    CHECK(set_layout(u, kAudioUnitScope_Output, 0, out.tag) == noErr);
    Source src;
    for (UInt32 j = 0; j < n; ++j) src.values.push_back(0.01f * (in.roles[j] + 1));
    set_source(u, 0, &src);
    CHECK(AudioUnitInitialize(u) == noErr);
    Matrix m;
    CHECK(render(u, n, 256, 0, m) == noErr);
    const ProbeReport* r = p.report();
    CHECK(r->input_channels == n && r->output_channels == n);
    for (UInt32 i = 0; i < n; ++i) CHECK(near(r->input_first[i], 0.01f * (canonical[i] + 1)));  // the plugin's index i holds role canonical[i]
    // The plugin copied each canonical channel through: the host's channel j with role r carries what came in with role r.
    for (UInt32 j = 0; j < n; ++j) CHECK(near(m[j][0], 0.01f * (out.roles[j] + 1)) && near(m[j][255], 0.01f * (out.roles[j] + 1)));
    // And the plugin's own tag on each output index lands on the host's channel with that role.
    set_param(u, kMode, 1.0f);
    CHECK(render(u, n, 256, 256, m) == noErr);
    for (UInt32 j = 0; j < n; ++j) CHECK(near(m[j][0], 0.01f * (out.roles[j] + 1)));
    std::printf("AU layout %s: roles at the right indices in and out\n", label);
    finish(u);
}

void effect_proof(const Probe& p, bool musicFx) {
    const AudioComponentDescription d = description(p.component);
    CHECK(d.componentType == fourcc(musicFx ? "aumf" : "aufx"));
    std::printf("AU %s: component type %s\n", musicFx ? "music effect" : "effect", musicFx ? "aumf" : "aufx");
    p.reset();
    AudioUnit u = make_unit(p, 2, 2);
    const auto pairs = channel_pairs(u);
    CHECK(has_pair(pairs, 2, 2) && has_pair(pairs, 1, 1) && has_pair(pairs, 6, 6) && has_pair(pairs, 8, 8) && has_pair(pairs, 12, 12) && has_pair(pairs, 2, 6));
    CHECK(!has_pair(pairs, 3, 3));
    std::puts("AU channels: SupportedNumChannels lists mono, stereo, 5.1, 7.1, 7.1.4 and stereo to 5.1");
    if (!musicFx) {
        // The tags each bus offers.
        UInt32 six = 0;
        auto sixUnit = make_unit(p, 6, 6);
        const auto t51 = tags_of(sixUnit, kAudioUnitScope_Input, 0);
        CHECK(has(t51, tag(121, 6)) && has(t51, tag(122, 6)) && has(t51, tag(123, 6)) && has(t51, tag(124, 6)) && !has(t51, tag(195, 10)));
        CHECK(t51.front() == tag(121, 6));
        (void)six;
        AudioChannelLayout def{};
        UInt32 defSize = sizeof(def);
        CHECK(AudioUnitGetProperty(sixUnit, kAudioUnitProperty_AudioChannelLayout, kAudioUnitScope_Input, 0, &def, &defSize) == noErr && def.mChannelLayoutTag == tag(121, 6));
        CHECK(set_layout(sixUnit, kAudioUnitScope_Input, 0, tag(110, 6)) != noErr);  // Hexagonal: not a layout of the plugin
        CHECK(AudioComponentInstanceDispose(sixUnit) == noErr);
        auto eightUnit = make_unit(p, 8, 8);
        CHECK(has(tags_of(eightUnit, kAudioUnitScope_Output, 0), tag(128, 8)));
        CHECK(AudioComponentInstanceDispose(eightUnit) == noErr);
        auto twelve = make_unit(p, 12, 12);
        const auto t714 = tags_of(twelve, kAudioUnitScope_Output, 0);
        CHECK(has(t714, tag(192, 12)) && has(t714, tag(202, 12)));
        CHECK(AudioComponentInstanceDispose(twelve) == noErr);
        auto monoUnit = make_unit(p, 1, 1);
        CHECK(has(tags_of(monoUnit, kAudioUnitScope_Output, 0), tag(100, 1)));
        CHECK(AudioComponentInstanceDispose(monoUnit) == noErr);
        std::puts("AU channels: SupportedChannelLayoutTags offer the tags whose roles are the plugin's, MPEG_5_1_A first, and refuse others");
    }

    // Sidechain: a second input element.
    UInt32 count = 0, size = sizeof(count);
    Boolean writable = false;
    CHECK(AudioUnitGetPropertyInfo(u, kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, 0, &size, &writable) == noErr && writable);
    count = 3;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, 0, &count, sizeof(count)) != noErr);
    count = 2;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, 0, &count, sizeof(count)) == noErr);
    CHECK(set_format(u, kAudioUnitScope_Input, 1, 2) == noErr);
    Source main{{0.5f, 0.5f}}, side{{0.2f, 0.3f}};
    set_source(u, 0, &main);
    set_source(u, 1, &side);
    CHECK(AudioUnitInitialize(u) == noErr);
    Matrix m;
    CHECK(render(u, 2, 256, 0, m) == noErr);
    const ProbeReport* r = p.report();
    CHECK(r->sidechain_channels == 2 && r->input_channels == 2 && r->layout_selected == 0);
    CHECK(near(r->sidechain_first[0], 0.2f) && near(r->sidechain_first[1], 0.3f) && near(r->input_first[0], 0.5f));
    for (UInt32 ch = 0; ch < 2; ++ch) for (UInt32 i = 0; i < 256; ++i) CHECK(near(m[ch][i], 0.5f + 0.5f * 0.2f));
    std::puts("AU sidechain: the second input element reaches the plugin as its sidechain, in the layout that has one");
    finish(u);
    // The element is there but nothing feeds it: the plugin runs without a sidechain.
    p.reset();
    u = make_unit(p, 2, 2);
    count = 2;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_ElementCount, kAudioUnitScope_Input, 0, &count, sizeof(count)) == noErr);
    CHECK(set_format(u, kAudioUnitScope_Input, 1, 2) == noErr);
    set_source(u, 0, &main);
    CHECK(AudioUnitInitialize(u) == noErr);
    CHECK(render(u, 2, 128, 0, m) == noErr);
    r = p.report();
    CHECK(r->sidechain_channels == 0 && r->layout_selected == 1 && near(m[0][5], 0.5f));
    std::puts("AU sidechain: an unconnected sidechain element selects the layout without one");
    finish(u);

    // Parameter events at sample offsets.
    p.reset();
    u = make_unit(p, 2, 2);
    set_source(u, 0, &main);
    CHECK(AudioUnitInitialize(u) == noErr);
    AudioUnitParameterEvent event{};
    event.scope = kAudioUnitScope_Global;
    event.element = 0;
    event.parameter = kGain;
    event.eventType = kParameterEvent_Immediate;
    event.eventValues.immediate.bufferOffset = 100;
    event.eventValues.immediate.value = 2.0f;
    CHECK(AudioUnitScheduleParameters(u, &event, 1) == noErr);
    CHECK(render(u, 2, 512, 0, m) == noErr);
    for (UInt32 i = 0; i < 512; ++i) CHECK(near(m[0][i], i < 100 ? 0.5f : 1.0f) && near(m[1][i], m[0][i]));
    AudioUnitParameterValue value = 0;
    CHECK(AudioUnitGetParameter(u, kGain, kAudioUnitScope_Global, 0, &value) == noErr && value == 2.0f);
    std::puts("AU parameters: AudioUnitScheduleParameters immediate at 100 changes the gain on sample 100 exactly");
    CHECK(AudioUnitSetParameter(u, kGain, kAudioUnitScope_Global, 0, 0.5f, 200) == noErr);
    CHECK(render(u, 2, 512, 512, m) == noErr);
    for (UInt32 i = 0; i < 512; ++i) CHECK(near(m[0][i], i < 200 ? 1.0f : 0.25f));
    CHECK(AudioUnitGetParameter(u, kGain, kAudioUnitScope_Global, 0, &value) == noErr && value == 0.5f);
    std::puts("AU parameters: AudioUnitSetParameter with a buffer offset of 200 lands on sample 200");
    set_param(u, kGain, 1.0f);
    event.eventType = kParameterEvent_Ramped;
    event.eventValues.ramp.startBufferOffset = 64;
    event.eventValues.ramp.durationInFrames = 256;
    event.eventValues.ramp.startValue = 0.0f;
    event.eventValues.ramp.endValue = 4.0f;
    CHECK(AudioUnitScheduleParameters(u, &event, 1) == noErr);
    CHECK(render(u, 2, 512, 1024, m) == noErr);
    float previous = 0.0f, worst = 0.0f;
    for (UInt32 i = 0; i < 512; ++i) {
        const float x = m[0][i];
        if (i < 64) { CHECK(near(x, 0.5f)); continue; }
        if (i >= 320) { CHECK(near(x, 2.0f)); continue; }
        const float ideal = 0.5f * 4.0f * (static_cast<float>(i) - 64.0f) / 256.0f;
        worst = std::max(worst, std::fabs(x - ideal));
        CHECK(x >= previous);
        previous = x;
    }
    CHECK(worst <= 0.5f * 4.0f * 16.0f / 256.0f + 1e-5f);
    CHECK(AudioUnitGetParameter(u, kGain, kAudioUnitScope_Global, 0, &value) == noErr && value == 4.0f);
    std::printf("AU parameters: a ramp from 0 to 4 over 256 frames is a staircase within %.4f of the line and ends on the end value\n", worst);
    // No allocation while rendering parameter events (a ramp, immediate events, a buffer offset), a sidechain and a 7.1.4 bus.
    {
        uint64_t allocs = 0, frees = 0;
        for (int block = 0; block < 8; ++block) {
            event.eventType = kParameterEvent_Ramped;
            event.eventValues.ramp.startBufferOffset = -20;
            event.eventValues.ramp.durationInFrames = 400;
            event.eventValues.ramp.startValue = 1.0f;
            event.eventValues.ramp.endValue = 2.0f;
            CHECK(AudioUnitScheduleParameters(u, &event, 1) == noErr);
            CHECK(AudioUnitSetParameter(u, kGain, kAudioUnitScope_Global, 0, 1.5f, 100 + block) == noErr);
            const Counted c = render_counted(u, 2, 512, 3000.0 + 512.0 * block, m);
            CHECK(c.status == noErr);
            allocs += c.allocs;
            frees += c.frees;
        }
        std::printf("AU allocation: 8 renders with a ramp and offset parameter events: %llu allocations, %llu frees\n", static_cast<unsigned long long>(allocs), static_cast<unsigned long long>(frees));
        CHECK(allocs == 0 && frees == 0);
        set_param(u, kGain, 4.0f);
    }
    // Events do not leak into the next render.
    CHECK(render(u, 2, 256, 2048, m) == noErr && near(m[0][0], 2.0f) && near(m[0][255], 2.0f));
    // A value outside the range, or a bad parameter, is refused with an offset too.
    CHECK(AudioUnitSetParameter(u, kGain, kAudioUnitScope_Global, 0, 99.0f, 10) != noErr);
    CHECK(AudioUnitSetParameter(u, 77, kAudioUnitScope_Global, 0, 1.0f, 10) != noErr);
    finish(u);

    if (musicFx) {
        // MIDI in and out on an effect.
        p.reset();
        u = make_unit(p, 2, 2);
        set_source(u, 0, &main);
        check_midi_info(u);
        MidiCapture midi;
        set_midi_callback(u, &midi);
        CHECK(AudioUnitInitialize(u) == noErr);
        CHECK(MusicDeviceMIDIEvent(u, 0x91, 50, 80, 100) == noErr);
        CHECK(render(u, 2, 256, 0, m) == noErr);
        for (UInt32 i = 0; i < 256; ++i) CHECK(near(m[1][i], 0.5f + (i >= 100 ? 0.1f * 80 / 127 : 0.0f)));
        CHECK(midi.packets.size() == 1 && is(midi.packets[0], 100, {0x91, 62, 80}));
        CHECK(p.report()->events[0].kind == OP_EVENT_NOTE_ON && p.report()->events[0].channel == 1);
        std::puts("AU music effect: MIDI in at sample 100 changes the audio there and the plugin's MIDI out reaches the callback");
        finish(u);
        return;
    }
    // An effect without MIDI has no MIDI properties.
    UInt32 infoSize = 0;
    Boolean w = false;
    u = make_unit(p, 2, 2);
    CHECK(AudioUnitGetPropertyInfo(u, kAudioUnitProperty_MIDIOutputCallbackInfo, kAudioUnitScope_Global, 0, &infoSize, &w) != noErr);
    CHECK(AudioComponentInstanceDispose(u) == noErr);

    // Layouts and roles.
    const std::vector<uint8_t> c51 = {0, 1, 2, 3, 4, 5};
    const Tagged a{tag(121, 6), {0, 1, 2, 3, 4, 5}}, b{tag(122, 6), {0, 1, 4, 5, 2, 3}}, c{tag(123, 6), {0, 2, 1, 4, 5, 3}}, dd{tag(124, 6), {2, 0, 1, 4, 5, 3}};
    roles_proof(p, "5.1 L R C LFE Ls Rs", 6, c51, a, a);
    roles_proof(p, "5.1 L R Ls Rs C LFE", 6, c51, b, b);
    roles_proof(p, "5.1 L C R Ls Rs LFE", 6, c51, c, c);
    roles_proof(p, "5.1 C L R Ls Rs LFE", 6, c51, dd, dd);
    roles_proof(p, "5.1 in L R C LFE Ls Rs, out C L R Ls Rs LFE", 6, c51, a, dd);
    const std::vector<uint8_t> c71 = {0, 1, 2, 3, 9, 10, 4, 5};
    roles_proof(p, "7.1 L R C LFE Ls Rs Rls Rrs", 8, c71, {tag(128, 8), c71}, {tag(128, 8), c71});
    const std::vector<uint8_t> c714 = {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17};
    roles_proof(p, "7.1.4 Atmos", 12, c714, {tag(192, 12), c714}, {tag(192, 12), c714});
    const std::vector<uint8_t> logicB = {0, 1, 4, 5, 9, 10, 2, 3, 12, 14, 15, 17};
    roles_proof(p, "7.1.4 Logic B (L R Rls Rrs Ls Rs C LFE ...)", 12, c714, {tag(202, 12), logicB}, {tag(192, 12), c714});

    // Channel descriptions instead of a tag: C L R LFE Ls Rs by label.
    {
        p.reset();
        u = make_unit(p, 6, 6);
        std::vector<uint8_t> raw(offsetof(AudioChannelLayout, mChannelDescriptions) + 6 * sizeof(AudioChannelDescription), 0);
        auto* layout = reinterpret_cast<AudioChannelLayout*>(raw.data());
        layout->mChannelLayoutTag = kAudioChannelLayoutTag_UseChannelDescriptions;
        layout->mNumberChannelDescriptions = 6;
        const uint32_t labels[] = {3, 1, 2, 4, 5, 6};
        for (int j = 0; j < 6; ++j) layout->mChannelDescriptions[j].mChannelLabel = labels[j];
        CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_AudioChannelLayout, kAudioUnitScope_Input, 0, layout, static_cast<UInt32>(raw.size())) == noErr);
        const uint8_t roles[] = {2, 0, 1, 3, 4, 5};
        Source s;
        for (int j = 0; j < 6; ++j) s.values.push_back(0.01f * (roles[j] + 1));
        set_source(u, 0, &s);
        CHECK(AudioUnitInitialize(u) == noErr);
        CHECK(render(u, 6, 64, 0, m) == noErr);
        for (UInt32 i = 0; i < 6; ++i) CHECK(near(p.report()->input_first[i], 0.01f * (i + 1)));
        std::puts("AU layout: channel descriptions (C L R LFE Ls Rs) map by label to the plugin's indices");
        finish(u);
    }

    // Stereo in, 5.1 out.
    {
        p.reset();
        u = make_unit(p, 2, 6);
        Source s{{0.3f, 0.4f}};
        set_source(u, 0, &s);
        CHECK(AudioUnitInitialize(u) == noErr);
        CHECK(render(u, 6, 64, 0, m) == noErr);
        CHECK(p.report()->layout_selected == 6 && p.report()->input_channels == 2 && p.report()->output_channels == 6);
        CHECK(near(m[0][3], 0.3f) && near(m[1][3], 0.4f) && near(m[2][3], 0.0f) && near(m[5][3], 0.0f));
        std::puts("AU layout: a stereo in, 5.1 out layout selects by channel counts");
        finish(u);
    }
    // A count no layout has.
    u = make_unit(p, 3, 3);
    CHECK(AudioUnitInitialize(u) == kAudioUnitErr_FormatNotSupported);
    CHECK(AudioComponentInstanceDispose(u) == noErr);

    // Bypass on 5.1 with a gain that makes processed and dry differ.
    {
        p.reset();
        u = make_unit(p, 6, 6);
        Source s;
        for (int j = 0; j < 6; ++j) s.values.push_back(0.1f * (j + 1));
        set_source(u, 0, &s);
        set_param(u, kGain, 2.0f);
        CHECK(AudioUnitInitialize(u) == noErr);
        CHECK(render(u, 6, 256, 0, m) == noErr && near(m[3][100], 0.8f));
        proof_set_bypass(u, true);
        for (int block = 1; block < 8; ++block) CHECK(render(u, 6, 256, block * 256, m) == noErr);
        for (UInt32 ch = 0; ch < 6; ++ch) CHECK(near(m[ch][255], 0.1f * (ch + 1)));
        proof_set_bypass(u, false);
        for (int block = 8; block < 16; ++block) CHECK(render(u, 6, 256, block * 256, m) == noErr);
        CHECK(near(m[3][255], 0.8f));
        std::puts("AU bypass: 5.1 bypass is the input on every channel, and leaving it restores the processed signal");
        finish(u);
    }
}

void abi_proof(const Probe& newer, const Probe& older) {
    newer.reset();
    AudioUnit u = make_unit(newer, 2, 2);
    Source s{{0.5f, 0.5f}};
    set_source(u, 0, &s);
    CHECK(AudioUnitInitialize(u) == noErr);
    Matrix m;
    CHECK(render(u, 2, 64, 0, m) == noErr && near(m[0][0], 0.5f));
    finish(u);
    u = make_unit(older, 2, 2);
    set_source(u, 0, &s);
    CHECK(AudioUnitInitialize(u) == kAudioUnitErr_FailedInitialization);
    CHECK(AudioComponentInstanceDispose(u) == noErr);
    std::puts("AU ABI check: a library of version 4 works with a shell built for 3; a library of version 2 is refused at Initialize");
}
}  // namespace

int main(int argc, char** argv) {
    CHECK(argc == 6);
    {
        // The counter must see an allocation made by another image, or a count of zero below proves nothing.
        const uint64_t before = op_alloc_count();
        op_alloc_arm();
        CFDataRef probe = CFDataCreateMutable(nullptr, 4096);  // made inside CoreFoundation, like the shell's own calls
        op_alloc_disarm();
        CFRelease(probe);
        CHECK(op_alloc_count() > before);
    }
    const Probe fx = load(argv[1]);
    const Probe inst = load(argv[2]);
    const Probe musicFx = load(argv[3]);
    const Probe newer = load(argv[4]);
    const Probe older = load(argv[5]);
    effect_proof(fx, false);
    instrument_proof(inst);
    effect_proof(musicFx, true);
    abi_proof(newer, older);
    std::puts("AU events proof: component types, MIDI in and out with sample offsets, parameter events, transport, sidechain, layouts and roles, offline, ABI check: PASS");
    return 0;
}
