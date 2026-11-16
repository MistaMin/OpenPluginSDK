// SPDX-License-Identifier: MIT
// Plugin state through kAudioUnitProperty_ClassInfo. The SDK example has no chunk
// of its own, so this loads the test bundle built from state_shell.cpp (two
// parameters and a plugin-owned chunk) in this process only, and checks:
//   - a save carries the base keys and the shell's "OpenPluginState" key;
//   - a round trip restores both parameters and the chunk, on an initialized
//     unit and on one that is not initialized yet, and the chunk survives
//     Uninitialize and Initialize;
//   - a dictionary without the key (an old preset) restores its parameters and
//     leaves the chunk alone;
//   - a corrupt blob, a blob the plugin refuses and a key of the wrong type
//     refuse the whole dictionary and change nothing, the base keys' parameters
//     included;
//   - a chunk that grows between the size query and the copy is still saved
//     whole, and a large chunk round-trips;
//   - saving and restoring beside a rendering thread loses no block.
//
//   ctest --test-dir build-shell -R au_state_proof --output-on-failure
#include "proof_au.h"
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <thread>

namespace {
using Bytes = std::vector<uint8_t>;
struct Hooks {
    void* (*last)();
    void (*set)(void*, const uint8_t*, uint32_t);
    uint32_t (*get)(void*, uint8_t*, uint32_t);
    void (*grow)(void*, uint32_t);
    uint32_t (*creates)();
    uint32_t (*sets)();
} hooks;

struct Unit {
    AudioUnit unit = nullptr;
    void* engine = nullptr;  // valid while initialized
};

Unit make(AudioComponent component, UInt32 channels) {
    Unit u;
    u.unit = proof_make_unit(component, channels);
    return u;
}
void initialize(Unit& u) {
    CHECK(AudioUnitInitialize(u.unit) == noErr);
    u.engine = hooks.last();
    CHECK(u.engine);
}
void uninitialize(Unit& u) {
    CHECK(AudioUnitUninitialize(u.unit) == noErr);
    u.engine = nullptr;
}
void set_param(Unit& u, UInt32 id, float value) {
    CHECK(AudioUnitSetParameter(u.unit, id, kAudioUnitScope_Global, 0, value, 0) == noErr);
}
float param(Unit& u, UInt32 id) {
    AudioUnitParameterValue value = -99.0f;
    CHECK(AudioUnitGetParameter(u.unit, id, kAudioUnitScope_Global, 0, &value) == noErr);
    return value;
}
Bytes chunk(Unit& u) {
    CHECK(u.engine);
    Bytes out(hooks.get(u.engine, nullptr, 0));
    if (!out.empty()) CHECK(hooks.get(u.engine, out.data(), static_cast<uint32_t>(out.size())) == out.size());
    return out;
}
void set_chunk(Unit& u, const Bytes& bytes) {
    CHECK(u.engine);
    hooks.set(u.engine, bytes.data(), static_cast<uint32_t>(bytes.size()));
}
Bytes bytes_of(const char* text) { return Bytes(text, text + std::strlen(text)); }

CFDictionaryRef class_info(Unit& u) {
    CFPropertyListRef saved = nullptr;
    UInt32 size = sizeof(saved);
    CHECK(AudioUnitGetProperty(u.unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &saved, &size) == noErr && saved);
    CHECK(CFGetTypeID(saved) == CFDictionaryGetTypeID());
    return static_cast<CFDictionaryRef>(saved);
}
OSStatus set_class_info(Unit& u, CFPropertyListRef plist) {
    return AudioUnitSetProperty(u.unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &plist, sizeof(plist));
}
Bytes blob_of(CFDictionaryRef dict) {
    auto data = static_cast<CFDataRef>(CFDictionaryGetValue(dict, CFSTR("OpenPluginState")));
    CHECK(data && CFGetTypeID(data) == CFDataGetTypeID());
    return Bytes(CFDataGetBytePtr(data), CFDataGetBytePtr(data) + CFDataGetLength(data));
}
CFMutableDictionaryRef with_blob(CFDictionaryRef base, const Bytes& blob) {
    CFMutableDictionaryRef copy = CFDictionaryCreateMutableCopy(nullptr, 0, base);
    CFDataRef data = CFDataCreate(nullptr, blob.data(), static_cast<CFIndex>(blob.size()));
    CFDictionarySetValue(copy, CFSTR("OpenPluginState"), data);
    CFRelease(data);
    return copy;
}
void put_float(Bytes& blob, size_t offset, float value) { std::memcpy(blob.data() + offset, &value, 4); }
void put_u32(Bytes& blob, size_t offset, uint32_t value) { std::memcpy(blob.data() + offset, &value, 4); }

/// The unit renders `gain` times the tone.
void check_gain(Unit& u, UInt32 channels, float gain) {
    const auto out = proof_render_run(u.unit, channels, 0, 2);
    for (UInt32 ch = 0; ch < channels; ++ch) {
        for (size_t n = 0; n < out[ch].size(); ++n) CHECK(std::fabs(out[ch][n] - proof_tone(static_cast<long>(n)) * gain) < 1e-5);
    }
}

constexpr UInt32 kGain = 0;
constexpr UInt32 kTilt = 1;
const Bytes kChunkA = {'c', 'h', 'u', 'n', 'k', '-', 'A', 0, 1, 2, 3, 0x7F, 0x80, 0xFE};

void check_one(AudioComponent component, UInt32 channels, const char* label) {
    // Source: gain 2.5, tilt 0.25 and a chunk of its own.
    Unit source = make(component, channels);
    initialize(source);
    set_param(source, kGain, 2.5f);
    set_param(source, kTilt, 0.25f);
    set_chunk(source, kChunkA);
    CFDictionaryRef saved = class_info(source);
    for (CFStringRef key : {CFSTR(kAUPresetVersionKey), CFSTR(kAUPresetTypeKey), CFSTR(kAUPresetSubtypeKey), CFSTR(kAUPresetManufacturerKey), CFSTR(kAUPresetDataKey), CFSTR(kAUPresetNameKey)}) {
        CHECK(CFDictionaryContainsKey(saved, key));
    }
    const Bytes blob = blob_of(saved);
    CHECK(blob.size() > kChunkA.size());
    std::printf("AU state %s: class info has the base keys and OpenPluginState (%zu bytes)\n", label, blob.size());

    // Round trip into a fresh, initialized unit.
    Unit target = make(component, channels);
    initialize(target);
    CHECK(param(target, kGain) == 1.0f && param(target, kTilt) == 0.5f && chunk(target).empty());
    CHECK(set_class_info(target, saved) == noErr);
    CHECK(param(target, kGain) == 2.5f && param(target, kTilt) == 0.25f);
    CHECK(chunk(target) == kChunkA);
    check_gain(target, channels, 2.5f);  // the render thread must not push the old values back
    CFDictionaryRef again = class_info(target);
    CHECK(blob_of(again) == blob);
    CFRelease(again);
    std::printf("AU state %s: round trip restores parameters, chunk and audio, and saves the same blob\n", label);

    // A unit that is not initialized yet takes the state and holds it for Initialize.
    Unit early = make(component, channels);
    CHECK(set_class_info(early, saved) == noErr);
    CHECK(param(early, kGain) == 2.5f && param(early, kTilt) == 0.25f);
    CFDictionaryRef early_saved = class_info(early);
    CHECK(blob_of(early_saved) == blob);
    CFRelease(early_saved);
    initialize(early);
    CHECK(chunk(early) == kChunkA);
    check_gain(early, channels, 2.5f);
    // Uninitialize drops the engine; the state comes back with the next one.
    set_chunk(early, bytes_of("changed while running"));
    uninitialize(early);
    CFDictionaryRef kept = class_info(early);
    CHECK(blob_of(kept).size() == 16 + bytes_of("changed while running").size());
    CFRelease(kept);
    initialize(early);
    CHECK(chunk(early) == bytes_of("changed while running"));
    CHECK(param(early, kGain) == 2.5f);
    std::printf("AU state %s: restored before Initialize, held, and kept over Uninitialize\n", label);

    // An old dictionary (no key): parameters restore, the chunk is left alone.
    CFMutableDictionaryRef old = CFDictionaryCreateMutableCopy(nullptr, 0, saved);
    CFDictionaryRemoveValue(old, CFSTR("OpenPluginState"));
    Unit legacy = make(component, channels);
    initialize(legacy);
    set_chunk(legacy, bytes_of("keep-me"));
    CHECK(set_class_info(legacy, old) == noErr);
    CHECK(param(legacy, kGain) == 2.5f && param(legacy, kTilt) == 0.25f);
    CHECK(chunk(legacy) == bytes_of("keep-me"));
    check_gain(legacy, channels, 2.5f);
    Unit legacy_early = make(component, channels);
    CHECK(set_class_info(legacy_early, old) == noErr);
    CHECK(param(legacy_early, kGain) == 2.5f);
    initialize(legacy_early);
    CHECK(chunk(legacy_early).empty());
    CFRelease(old);
    std::printf("AU state %s: a dictionary without the key restores its parameters and leaves the chunk\n", label);

    // Corrupt blobs refuse the whole dictionary. The base keys carry gain 2.5 / tilt 0.25;
    // the targets hold gain 1.0 / tilt 0.5, which must stay.
    Bytes truncated(blob.begin(), blob.end() - 1);
    Bytes cut_header(blob.begin(), blob.begin() + 15);
    Bytes extended = blob;
    extended.push_back(0);
    Bytes bad_magic = blob;
    bad_magic[0] ^= 0xFF;
    Bytes nan_gain = blob;
    put_float(nan_gain, 4, std::nanf(""));
    Bytes big_gain = blob;
    put_float(big_gain, 4, 5.0f);
    Bytes negative_tilt = blob;
    put_float(negative_tilt, 8, -1.0f);
    Bytes wrong_length = blob;
    put_u32(wrong_length, 12, static_cast<uint32_t>(kChunkA.size()) + 1);
    Bytes refused_chunk = blob;  // the plugin's own load_state says no
    refused_chunk.back() = 0xFF;
    const std::pair<const char*, Bytes> cases[] = {{"empty", {}}, {"truncated", truncated}, {"cut in the header", cut_header}, {"extended", extended},
        {"wrong magic", bad_magic}, {"NaN gain", nan_gain}, {"gain out of range", big_gain}, {"negative tilt", negative_tilt},
        {"wrong chunk length", wrong_length}, {"chunk the plugin refuses", refused_chunk}};
    Unit live = make(component, channels);
    initialize(live);
    set_chunk(live, bytes_of("E-chunk"));
    Unit idle = make(component, channels);
    int refused = 0;
    for (const auto& [name, bad] : cases) {
        CFMutableDictionaryRef dict = with_blob(saved, bad);
        CHECK(set_class_info(live, dict) != noErr);
        CHECK(param(live, kGain) == 1.0f && param(live, kTilt) == 0.5f);
        CHECK(chunk(live) == bytes_of("E-chunk"));
        CHECK(set_class_info(idle, dict) != noErr);
        CHECK(param(idle, kGain) == 1.0f && param(idle, kTilt) == 0.5f);
        CFRelease(dict);
        ++refused;
        (void)name;
    }
    CFMutableDictionaryRef wrong_type = CFDictionaryCreateMutableCopy(nullptr, 0, saved);
    CFDictionarySetValue(wrong_type, CFSTR("OpenPluginState"), CFSTR("not data"));
    CHECK(set_class_info(live, wrong_type) != noErr);
    CHECK(set_class_info(idle, wrong_type) != noErr);
    CFRelease(wrong_type);
    ++refused;
    check_gain(live, channels, 1.0f);
    CFDictionaryRef idle_saved = class_info(idle);
    CHECK(!CFDictionaryContainsKey(idle_saved, CFSTR("OpenPluginState")));  // nothing held from a refusal
    CFRelease(idle_saved);
    initialize(idle);
    CHECK(chunk(idle).empty() && param(idle, kGain) == 1.0f);
    // A good blob still loads after the refusals.
    CHECK(set_class_info(live, saved) == noErr);
    CHECK(chunk(live) == kChunkA && param(live, kGain) == 2.5f);
    std::printf("AU state %s: %d corrupt cases refused, initialized and not, nothing changed\n", label, refused);

    // The size query: a chunk that grows between the query and the copy, and a large one.
    Unit grow = make(component, channels);
    initialize(grow);
    set_chunk(grow, Bytes(100, 0x22));
    hooks.grow(grow.engine, 50);
    CFDictionaryRef grown = class_info(grow);
    CHECK(chunk(grow).size() == 150);
    const Bytes grown_blob = blob_of(grown);
    CHECK(grown_blob.size() == 16 + 150);
    CHECK(Bytes(grown_blob.begin() + 16, grown_blob.end()) == chunk(grow));
    CFRelease(grown);
    Bytes large(300000);
    for (size_t i = 0; i < large.size(); ++i) large[i] = static_cast<uint8_t>(i * 31 % 251);
    set_chunk(grow, large);
    CFDictionaryRef large_saved = class_info(grow);
    Unit large_target = make(component, channels);
    initialize(large_target);
    CHECK(set_class_info(large_target, large_saved) == noErr);
    CHECK(chunk(large_target) == large);
    CFRelease(large_saved);
    std::printf("AU state %s: a chunk that grows after the size query is saved whole; a 300000 byte chunk round-trips\n", label);

    // Save and restore beside a rendering thread.
    std::atomic<bool> stop{false};
    std::atomic<int> blocks{0};
    std::atomic<int> failures{0};
    std::thread renderer([&] {
        ProofBuffers buffers(channels, kProofBlock);
        double start = 0.0;
        while (!stop.load()) {
            if (proof_render(live.unit, buffers, kProofBlock, start) != noErr) failures.fetch_add(1);
            blocks.fetch_add(1);
            start += kProofBlock;
        }
    });
    // Let the renderer start first: on a slow machine (x86_64 under Rosetta in CI) the
    // 100 saves can otherwise finish before it renders a single block.
    while (blocks.load() == 0) std::this_thread::yield();
    for (int i = 0; i < 100; ++i) {
        CFDictionaryRef during = class_info(live);
        CHECK(set_class_info(live, during) == noErr);
        CFRelease(during);
    }
    const int before_stop = blocks.load();
    while (blocks.load() < before_stop + 2) std::this_thread::yield();
    stop.store(true);
    renderer.join();
    CHECK(failures.load() == 0);
    CHECK(blocks.load() > 0);
    CHECK(chunk(live) == kChunkA && param(live, kGain) == 2.5f);
    std::printf("AU state %s: 100 saves and restores beside %d rendered blocks, no render failure\n", label, blocks.load());

    // An old preset (no "OpenPluginState" key) from an untrusted file: the base class writes
    // its values into the unit unchecked. One that is not a number, and one out of range, are
    // put back to the defaults, and the unit still renders. The element saves each value as a
    // big-endian float: gain 2.5 is 40 20 00 00 and tilt 0.25 is 3E 80 00 00.
    {
        CFMutableDictionaryRef hostile = CFDictionaryCreateMutableCopy(nullptr, 0, saved);
        CFDictionaryRemoveValue(hostile, CFSTR("OpenPluginState"));
        auto data = static_cast<CFDataRef>(CFDictionaryGetValue(hostile, CFSTR(kAUPresetDataKey)));
        CHECK(data && CFGetTypeID(data) == CFDataGetTypeID());
        Bytes raw(CFDataGetBytePtr(data), CFDataGetBytePtr(data) + CFDataGetLength(data));
        const auto replace = [&raw](const Bytes& from, const Bytes& to) {
            const auto at = std::search(raw.begin(), raw.end(), from.begin(), from.end());
            CHECK(at != raw.end());
            std::copy(to.begin(), to.end(), at);
        };
        replace({0x40, 0x20, 0x00, 0x00}, {0x7F, 0xC0, 0x00, 0x00});  // gain: NaN
        replace({0x3E, 0x80, 0x00, 0x00}, {0x41, 0x10, 0x00, 0x00});  // tilt: 9.0, above its maximum of 1
        CFDataRef patched = CFDataCreate(nullptr, raw.data(), static_cast<CFIndex>(raw.size()));
        CFDictionarySetValue(hostile, CFSTR(kAUPresetDataKey), patched);
        CFRelease(patched);
        for (bool initialized : {true, false}) {
            Unit unit = make(component, channels);
            if (initialized) initialize(unit);
            set_param(unit, kGain, 3.0f);
            CHECK(set_class_info(unit, hostile) == noErr);
            CHECK(param(unit, kGain) == 1.0f && param(unit, kTilt) == 0.5f);
            if (!initialized) initialize(unit);
            check_gain(unit, channels, 1.0f);
            uninitialize(unit);
            CHECK(AudioComponentInstanceDispose(unit.unit) == noErr);
        }
        CFRelease(hostile);
        std::printf("AU state %s: an old preset with a NaN and an out-of-range value restores the defaults and renders\n", label);
    }

    // The render thread hands the engine a parameter value only when it moved.
    {
        Unit unit = make(component, channels);
        initialize(unit);
        proof_render_run(unit.unit, channels, 0, 2);
        const uint32_t before = hooks.sets();
        proof_render_run(unit.unit, channels, 2, 64);
        CHECK(hooks.sets() == before);
        set_param(unit, kGain, 2.0f);
        proof_render_run(unit.unit, channels, 66, 8);
        CHECK(hooks.sets() - before == 1);
        check_gain(unit, channels, 2.0f);
        // A state call writes the engine from the main thread; the next render pushes everything once.
        CFDictionaryRef now = class_info(unit);
        CFRelease(now);
        const uint32_t afterSave = hooks.sets();
        proof_render_run(unit.unit, channels, 0, 8);
        CHECK(hooks.sets() - afterSave == 2);
        uninitialize(unit);
        CHECK(AudioComponentInstanceDispose(unit.unit) == noErr);
        std::printf("AU state %s: 64 steady renders make no parameter call into the engine; a changed value makes one\n", label);
    }

    // A state restored before Initialize is loaded into a scratch engine, which Initialize
    // adopts when it was made for the same formats and slice size.
    {
        Unit unit = make(component, channels);
        const uint32_t before = hooks.creates();
        CHECK(set_class_info(unit, saved) == noErr);
        initialize(unit);
        CHECK(hooks.creates() - before == 1);
        CHECK(chunk(unit) == kChunkA && param(unit, kGain) == 2.5f && param(unit, kTilt) == 0.25f);
        check_gain(unit, channels, 2.5f);
        uninitialize(unit);
        CHECK(AudioComponentInstanceDispose(unit.unit) == noErr);
        // Another slice size after the restore: the scratch engine does not fit, a new one is made.
        Unit other = make(component, channels);
        const uint32_t again = hooks.creates();
        CHECK(set_class_info(other, saved) == noErr);
        const UInt32 slice = 1024;
        CHECK(AudioUnitSetProperty(other.unit, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &slice, sizeof(slice)) == noErr);
        initialize(other);
        CHECK(hooks.creates() - again == 2);
        CHECK(chunk(other) == kChunkA && param(other, kGain) == 2.5f);
        check_gain(other, channels, 2.5f);
        uninitialize(other);
        CHECK(AudioComponentInstanceDispose(other.unit) == noErr);
        std::printf("AU state %s: a restore before Initialize builds one engine, not two; a changed slice size builds a new one\n", label);
    }

    CFRelease(saved);
    for (Unit* u : {&source, &target, &early, &legacy, &legacy_early, &live, &idle, &grow, &large_target}) {
        if (u->engine) uninitialize(*u);
        CHECK(AudioComponentInstanceDispose(u->unit) == noErr);
    }
}
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    AudioComponent component = proof_register_component(argv[1]);
    hooks.last = reinterpret_cast<void* (*)()>(proof_bundle_symbol("op_test_last_engine"));
    hooks.set = reinterpret_cast<void (*)(void*, const uint8_t*, uint32_t)>(proof_bundle_symbol("op_test_set_chunk"));
    hooks.get = reinterpret_cast<uint32_t (*)(void*, uint8_t*, uint32_t)>(proof_bundle_symbol("op_test_get_chunk"));
    hooks.grow = reinterpret_cast<void (*)(void*, uint32_t)>(proof_bundle_symbol("op_test_grow_after_query"));
    hooks.creates = reinterpret_cast<uint32_t (*)()>(proof_bundle_symbol("op_test_creates"));
    hooks.sets = reinterpret_cast<uint32_t (*)()>(proof_bundle_symbol("op_test_param_sets"));
    CHECK(hooks.last && hooks.set && hooks.get && hooks.grow && hooks.creates && hooks.sets);
    check_one(component, 1, "mono");
    check_one(component, 2, "stereo");
    std::puts("AU state proof: ClassInfo carries the plugin state: round trip, old dictionary, corrupt blobs refused, size query, beside render: PASS");
    return 0;
}
