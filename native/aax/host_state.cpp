// SPDX-License-Identifier: MIT
// Proves the AAX state chunk. It drives the shell's AAX_CEffectParameters
// chunk calls (GetNumberOfChunks, GetChunkIDFromIndex, GetChunkSize, GetChunk,
// SetChunk, CompareActiveChunk) on real parameter objects built from the
// op_shell_* table, without an AAX controller, and checks:
//   - a changed parameter set saved as a chunk and loaded into a fresh
//     instance gives the same values and the same audio as the original;
//   - corrupt, truncated, extended and foreign chunks are refused and leave
//     every value unchanged;
//   - the master bypass is saved, restored, and still copies the dry signal.
// Not a Pro Tools test.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_state_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "OpenPluginAAXState.h"
#include "AAX_CEffectParameters.h"
#include "AAX_Errors.h"
#include "proof_shell.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <functional>
#include <memory>
#include <string>
#include <vector>

using namespace openplugin_aax;

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX state proof failed: %s\n", message);
        std::exit(1);
    }
}

// A chunk as the host holds it: the header followed by room for fData.
struct Chunk {
    std::vector<unsigned char> storage;
    explicit Chunk(size_t dataBytes) : storage(sizeof(AAX_SPlugInChunk) + dataBytes + 8, 0) {}
    AAX_SPlugInChunk* get() { return reinterpret_cast<AAX_SPlugInChunk*>(storage.data()); }
    unsigned char* data() { return reinterpret_cast<unsigned char*>(get()->fData); }
};

struct Instance {
    std::unique_ptr<AAX_CEffectParameters> params;
    explicit Instance(uint32_t channels) : params(createParameters()) {
        require(params && proofInit(params.get(), 48000.0, channels), "parameters build without a controller");
    }
    PacketBuffer packet() const {
        PacketBuffer out;
        require(proofPacket(params.get(), &out), "read packet");
        return out;
    }
    // Save through the AAX chunk calls exactly as a host does.
    Chunk save() const {
        int32_t count = 0;
        require(params->GetNumberOfChunks(&count) == AAX_SUCCESS && count == 1, "one chunk");
        AAX_CTypeID id = 0;
        require(params->GetChunkIDFromIndex(0, &id) == AAX_SUCCESS && id == state::kChunkId, "chunk id");
        uint32_t size = 0;
        require(params->GetChunkSize(id, &size) == AAX_SUCCESS && size > state::encodedSizeV1(), "chunk size is version 1 plus the plugin blob");
        Chunk chunk(size);
        chunk.get()->fChunkID = id;
        require(params->GetChunk(id, chunk.get()) == AAX_SUCCESS, "GetChunk");
        require(static_cast<uint32_t>(chunk.get()->fSize) == size, "GetChunk reports the size GetChunkSize promised");
        require(chunk.get()->fVersion == static_cast<int32_t>(state::kVersion), "chunk version");
        return chunk;
    }
};

static bool same(const PacketBuffer& a, const PacketBuffer& b) {
    if (a.count() != b.count() || a.get()->bypass != b.get()->bypass) return false;
    for (uint32_t i = 0; i < a.count(); ++i)
        if (a.plain()[i] != b.plain()[i]) return false;
    return true;
}

static std::vector<float> render(ProcessState& state, PacketBuffer packet, int channels, int blocks, std::vector<float>* dry = nullptr,
                                 std::vector<float>* rightOut = nullptr) {
    constexpr int kFrames = 512;
    std::vector<float> inL(kFrames), inR(kFrames), outL(kFrames), outR(kFrames);
    float* input[] = {inL.data(), channels > 1 ? inR.data() : nullptr};
    float* output[] = {outL.data(), channels > 1 ? outR.data() : nullptr};
    int32_t frames = kFrames;
    auto* statePtr = &state;
    Context context{input, output, &frames, packet.get(), &statePtr};
    Context* contexts[] = {&context};
    for (int block = 0; block < blocks; ++block) {
        fill_sine(inL.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (channels > 1) fill_sine(inR.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (dry) *dry = inL;
        process(contexts, contexts + 1);
    }
    if (rightOut) *rightOut = outR;
    return outL;
}

static uint32_t find_param(const std::vector<OpParamDesc>& descs, const char* name) {
    for (uint32_t i = 0; i < descs.size(); ++i)
        if (std::strcmp(descs[i].name, name) == 0) return i;
    std::fprintf(stderr, "AAX state proof: the shell has no parameter named %s\n", name);
    std::exit(1);
}

int main() {
    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.param_count > 0 && info.param_count <= kMaxParams, "shell info");
    std::vector<OpParamDesc> descs(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) require(op_shell_param(i, &descs[i]) != 0, "shell param");
    const uint32_t gain = find_param(descs, "Gain");
    const uint32_t warm = find_param(descs, "Warm");

    // The version-1 header is frozen: a later format must still read these bytes.
    {
        Instance probe(2);
        Chunk chunk = probe.save();
        const unsigned char* d = chunk.data();
        require(d[0] == 'O' && d[1] == 'P' && d[2] == 'S' && d[3] == 'T', "chunk magic bytes");
        require(d[4] == 2 && d[5] == 0 && d[6] == 0 && d[7] == 0, "chunk payload version 2, little-endian");
        require(d[8] == info.param_count && d[9] == 0 && d[10] == 0 && d[11] == 0, "chunk parameter count");
        const size_t fixed = 20 + 4 * static_cast<size_t>(info.param_count);
        const size_t blobLength = static_cast<size_t>(d[fixed]) | static_cast<size_t>(d[fixed + 1]) << 8 | static_cast<size_t>(d[fixed + 2]) << 16 | static_cast<size_t>(d[fixed + 3]) << 24;
        require(blobLength > 0 && static_cast<size_t>(chunk.get()->fSize) == fixed + 4 + blobLength, "chunk length is header, one float per parameter, a blob length and the blob");
        // Wrong id and index are refused.
        AAX_CTypeID id = 7;
        require(probe.params->GetChunkIDFromIndex(1, &id) == AAX_ERROR_INVALID_CHUNK_INDEX && id == 0, "chunk index 1 refused");
        uint32_t size = 9;
        require(probe.params->GetChunkSize('xxxx', &size) == AAX_ERROR_INVALID_CHUNK_ID && size == 0, "unknown chunk id size refused");
        require(probe.params->GetChunk('xxxx', chunk.get()) == AAX_ERROR_INVALID_CHUNK_ID, "unknown chunk id get refused");
        require(probe.params->SetChunk('xxxx', chunk.get()) == AAX_ERROR_INVALID_CHUNK_ID, "unknown chunk id set refused");
    }

    // Round trip. Values: a gain that is not a default, Warm on, through each channel layout.
    for (uint32_t channels = 1; channels <= 2; ++channels) {
        Instance original(channels);
        const PacketBuffer defaults = original.packet();
        const float testGain = static_cast<float>(std::fmax(descs[gain].min, std::fmin(descs[gain].max, 6.0)));
        require(proofSetPlain(original.params.get(), gain, testGain), "set gain");
        require(proofSetPlain(original.params.get(), warm, 1.0f), "set warm");
        const PacketBuffer changed = original.packet();
        require(!same(changed, defaults), "the changed set differs from the defaults");

        Chunk saved = original.save();
        Instance fresh(channels);
        require(same(fresh.packet(), defaults), "a fresh instance starts at the defaults");
        AAX_CBoolean equal = true;
        require(fresh.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && !equal, "a fresh instance differs from the saved chunk");
        require(fresh.params->SetChunk(state::kChunkId, saved.get()) == AAX_SUCCESS, "SetChunk");
        const PacketBuffer restored = fresh.packet();
        require(same(restored, changed), "restored values equal the saved values");
        for (uint32_t i = 0; i < info.param_count; ++i)
            require(restored.plain()[i] == changed.plain()[i], "every restored value is bit-identical");
        require(fresh.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && equal, "the restored instance compares equal");
        require(original.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && equal, "the original compares equal to its own chunk");
        // A second save of the restored instance is byte-identical.
        Chunk again = fresh.save();
        require(again.get()->fSize == saved.get()->fSize && std::memcmp(again.data(), saved.data(), static_cast<size_t>(saved.get()->fSize)) == 0,
                "saving the restored instance gives the same bytes");

        // A version 1 chunk (what earlier releases wrote) still loads: same values, same bypass.
        {
            state::Values v1{};
            v1.count = changed.count();
            v1.bypass = changed.get()->bypass != 0;
            for (uint32_t i = 0; i < v1.count; ++i) v1.plain[i] = changed.plain()[i];
            Chunk old(state::encodedSizeV1());
            old.get()->fChunkID = state::kChunkId;
            old.get()->fVersion = 1;
            old.get()->fSize = static_cast<int32_t>(state::encodedSizeV1());
            require(state::encodeV1(v1, old.data(), state::encodedSizeV1()), "encode version 1");
            Instance legacy(channels);
            require(legacy.params->SetChunk(state::kChunkId, old.get()) == AAX_SUCCESS, "a version 1 chunk loads");
            require(same(legacy.packet(), changed), "a version 1 chunk restores the same values");
            AAX_CBoolean eq = false;
            require(legacy.params->CompareActiveChunk(old.get(), &eq) == AAX_SUCCESS && eq, "a version 1 chunk compares equal on values");
        }

        ProcessState a, b;
        require(a.prepare(48000.0, channels) && b.prepare(48000.0, channels), "prepare");
        std::vector<float> rightA, rightB;
        const auto audioA = render(a, changed, static_cast<int>(channels), 8, nullptr, &rightA);
        const auto audioB = render(b, restored, static_cast<int>(channels), 8, nullptr, &rightB);
        require(max_difference(audioA, audioB) == 0.0, "restored instance gives the same audio");
        if (channels > 1) require(max_difference(rightA, rightB) == 0.0, "restored instance gives the same right channel");
        ProcessState d;
        require(d.prepare(48000.0, channels), "prepare default");
        const auto audioDefault = render(d, defaults, static_cast<int>(channels), 8);
        require(max_difference(audioA, audioDefault) > 0.01, "the changed values really change the audio");
        std::printf("AAX state %s: %u parameters round-trip bit-identical; audio difference %.3g; default vs changed %.3g\n",
                    channels == 1 ? "mono" : "stereo", info.param_count, max_difference(audioA, audioB), max_difference(audioA, audioDefault));

        // Corrupt chunks. Each must be refused and leave every value, and the
        // audio, unchanged.
        const size_t size = static_cast<size_t>(saved.get()->fSize);
        struct Case {
            const char* name;
            std::function<void(Chunk&)> damage;
        };
        const std::vector<Case> cases = {
            {"empty", [&](Chunk& c) { c.get()->fSize = 0; }},
            {"shorter than the header", [&](Chunk& c) { c.get()->fSize = 12; }},
            {"negative size", [&](Chunk& c) { c.get()->fSize = -4; }},
            {"one byte short", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size - 1); }},
            {"one float short", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size - 4); }},
            {"header only", [&](Chunk& c) { c.get()->fSize = 20; }},
            {"floats only, no blob length", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(20 + 4 * info.param_count); }},
            {"blob length larger than the data", [&](Chunk& c) { c.data()[20 + 4 * info.param_count + 3] = 0x7f; }},
            {"blob length zero", [&](Chunk& c) { std::memset(c.data() + 20 + 4 * info.param_count, 0, 4); c.get()->fSize = static_cast<int32_t>(20 + 4 * info.param_count + 4); }},
            {"blob damaged", [&](Chunk& c) { std::memset(c.data() + 24 + 4 * info.param_count, 0xA5, 8); }},
            {"one float long", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size + 4); }},
            {"one byte long", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size + 1); }},
            {"wrong magic", [&](Chunk& c) { c.data()[0] ^= 0xff; }},
            {"newer payload version", [&](Chunk& c) { c.data()[4] = 3; c.get()->fVersion = 3; }},
            {"version zero", [&](Chunk& c) { c.data()[4] = 0; c.get()->fVersion = 0; }},
            {"header version differs from fVersion", [&](Chunk& c) { c.get()->fVersion = 1; }},
            {"parameter count too large", [&](Chunk& c) { c.data()[8] = static_cast<unsigned char>(info.param_count + 1); }},
            {"parameter count zero", [&](Chunk& c) { c.data()[8] = 0; }},
            {"layout hash changed", [&](Chunk& c) { c.data()[12] ^= 0x01; }},
            {"bypass not 0 or 1", [&](Chunk& c) { c.data()[16] = 2; }},
            {"NaN value", [&](Chunk& c) { const float v = std::nanf(""); std::memcpy(c.data() + 20, &v, 4); }},
            {"infinite value", [&](Chunk& c) { const float v = INFINITY; std::memcpy(c.data() + 20 + 4 * (info.param_count - 1), &v, 4); }},
            {"value above its range", [&](Chunk& c) { const float v = static_cast<float>(descs[0].max) + 1.0f; std::memcpy(c.data() + 20, &v, 4); }},
            {"value below its range", [&](Chunk& c) { const float v = static_cast<float>(descs[0].min) - 1.0f; std::memcpy(c.data() + 20, &v, 4); }},
        };
        for (const Case& test : cases) {
            Chunk bad(size + 8);
            std::memcpy(bad.storage.data(), saved.storage.data(), sizeof(AAX_SPlugInChunk) + size);
            bad.get()->fChunkID = state::kChunkId;
            test.damage(bad);
            // Start from a distinct state, so "unchanged" is not "still at the defaults".
            Instance target(channels);
            require(proofSetPlain(target.params.get(), gain, static_cast<float>(descs[gain].min)), "set target gain");
            require(proofSetBypass(target.params.get(), true), "set target bypass");
            const PacketBuffer before = target.packet();
            const AAX_Result result = target.params->SetChunk(state::kChunkId, bad.get());
            if (result == AAX_SUCCESS) {
                std::fprintf(stderr, "AAX state proof failed: a %s chunk was accepted\n", test.name);
                return 1;
            }
            if (!same(target.packet(), before)) {
                std::fprintf(stderr, "AAX state proof failed: a %s chunk changed a value\n", test.name);
                return 1;
            }
            AAX_CBoolean cmp = true;
            require(target.params->CompareActiveChunk(bad.get(), &cmp) == AAX_SUCCESS && !cmp, "a refused chunk never compares equal");
        }
        std::printf("AAX state %s: %zu corrupt, truncated, extended and foreign chunks refused, values unchanged\n",
                    channels == 1 ? "mono" : "stereo", cases.size());

        // A chunk that does not belong to this plug-in is not "different" for the compare light.
        Chunk foreign(size);
        std::memcpy(foreign.storage.data(), saved.storage.data(), sizeof(AAX_SPlugInChunk) + size);
        foreign.get()->fChunkID = 'zzzz';
        AAX_CBoolean cmp = false;
        require(fresh.params->CompareActiveChunk(foreign.get(), &cmp) == AAX_SUCCESS && cmp, "an unknown chunk id does not light compare");
    }

    // Master bypass: saved, restored, and the dry signal is copied unchanged.
    for (uint32_t channels = 1; channels <= 2; ++channels) {
        Instance original(channels);
        require(proofSetPlain(original.params.get(), gain, static_cast<float>(std::fmin(descs[gain].max, 6.0))), "set gain");
        require(proofSetBypass(original.params.get(), true), "set bypass");
        Chunk saved = original.save();
        Instance fresh(channels);
        require(fresh.packet().get()->bypass == 0, "a fresh instance is not bypassed");
        require(fresh.params->SetChunk(state::kChunkId, saved.get()) == AAX_SUCCESS, "SetChunk with bypass");
        const PacketBuffer bypassed = fresh.packet();
        require(bypassed.get()->bypass == 1, "bypass restored from the chunk");
        ProcessState state;
        require(state.prepare(48000.0, channels), "prepare");
        std::vector<float> dry, right;
        const auto left = render(state, bypassed, static_cast<int>(channels), 3, &dry, &right);
        require(max_difference(left, dry) == 0.0, "bypass copies the dry left buffer exactly");
        if (channels > 1) require(max_difference(right, dry) == 0.0, "bypass copies the dry right buffer exactly");
        // And with bypass off the same values process.
        PacketBuffer active = bypassed;
        active.get()->bypass = 0;
        ProcessState state2;
        require(state2.prepare(48000.0, channels), "prepare");
        const auto processed = render(state2, active, static_cast<int>(channels), 3, &dry);
        require(max_difference(processed, dry) > 0.01, "without bypass the same values process the signal");
        std::printf("AAX bypass %s: restored from the chunk, copies the dry signal exactly (processed differs by %.3g)\n",
                    channels == 1 ? "mono" : "stereo", max_difference(processed, dry));
    }

    std::puts("AAX state proof: round trip of every parameter and the bypass through the AAX chunk calls, same values and same audio, corrupt chunks refused without changing values. Not a Pro Tools test: PASS");
    return 0;
}
