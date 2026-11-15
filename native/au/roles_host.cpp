// SPDX-License-Identifier: MIT
// The channel order proof (0.6 task 8) through the Audio Unit shell. The plugin is
// Role Tags (examples/role_tags), which declares every SDK layout from mono to 9.1.6
// and checks every input channel against the role it believes the channel has (see
// native/include/role_tags.h). The bundle is registered in this process only
// (AudioComponentRegister); nothing is installed. Not a DAW test.
//
// This host speaks Audio Unit only. For every channel count it runs every layout tag
// the unit offers in kAudioUnitProperty_SupportedChannelLayoutTags, and then the same
// speakers as channel descriptions in reverse order. It learns which speaker each of
// its channels is from CoreAudio (kAudioFormatProperty_ChannelLayoutForTag expands a
// tag into channel labels), never from the shell's table, and maps labels to SDK
// roles with its own table. Every output channel must come back with the plugin's tag
// of the same speaker.
//
//   AURolesProof <RoleTags.component>
#include "proof_au.h"
#include "role_tags.h"
#include <set>

using namespace roletags;

namespace {
constexpr UInt32 kFrames = 64;
using Matrix = std::vector<std::vector<float>>;

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

AudioUnit make_unit(AudioComponent component, UInt32 channels) {
    AudioUnit u = nullptr;
    CHECK(AudioComponentInstanceNew(component, &u) == noErr && u);
    const AudioStreamBasicDescription f = format(channels);
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &f, sizeof(f)) == noErr);
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 0, &f, sizeof(f)) == noErr);
    UInt32 maxFrames = 512;
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_MaximumFramesPerSlice, kAudioUnitScope_Global, 0, &maxFrames, sizeof(maxFrames)) == noErr);
    return u;
}

std::vector<AudioChannelLayoutTag> tags_of(AudioUnit u, AudioUnitScope scope) {
    UInt32 size = 0;
    Boolean writable = false;
    if (AudioUnitGetPropertyInfo(u, kAudioUnitProperty_SupportedChannelLayoutTags, scope, 0, &size, &writable) != noErr) return {};
    std::vector<AudioChannelLayoutTag> tags(size / sizeof(AudioChannelLayoutTag));
    CHECK(AudioUnitGetProperty(u, kAudioUnitProperty_SupportedChannelLayoutTags, scope, 0, tags.data(), &size) == noErr);
    return tags;
}

// The labels CoreAudio gives a tag, in channel order.
std::vector<AudioChannelLabel> labels_of(AudioChannelLayoutTag tag) {
    UInt32 size = 0;
    CHECK(AudioFormatGetPropertyInfo(kAudioFormatProperty_ChannelLayoutForTag, sizeof(tag), &tag, &size) == noErr && size >= sizeof(AudioChannelLayout));
    std::vector<uint8_t> raw(size);
    auto* layout = reinterpret_cast<AudioChannelLayout*>(raw.data());
    CHECK(AudioFormatGetProperty(kAudioFormatProperty_ChannelLayoutForTag, sizeof(tag), &tag, &size, layout) == noErr);
    // The answer keeps the tag and fills in a description, with its label, per channel.
    CHECK(layout->mNumberChannelDescriptions > 0);
    std::vector<AudioChannelLabel> labels;
    for (UInt32 j = 0; j < layout->mNumberChannelDescriptions; ++j) labels.push_back(layout->mChannelDescriptions[j].mChannelLabel);
    return labels;
}

// What each CoreAudio channel label is in the SDK's role ids. Left and Right Surround
// are the SDK's back pair (BL, BR) unless the layout also has a rear pair (Rls, Rrs),
// in which case they are the sides; top middle and top surround are the top side pair.
std::vector<uint8_t> speakers_of(const std::vector<AudioChannelLabel>& labels) {
    bool hasRear = false;
    for (AudioChannelLabel l : labels)
        hasRear = hasRear || l == kAudioChannelLabel_RearSurroundLeft || l == kAudioChannelLabel_LeftBackSurround;
    std::vector<uint8_t> codes;
    for (AudioChannelLabel l : labels) {
        uint8_t code = kUnknown;
        switch (l) {
        case kAudioChannelLabel_Mono: code = kMonoCode; break;
        case kAudioChannelLabel_Left: code = FL; break;
        case kAudioChannelLabel_Right: code = FR; break;
        case kAudioChannelLabel_Center: code = FC; break;
        case kAudioChannelLabel_LFEScreen: code = LFE; break;
        case kAudioChannelLabel_LeftSurround: code = hasRear ? SL : BL; break;
        case kAudioChannelLabel_RightSurround: code = hasRear ? SR : BR; break;
        case kAudioChannelLabel_LeftSideSurround: case kAudioChannelLabel_LeftSurroundDirect: code = SL; break;
        case kAudioChannelLabel_RightSideSurround: case kAudioChannelLabel_RightSurroundDirect: code = SR; break;
        case kAudioChannelLabel_RearSurroundLeft: case kAudioChannelLabel_LeftBackSurround: code = BL; break;
        case kAudioChannelLabel_RearSurroundRight: case kAudioChannelLabel_RightBackSurround: code = BR; break;
        case kAudioChannelLabel_CenterSurround: code = BC; break;
        case kAudioChannelLabel_LeftCenter: code = FLC; break;
        case kAudioChannelLabel_RightCenter: code = FRC; break;
        case kAudioChannelLabel_TopCenterSurround: code = TC; break;
        case kAudioChannelLabel_VerticalHeightLeft: code = TFL; break;
        case kAudioChannelLabel_VerticalHeightCenter: code = TFC; break;
        case kAudioChannelLabel_VerticalHeightRight: code = TFR; break;
        case kAudioChannelLabel_TopBackLeft: case kAudioChannelLabel_LeftTopRear: code = TBL; break;
        case kAudioChannelLabel_TopBackCenter: case kAudioChannelLabel_CenterTopRear: code = TBC; break;
        case kAudioChannelLabel_TopBackRight: case kAudioChannelLabel_RightTopRear: code = TBR; break;
        case kAudioChannelLabel_LeftTopMiddle: case kAudioChannelLabel_LeftTopSurround: code = TSL; break;
        case kAudioChannelLabel_RightTopMiddle: case kAudioChannelLabel_RightTopSurround: code = TSR; break;
        case kAudioChannelLabel_LeftWide: code = FLW; break;
        case kAudioChannelLabel_RightWide: code = FRW; break;
        default: break;
        }
        if (code == kUnknown) std::fprintf(stderr, "AU roles: label %u has no SDK role in this host\n", static_cast<unsigned>(l));
        CHECK(code != kUnknown);
        codes.push_back(code);
    }
    return codes;
}

// The layout the host sets on both elements: a tag, or channel descriptions.
std::vector<uint8_t> layout_bytes(AudioChannelLayoutTag tag, const std::vector<AudioChannelLabel>* labels) {
    const UInt32 n = labels ? static_cast<UInt32>(labels->size()) : 0;
    std::vector<uint8_t> raw(offsetof(AudioChannelLayout, mChannelDescriptions) + std::max<UInt32>(n, 1) * sizeof(AudioChannelDescription), 0);
    auto* layout = reinterpret_cast<AudioChannelLayout*>(raw.data());
    layout->mChannelLayoutTag = labels ? kAudioChannelLayoutTag_UseChannelDescriptions : tag;
    layout->mNumberChannelDescriptions = n;
    for (UInt32 j = 0; j < n; ++j) layout->mChannelDescriptions[j].mChannelLabel = (*labels)[j];
    return raw;
}

Matrix render(AudioUnit u, UInt32 channels) {
    Matrix out(channels, std::vector<float>(kFrames, 7.0f));
    std::vector<uint8_t> raw(sizeof(AudioBufferList) + sizeof(AudioBuffer) * channels, 0);
    auto* list = reinterpret_cast<AudioBufferList*>(raw.data());
    list->mNumberBuffers = channels;
    for (UInt32 ch = 0; ch < channels; ++ch) list->mBuffers[ch] = {1, kFrames * static_cast<UInt32>(sizeof(float)), out[ch].data()};
    AudioTimeStamp stamp{};
    stamp.mFlags = kAudioTimeStampSampleTimeValid;
    AudioUnitRenderActionFlags flags = 0;
    CHECK(AudioUnitRender(u, &flags, &stamp, 0, kFrames, list) == noErr);
    return out;
}

// One run: the host's layout on both elements, `speakers` the role of each host channel
// (shifted by `skew` for the negative control), then the output checked or all flagged.
void run(AudioComponent component, const char* name, AudioChannelLayoutTag tag, const std::vector<AudioChannelLabel>* labels,
         std::vector<uint8_t> speakers, bool skew = false) {
    const UInt32 n = static_cast<UInt32>(speakers.size());
    AudioUnit u = make_unit(component, n);
    const std::vector<uint8_t> layout = layout_bytes(tag, labels);
    for (AudioUnitScope scope : {kAudioUnitScope_Input, kAudioUnitScope_Output})
        CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_AudioChannelLayout, scope, 0, layout.data(), static_cast<UInt32>(layout.size())) == noErr);
    Source src;
    std::vector<uint8_t> written = speakers;
    if (skew) std::rotate(written.begin(), written.begin() + 1, written.end());
    for (uint8_t code : written) src.values.push_back(inTag(code));
    AURenderCallbackStruct callback{source_proc, &src};
    CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &callback, sizeof(callback)) == noErr);
    CHECK(AudioUnitInitialize(u) == noErr);
    const Matrix out = render(u, n);
    if (skew) {
        CHECK(allFlagged(out));
    } else {
        for (UInt32 j = 0; j < n; ++j) CHECK(checkChannel("AU", name, j, speakers[j], out[j].data(), kFrames));
    }
    CHECK(AudioUnitUninitialize(u) == noErr);
    CHECK(AudioComponentInstanceDispose(u) == noErr);
}

std::string tag_name(AudioChannelLayoutTag tag) {
    switch (tag) {
    case kAudioChannelLayoutTag_Mono: return "Mono";
    case kAudioChannelLayoutTag_Stereo: return "Stereo";
    case kAudioChannelLayoutTag_MPEG_3_0_A: return "MPEG_3_0_A";
    case kAudioChannelLayoutTag_Quadraphonic: return "Quadraphonic";
    case kAudioChannelLayoutTag_MPEG_5_0_A: return "MPEG_5_0_A";
    case kAudioChannelLayoutTag_MPEG_5_0_B: return "MPEG_5_0_B";
    case kAudioChannelLayoutTag_MPEG_5_1_A: return "MPEG_5_1_A";
    case kAudioChannelLayoutTag_MPEG_5_1_B: return "MPEG_5_1_B";
    case kAudioChannelLayoutTag_MPEG_5_1_C: return "MPEG_5_1_C";
    case kAudioChannelLayoutTag_MPEG_5_1_D: return "MPEG_5_1_D";
    case kAudioChannelLayoutTag_MPEG_6_1_A: return "MPEG_6_1_A";
    case kAudioChannelLayoutTag_MPEG_7_1_C: return "MPEG_7_1_C";
    case kAudioChannelLayoutTag_Atmos_5_1_2: return "Atmos_5_1_2";
    case kAudioChannelLayoutTag_Atmos_5_1_4: return "Atmos_5_1_4";
    case kAudioChannelLayoutTag_Atmos_7_1_2: return "Atmos_7_1_2";
    case kAudioChannelLayoutTag_Atmos_7_1_4: return "Atmos_7_1_4";
    case kAudioChannelLayoutTag_Logic_Atmos_7_1_4_B: return "Logic_Atmos_7_1_4_B";
    case kAudioChannelLayoutTag_Atmos_9_1_6: return "Atmos_9_1_6";
    default: {
        char text[32];
        std::snprintf(text, sizeof text, "tag %u", static_cast<unsigned>(tag >> 16));
        return text;
    }
    }
}
}  // namespace

int main(int argc, char** argv) {
    CHECK(argc == 2);
    AudioComponent component = proof_register_component(argv[1]);

    // The tag of every layout Role Tags declares (CoreAudioBaseTypes.h names). Each must be
    // offered; every other tag the unit offers is run too.
    const std::pair<const char*, AudioChannelLayoutTag> layouts[] = {
        {"mono", kAudioChannelLayoutTag_Mono},          {"stereo", kAudioChannelLayoutTag_Stereo},
        {"LCR", kAudioChannelLayoutTag_MPEG_3_0_A},     {"quad", kAudioChannelLayoutTag_Quadraphonic},
        {"5.0", kAudioChannelLayoutTag_MPEG_5_0_A},     {"5.1", kAudioChannelLayoutTag_MPEG_5_1_A},
        {"6.1", kAudioChannelLayoutTag_MPEG_6_1_A},     {"7.1", kAudioChannelLayoutTag_MPEG_7_1_C},
        {"7.1.4", kAudioChannelLayoutTag_Atmos_7_1_4},  {"5.1.2", kAudioChannelLayoutTag_Atmos_5_1_2},
        {"5.1.4", kAudioChannelLayoutTag_Atmos_5_1_4},  {"7.1.2", kAudioChannelLayoutTag_Atmos_7_1_2},
        {"9.1.6", kAudioChannelLayoutTag_Atmos_9_1_6},
    };
    std::set<UInt32> counts;
    for (const auto& l : layouts) counts.insert(AudioChannelLayoutTag_GetNumberOfChannels(l.second));

    std::set<AudioChannelLayoutTag> ran;
    int described = 0;
    for (UInt32 n : counts) {
        AudioUnit probe = make_unit(component, n);
        const auto inTags = tags_of(probe, kAudioUnitScope_Input);
        const auto outTags = tags_of(probe, kAudioUnitScope_Output);
        CHECK(AudioComponentInstanceDispose(probe) == noErr);
        CHECK(inTags == outTags && !inTags.empty());
        // The auval fix: mono and stereo buses do not offer UseChannelDescriptions.
        const bool offersDescriptions = std::find(inTags.begin(), inTags.end(), kAudioChannelLayoutTag_UseChannelDescriptions) != inTags.end();
        CHECK(offersDescriptions == (n > 2));
        for (AudioChannelLayoutTag tag : inTags) {
            if (tag == kAudioChannelLayoutTag_UseChannelDescriptions) continue;
            const std::string name = tag_name(tag);
            const std::vector<AudioChannelLabel> labels = labels_of(tag);
            CHECK(labels.size() == n);
            const std::vector<uint8_t> speakers = speakers_of(labels);
            run(component, name.c_str(), tag, nullptr, speakers);
            ran.insert(tag);
            std::printf("AU roles: %-20s %2u channels, host order %s: every role at its index in and out\n", name.c_str(), n, codes(speakers).c_str());
            if (offersDescriptions) {
                // The same speakers as channel descriptions, last channel first.
                std::vector<AudioChannelLabel> reversed(labels.rbegin(), labels.rend());
                const std::vector<uint8_t> rspeakers(speakers.rbegin(), speakers.rend());
                run(component, (name + " descriptions").c_str(), 0, &reversed, rspeakers);
                ++described;
            }
        }
    }
    for (const auto& l : layouts) {
        if (!ran.count(l.second)) std::fprintf(stderr, "AU roles: %s (%s) is not offered\n", l.first, tag_name(l.second).c_str());
        CHECK(ran.count(l.second));
    }
    std::printf("AU roles: %d tagged layouts and %d channel description layouts (reversed) run\n", static_cast<int>(ran.size()), described);

    // The negative control: tags one channel off on the widest bed are caught.
    {
        const std::vector<AudioChannelLabel> labels = labels_of(kAudioChannelLayoutTag_Atmos_9_1_6);
        run(component, "9.1.6 skewed", kAudioChannelLayoutTag_Atmos_9_1_6, nullptr, speakers_of(labels), true);
        std::puts("AU roles: Atmos_9_1_6 with the tags one channel off: every channel flagged (negative control)");
    }
    // A tag with the right channel count but other speakers is refused.
    {
        AudioUnit u = make_unit(component, 10);
        AudioChannelLayout other{};
        other.mChannelLayoutTag = (207U << 16) | 10;  // CICP_16: L R C LFE Ls Rs Vhl Vhr Lts Rts
        CHECK(AudioUnitSetProperty(u, kAudioUnitProperty_AudioChannelLayout, kAudioUnitScope_Input, 0, &other, sizeof(other)) != noErr);
        CHECK(AudioComponentInstanceDispose(u) == noErr);
        std::puts("AU roles: CICP_16 (front and top side heights, not a layout of the plugin) is refused");
    }
    std::puts("AU roles proof: mono, stereo, LCR, quad, 5.0, 5.1, 6.1, 7.1, 7.1.4, 5.1.2, 5.1.4, 7.1.2, 9.1.6; skipped: none: PASS");
    return 0;
}
