// SPDX-License-Identifier: MIT
// The identity pinning proof (porting toolkit task 2) through the Audio Unit shell.
// The plugin is Pinned (examples/pinned) in four builds, each behind the real shell
// and registered in this process only:
//
//   Pinned A   pinned ids, parameters level, offset, invert
//   Pinned B   pinned ids, parameters invert, drive (new), offset, level
//   Plain A    the first table, no pins
//   Plain B    the second table, no pins
//
// This host does what an Audio Unit host does with a project. It keeps the
// component description (type, subtype, manufacturer), the ClassInfo dictionary
// the unit wrote, and an automation lane whose points name a parameter by its
// AudioUnitParameterID. Saved with Pinned A and reopened with Pinned B, the
// project must find the same component, put every saved value on its own
// parameter, and play the lane to the same audio, sample for sample; and the
// same from B back to A. The subtype and manufacturer of the pinned builds are
// the ones the plugin pins: the build was given none (native/CMakeLists.txt
// asked the plugin).
//
// The negative control is the same project through Plain A and Plain B, where
// an AudioUnitParameterID is the parameter's position, as it always was: the
// reordered build refuses the saved state and the lane moves other parameters.
// Not a DAW test.
//
//   AUIdentityProof <Pinned A.component> <Pinned B.component> <Plain A.component> <Plain B.component>
#include "proof_au.h"
#include <utility>

namespace {
constexpr UInt32 kFrames = 64;
// The pins of examples/pinned/src/lib.rs. The Audio Unit of the "old" plugin numbered
// level 7, so that format has a pin of its own.
constexpr AudioUnitParameterID kLevel = 7;
constexpr AudioUnitParameterID kOffset = 0x6F666673;
constexpr AudioUnitParameterID kInvert = 0x696E7672;
constexpr AudioUnitParameterID kDrive = 0x64726976;
constexpr OSType kSubtype = 'PnEx';
constexpr OSType kManufacturer = 'OpTs';

using Audio = std::vector<float>;

// One automation point: the parameter as the host stored it, the sample and the value.
struct LanePoint {
    AudioUnitParameterID id;
    UInt32 offset;
    float value;
};

float gInput = 0.0f;
OSStatus constantInput(void*, AudioUnitRenderActionFlags*, const AudioTimeStamp*, UInt32, UInt32 frames, AudioBufferList* data) {
    for (UInt32 channel = 0; channel < data->mNumberBuffers; ++channel) {
        auto* samples = static_cast<float*>(data->mBuffers[channel].mData);
        if (!samples) return kAudio_ParamError;
        std::fill(samples, samples + frames, gInput);
        data->mBuffers[channel].mDataByteSize = frames * sizeof(float);
    }
    return noErr;
}

struct Unit {
    AudioUnit unit = nullptr;
    AudioComponentDescription desc{};
    double clock = 0.0;

    explicit Unit(AudioComponent component, bool initialize = true) {
        CHECK(AudioComponentGetDescription(component, &desc) == noErr);
        unit = proof_make_unit(component, 2);
        AURenderCallbackStruct callback{constantInput, nullptr};
        CHECK(AudioUnitSetProperty(unit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback, sizeof(callback)) == noErr);
        if (initialize) CHECK(AudioUnitInitialize(unit) == noErr);
    }
    ~Unit() {
        AudioUnitUninitialize(unit);
        AudioComponentInstanceDispose(unit);
    }
    Unit(const Unit&) = delete;
    Unit& operator=(const Unit&) = delete;

    std::vector<AudioUnitParameterID> ids() const {
        UInt32 size = 0;
        Boolean writable = false;
        CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &size, &writable) == noErr);
        std::vector<AudioUnitParameterID> list(size / sizeof(AudioUnitParameterID));
        if (!list.empty()) CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, list.data(), &size) == noErr);
        return list;
    }
    std::string nameOf(AudioUnitParameterID id) const {
        AudioUnitParameterInfo info{};
        UInt32 size = sizeof(info);
        if (AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, id, &info, &size) != noErr) return "<refused>";
        char text[128] = {};
        CHECK(info.cfNameString && CFStringGetCString(info.cfNameString, text, sizeof(text), kCFStringEncodingUTF8));
        if (info.flags & kAudioUnitParameterFlag_CFNameRelease) CFRelease(info.cfNameString);
        return text;
    }
    AudioUnitParameterID idOf(const char* name) const {
        for (AudioUnitParameterID id : ids())
            if (nameOf(id) == name) return id;
        std::fprintf(stderr, "no parameter named %s\n", name);
        std::exit(1);
    }
    bool has(AudioUnitParameterID id) const {
        const auto list = ids();
        return std::find(list.begin(), list.end(), id) != list.end();
    }
    void set(AudioUnitParameterID id, float value) { CHECK(AudioUnitSetParameter(unit, id, kAudioUnitScope_Global, 0, value, 0) == noErr); }
    void set(const std::vector<std::pair<AudioUnitParameterID, float>>& values) {
        for (const auto& [id, value] : values) set(id, value);
    }
    float get(AudioUnitParameterID id) const {
        AudioUnitParameterValue value = -99.0f;
        CHECK(AudioUnitGetParameter(unit, id, kAudioUnitScope_Global, 0, &value) == noErr);
        return value;
    }
    // One block of a constant input with the points of a lane (scheduled at their sample
    // offsets, as a host plays automation); the left channel.
    Audio render(float input, const std::vector<LanePoint>& points) {
        std::vector<AudioUnitParameterEvent> events;
        for (const LanePoint& point : points) {
            AudioUnitParameterEvent event{};
            event.scope = kAudioUnitScope_Global;
            event.element = 0;
            event.parameter = point.id;
            event.eventType = kParameterEvent_Immediate;
            event.eventValues.immediate.bufferOffset = point.offset;
            event.eventValues.immediate.value = point.value;
            events.push_back(event);
        }
        // A point for a parameter the unit does not have is refused; a host skips it.
        for (const AudioUnitParameterEvent& event : events) (void)AudioUnitScheduleParameters(unit, &event, 1);
        gInput = input;
        ProofBuffers buffers(2, kFrames);
        CHECK(proof_render(unit, buffers, kFrames, clock) == noErr);
        clock += kFrames;
        CHECK(buffers.data[0] == buffers.data[1]);
        return buffers.data[0];
    }
    CFDictionaryRef save() const {
        CFPropertyListRef saved = nullptr;
        UInt32 size = sizeof(saved);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &saved, &size) == noErr && saved);
        CHECK(CFGetTypeID(saved) == CFDictionaryGetTypeID());
        return static_cast<CFDictionaryRef>(saved);
    }
    OSStatus load(CFPropertyListRef plist) { return AudioUnitSetProperty(unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0, &plist, sizeof(plist)); }
};

// The lane of the proof: level and offset move in the middle of the block.
std::vector<LanePoint> lane(AudioUnitParameterID level, AudioUnitParameterID offset) { return {{level, 16, 1.5f}, {offset, 32, -0.5f}, {level, 48, 0.25f}}; }

// The audio the lane must give after the saved values (level 0.5, offset 0.25, inverted)
// with an input of 0.5, from the plugin's own formula. Every value is exact in a float.
Audio expected() {
    Audio out(kFrames);
    for (UInt32 n = 0; n < kFrames; ++n) {
        const float level = n < 16 ? 0.5f : n < 48 ? 1.5f : 0.25f;
        const float offset = n < 32 ? 0.25f : -0.5f;
        out[n] = -(0.5f * level + offset);
    }
    return out;
}

bool sameComponent(const AudioComponentDescription& a, const AudioComponentDescription& b) {
    return a.componentType == b.componentType && a.componentSubType == b.componentSubType && a.componentManufacturer == b.componentManufacturer;
}

// The dictionary without the shell's own key: what a preset written before the shell kept
// the plugin's state holds, the base class's parameter values by AudioUnitParameterID.
CFMutableDictionaryRef withoutBlob(CFDictionaryRef saved) {
    CFMutableDictionaryRef copy = CFDictionaryCreateMutableCopy(nullptr, 0, saved);
    CHECK(CFDictionaryContainsKey(copy, CFSTR("OpenPluginState")));
    CFDictionaryRemoveValue(copy, CFSTR("OpenPluginState"));
    return copy;
}
}

int main(int argc, char** argv) {
    CHECK(argc == 5);
    // The two builds of a pair are one component (the same type, subtype and manufacturer),
    // so the second is registered as the next version of it.
    const AudioComponent pinnedA = proof_register_component(argv[1]);
    const AudioComponent pinnedB = proof_register_component(argv[2], 1);
    const AudioComponent plainA = proof_register_component(argv[3]);
    const AudioComponent plainB = proof_register_component(argv[4], 1);
    // ---- what each build shows the host ------------------------------------------------
    {
        Unit a(pinnedA), b(pinnedB), pa(plainA), pb(plainB);
        // The codes of the pinned builds come from the plugin (Plugin::au_ids): the build gave none.
        CHECK(a.desc.componentType == kAudioUnitType_Effect && a.desc.componentSubType == kSubtype && a.desc.componentManufacturer == kManufacturer);
        CHECK(sameComponent(a.desc, b.desc));
        CHECK(a.ids() == (std::vector<AudioUnitParameterID>{kLevel, kOffset, kInvert}));
        CHECK(b.ids() == (std::vector<AudioUnitParameterID>{kInvert, kDrive, kOffset, kLevel}));
        CHECK(a.nameOf(kLevel) == "Level" && b.nameOf(kLevel) == "Level" && a.nameOf(kInvert) == "Invert" && b.nameOf(kDrive) == "Drive");
        CHECK(a.get(kLevel) == 1.0f && b.get(kLevel) == 1.0f && a.get(kOffset) == 0.0f && b.get(kDrive) == 0.0f);
        // Text goes by the host's id to the right parameter in either order.
        const AudioUnitParameterValue on = 1.0f;
        CHECK(proof_text_of(a.unit, kInvert, &on) == "On" && proof_text_of(b.unit, kInvert, &on) == "On");
        for (AudioUnitParameterID stale : {AudioUnitParameterID(0), AudioUnitParameterID(1), AudioUnitParameterID(2), AudioUnitParameterID(3), kDrive + 1}) {
            CHECK(a.nameOf(stale) == "<refused>" && b.nameOf(stale) == "<refused>");
            CHECK(AudioUnitSetParameter(a.unit, stale, kAudioUnitScope_Global, 0, 1.0f, 0) != noErr);
            bool ok = true;
            proof_text_of(a.unit, stale, &on, &ok);
            CHECK(!ok);
        }
        CHECK(!a.has(kDrive));
        // Without pins an AudioUnitParameterID is the position.
        CHECK(pa.ids() == (std::vector<AudioUnitParameterID>{0, 1, 2}));
        CHECK(pb.ids() == (std::vector<AudioUnitParameterID>{0, 1, 2, 3}));
        CHECK(sameComponent(pa.desc, pb.desc) && !sameComponent(pa.desc, a.desc));
        std::printf("AU identity: Pinned A and B are both the component aufx 'PnEx' 'OpTs' (pinned in the plugin) with parameter IDs 7, %08X, %08X (B also %08X) in their own orders; Plain A and B show positions 0..n-1\n",
                    static_cast<unsigned>(kOffset), static_cast<unsigned>(kInvert), static_cast<unsigned>(kDrive));
    }
    // ---- a project saved with Pinned A reopens in Pinned B ------------------------------
    {
        Unit first(pinnedA);
        first.set({{kLevel, 0.5f}, {kOffset, 0.25f}, {kInvert, 1.0f}});
        const CFDictionaryRef saved = first.save();
        const std::vector<LanePoint> points = lane(first.idOf("Level"), first.idOf("Offset"));
        const Audio heard = first.render(0.5f, points);
        CHECK(heard == expected());

        for (bool initialized : {true, false}) {
            Unit second(pinnedB, initialized);
            CHECK(sameComponent(second.desc, first.desc));
            for (const LanePoint& point : points) CHECK(second.has(point.id));
            CHECK(second.load(saved) == noErr);
            CHECK(second.get(kLevel) == 0.5f && second.get(kOffset) == 0.25f && second.get(kInvert) == 1.0f && second.get(kDrive) == 0.0f);
            if (!initialized) CHECK(AudioUnitInitialize(second.unit) == noErr);
            const Audio still = second.render(0.5f, {});
            for (float sample : still) CHECK(sample == -(0.5f * 0.5f + 0.25f));
            // The lane, from the saved values again.
            second.set({{kLevel, 0.5f}, {kOffset, 0.25f}});
            CHECK(second.render(0.5f, points) == heard);
        }
        std::printf("AU identity: a project of Pinned A (level 0.5, offset 0.25, inverted, a 3-point lane) reopens in Pinned B, initialized or not: same component, every value on its own parameter, drive at its default, the lane's audio identical over %u samples\n", kFrames);

        // A preset without the shell's key holds the base class's values by parameter ID:
        // it lands on the same parameters too, and names no parameter the build lacks.
        const CFMutableDictionaryRef old = withoutBlob(saved);
        Unit plainLoad(pinnedB);
        CHECK(plainLoad.load(old) == noErr);
        CHECK(plainLoad.get(kLevel) == 0.5f && plainLoad.get(kOffset) == 0.25f && plainLoad.get(kInvert) == 1.0f && plainLoad.get(kDrive) == 0.0f);
        CFRelease(old);
        CFRelease(saved);

        // And back: a project of B, drive included, in A.
        Unit newer(pinnedB);
        newer.set({{kLevel, 2.0f}, {kOffset, -0.25f}, {kInvert, 0.0f}, {kDrive, 0.5f}});
        const CFDictionaryRef savedB = newer.save();
        const Audio driven = newer.render(0.5f, points);
        for (bool initialized : {true, false}) {
            Unit back(pinnedA, initialized);
            CHECK(back.load(savedB) == noErr);
            CHECK(back.get(kLevel) == 2.0f && back.get(kOffset) == -0.25f && back.get(kInvert) == 0.0f);
            // The value of drive, which this build does not have, adds no parameter to it.
            CHECK(back.ids() == (std::vector<AudioUnitParameterID>{kLevel, kOffset, kInvert}));
            if (!initialized) CHECK(AudioUnitInitialize(back.unit) == noErr);
            const Audio plain = back.render(0.5f, points);
            for (UInt32 n = 0; n < kFrames; ++n) CHECK(plain[n] * 1.5f == driven[n]);
        }
        // The same without the shell's key, into a unit that is not initialized: the base
        // class would keep a value for the unknown ID; the host is still shown three parameters.
        const CFMutableDictionaryRef oldB = withoutBlob(savedB);
        Unit lazy(pinnedA, false);
        CHECK(lazy.load(oldB) == noErr);
        CHECK(lazy.ids() == (std::vector<AudioUnitParameterID>{kLevel, kOffset, kInvert}));
        CHECK(lazy.get(kLevel) == 2.0f && lazy.get(kOffset) == -0.25f && lazy.nameOf(kDrive) == "<refused>");
        CFRelease(oldB);
        CFRelease(savedB);
        std::printf("AU identity: a project of Pinned B reopens in Pinned A: the three shared parameters and the lane land as saved; the value of drive, which A does not have, is dropped and adds no parameter\n");
    }
    // ---- points the pinned build must not take -------------------------------------------
    {
        Unit a(pinnedA);
        a.set(kLevel, 0.5f);
        const Audio quiet = a.render(0.5f, {});
        for (AudioUnitParameterID stale : {kDrive, AudioUnitParameterID(0), AudioUnitParameterID(1), AudioUnitParameterID(2)}) CHECK(a.render(0.5f, {{stale, 0, 1.0f}, {stale, 8, 1.0f}}) == quiet);
        CHECK(a.get(kLevel) == 0.5f && a.get(kOffset) == 0.0f && a.get(kInvert) == 0.0f);
        std::printf("AU identity: in Pinned A, points for parameter IDs it does not have move nothing\n");
    }
    // ---- the negative control: the same project without pins ----------------------------
    {
        Unit first(plainA);
        const AudioUnitParameterID level = first.idOf("Level"), offset = first.idOf("Offset"), invert = first.idOf("Invert");
        first.set({{level, 0.5f}, {offset, 0.25f}, {invert, 1.0f}});
        const CFDictionaryRef saved = first.save();
        const std::vector<LanePoint> points = lane(level, offset);
        const Audio heard = first.render(0.5f, points);
        CHECK(heard == expected());

        Unit second(plainB);
        CHECK(sameComponent(second.desc, first.desc));
        // The positional state of three parameters is not a state of four: refused whole.
        CHECK(second.load(saved) != noErr);
        CHECK(second.get(second.idOf("Level")) == 1.0f && second.get(second.idOf("Invert")) == 0.0f);
        // The lane's ids are positions: 0 was level and is invert now, 1 was offset and is drive.
        CHECK(second.idOf("Invert") == level && second.idOf("Drive") == offset);
        const std::vector<std::pair<AudioUnitParameterID, float>> byName = {{second.idOf("Level"), 0.5f}, {second.idOf("Offset"), 0.25f}, {second.idOf("Invert"), 1.0f}, {second.idOf("Drive"), 0.0f}};
        // With the values put right by hand the lane still lands on the wrong parameters...
        second.set(byName);
        CHECK(second.render(0.5f, points) != heard);
        // ...and addressed by name it is right: only the ids were wrong.
        second.set(byName);
        CHECK(second.render(0.5f, lane(second.idOf("Level"), second.idOf("Offset"))) == heard);
        // A preset without the shell's key puts each value on the parameter now at that position.
        const CFMutableDictionaryRef old = withoutBlob(saved);
        Unit third(plainB);
        CHECK(third.load(old) == noErr);
        CHECK(third.get(third.idOf("Invert")) == 0.5f && third.get(third.idOf("Level")) == 1.0f);
        CFRelease(old);
        CFRelease(saved);
        std::printf("AU identity, negative control: without pins the reordered build refuses the saved state, a preset of parameter values lands on other parameters, and the lane moves other parameters (the audio differs); addressed by name the same lane is right\n");
    }
    std::printf("AU identity proof passed\n");
    return 0;
}
