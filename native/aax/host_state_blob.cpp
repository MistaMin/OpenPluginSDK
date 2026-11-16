// SPDX-License-Identifier: MIT
// Proves the AAX state chunk carries the plugin's own state. The shell's real
// AAX_CEffectParameters runs (no AAX controller) over a stand-in ABI
// (proof_stub.cpp) whose plugin owns a chunk: op_shell_save_state and
// op_shell_load_state carry it, and a non-empty chunk changes the audio.
//   - A version 2 chunk round-trips parameters, bypass and the plugin's chunk,
//     and the restored instance renders the same audio.
//   - A version 1 chunk still loads: parameters and bypass restored, the
//     plugin's chunk left alone.
//   - Corrupt version 2 chunks (truncated or extended, wrong blob length, a
//     damaged blob, a blob the plugin refuses, a changed header) are refused
//     and change no parameter, no bypass and no plugin chunk.
//   - The size protocol: GetChunk never writes more than GetChunkSize promised;
//     a state that grows between the size query and the copy is handled; one
//     that grows past the promise is refused.
// Not a Pro Tools test.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_state_blob_proof --output-on-failure
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
#include <vector>

using namespace openplugin_aax;

extern "C" {
uint32_t op_stub_set_chunk(void* instance, const uint8_t* data, uint32_t length);
uint32_t op_stub_chunk(void* instance, uint8_t* out, uint32_t capacity);
uint32_t op_stub_loads(void* instance);
void op_stub_grow_on_query(void* instance, uint32_t bytes);
uint32_t op_stub_saves(void* instance);
uint32_t op_stub_sets(void* instance);
}

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX state blob proof failed: %s\n", message);
        std::exit(1);
    }
}

static constexpr size_t kGuard = 16;
static constexpr unsigned char kGuardByte = 0xCD;

// A chunk as the host holds it: the header, room for fData, and guard bytes after it.
struct Chunk {
    std::vector<unsigned char> storage;
    size_t room;
    explicit Chunk(size_t dataBytes) : storage(sizeof(AAX_SPlugInChunk) + dataBytes + kGuard, kGuardByte), room(dataBytes) {
        std::memset(storage.data(), 0, sizeof(AAX_SPlugInChunk) + dataBytes);
    }
    AAX_SPlugInChunk* get() { return reinterpret_cast<AAX_SPlugInChunk*>(storage.data()); }
    unsigned char* data() { return reinterpret_cast<unsigned char*>(get()->fData); }
    bool guardIntact() const {
        for (size_t i = 0; i < kGuard; ++i)
            if (storage[sizeof(AAX_SPlugInChunk) + room + i] != kGuardByte) return false;
        return true;
    }
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
    void* engine() const {
        void* engine = nullptr;
        uint32_t written = 0;
        require(params->GetCustomData(kEngineData, sizeof engine, &engine, &written) == AAX_SUCCESS && engine, "engine pointer");
        return engine;
    }
    ProcessState* processState() const {
        ProcessState* state = nullptr;
        require(params->ResetFieldData(stateField, &state, sizeof state) == AAX_SUCCESS && state, "process state");
        return state;
    }
    void setChunk(const std::vector<uint8_t>& bytes) const {
        require(op_stub_set_chunk(engine(), bytes.data(), static_cast<uint32_t>(bytes.size())), "set plugin chunk");
    }
    std::vector<uint8_t> pluginChunk() const {
        const uint32_t size = op_stub_chunk(engine(), nullptr, 0);
        std::vector<uint8_t> out(size);
        if (size) op_stub_chunk(engine(), out.data(), size);
        return out;
    }
    uint32_t loads() const { return op_stub_loads(engine()); }
    uint32_t size() const {
        uint32_t bytes = 0;
        require(params->GetChunkSize(state::kChunkId, &bytes) == AAX_SUCCESS && bytes > 0, "GetChunkSize");
        return bytes;
    }
    // Save through the AAX chunk calls exactly as a host does.
    Chunk save() const {
        const uint32_t bytes = size();
        Chunk chunk(bytes);
        chunk.get()->fChunkID = state::kChunkId;
        require(params->GetChunk(state::kChunkId, chunk.get()) == AAX_SUCCESS, "GetChunk");
        require(static_cast<uint32_t>(chunk.get()->fSize) == bytes, "GetChunk reports the size GetChunkSize promised");
        require(chunk.get()->fVersion == static_cast<int32_t>(state::kVersion), "chunk version is 2");
        require(chunk.guardIntact(), "GetChunk stayed inside the promised size");
        return chunk;
    }
};

static bool same(const PacketBuffer& a, const PacketBuffer& b) {
    if (a.count() != b.count() || a.get()->bypass != b.get()->bypass) return false;
    for (uint32_t i = 0; i < a.count(); ++i)
        if (a.plain()[i] != b.plain()[i]) return false;
    return true;
}

static std::vector<float> render(ProcessState& state, const PacketBuffer& packet, int channels, int blocks, std::vector<float>* rightOut = nullptr) {
    constexpr int kFrames = 512;
    std::vector<float> inL(kFrames), inR(kFrames), outL(kFrames), outR(kFrames), all;
    float* input[] = {inL.data(), channels > 1 ? inR.data() : nullptr};
    float* output[] = {outL.data(), channels > 1 ? outR.data() : nullptr};
    int32_t frames = kFrames;
    auto* statePtr = &state;
    Context context{input, output, &frames, packet.get(), &statePtr};
    Context* contexts[] = {&context};
    for (int block = 0; block < blocks; ++block) {
        fill_sine(inL.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (channels > 1) fill_sine(inR.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        process(contexts, contexts + 1);
        all.insert(all.end(), outL.begin(), outL.end());
    }
    if (rightOut) *rightOut = outR;
    return all;
}

static std::vector<uint8_t> pattern(size_t bytes, uint8_t first) {
    std::vector<uint8_t> out(bytes);
    for (size_t i = 0; i < bytes; ++i) out[i] = static_cast<uint8_t>(first + i * 7);
    return out;
}

static void configure(Instance& instance, float p0, float p1, float p2, bool bypass, const std::vector<uint8_t>& chunk) {
    require(proofSetPlain(instance.params.get(), 0, p0) && proofSetPlain(instance.params.get(), 1, p1) && proofSetPlain(instance.params.get(), 2, p2), "set parameters");
    require(proofSetBypass(instance.params.get(), bypass), "set bypass");
    instance.setChunk(chunk);
}

int main() {
    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.param_count == 3, "the stand-in declares 3 parameters");
    const size_t fixed = state::kHeaderBytes + 3 * sizeof(float);

    // Round trip through both channel layouts.
    for (uint32_t channels = 1; channels <= 2; ++channels) {
        const char* layout = channels == 1 ? "mono" : "stereo";
        const std::vector<uint8_t> chunkBytes = pattern(53, 100);
        Instance original(channels);
        configure(original, 0.8f, 0.125f, 0.375f, true, chunkBytes);
        const PacketBuffer changed = original.packet();
        Chunk saved = original.save();

        // Version 2 layout: header, parameter floats, blob length, blob.
        const unsigned char* d = saved.data();
        require(d[0] == 'O' && d[1] == 'P' && d[2] == 'S' && d[3] == 'T' && d[4] == 2 && d[5] == 0 && d[6] == 0 && d[7] == 0, "magic and payload version 2");
        require(d[8] == 3 && d[16] == 1, "parameter count 3 and bypass 1");
        const size_t blobLength = d[fixed] | d[fixed + 1] << 8 | d[fixed + 2] << 16 | static_cast<size_t>(d[fixed + 3]) << 24;
        require(blobLength == 12 + 4 * 3 + chunkBytes.size(), "blob length is the stub blob: 12 + 4 per parameter + chunk");
        require(static_cast<size_t>(saved.get()->fSize) == fixed + 4 + blobLength, "chunk ends with the blob");
        require(std::memcmp(d + fixed + 4 + blobLength - chunkBytes.size(), chunkBytes.data(), chunkBytes.size()) == 0, "the plugin chunk is the tail of the blob");

        Instance fresh(channels);
        AAX_CBoolean equal = true;
        require(fresh.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && !equal, "a fresh instance differs from the saved chunk");
        require(fresh.loads() == 0 && fresh.pluginChunk().empty(), "a fresh instance has no plugin chunk");
        require(fresh.params->SetChunk(state::kChunkId, saved.get()) == AAX_SUCCESS, "SetChunk");
        require(fresh.loads() == 1, "the blob went through op_shell_load_state once");
        require(same(fresh.packet(), changed), "parameters and bypass restored");
        require(fresh.pluginChunk() == chunkBytes, "the plugin chunk is restored byte for byte");
        require(fresh.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && equal, "the restored instance compares equal");
        require(original.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && equal, "the original compares equal to its own chunk");
        Chunk again = fresh.save();
        require(again.get()->fSize == saved.get()->fSize && std::memcmp(again.data(), saved.data(), static_cast<size_t>(saved.get()->fSize)) == 0,
                "saving the restored instance gives the same bytes");

        // Compare notices a change of the plugin's chunk alone, and of one parameter alone.
        {
            Instance other(channels);
            configure(other, 0.8f, 0.125f, 0.375f, true, pattern(53, 101));
            require(other.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && !equal, "a different plugin chunk does not compare equal");
            configure(other, 0.8f, 0.126f, 0.375f, true, chunkBytes);
            require(other.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && !equal, "a different parameter does not compare equal");
        }

        // Identical audio, through the bypass-off path too (the chunk scales the output).
        PacketBuffer active = changed;
        active.get()->bypass = 0;
        std::vector<float> rightA, rightB;
        const auto audioA = render(*original.processState(), active, static_cast<int>(channels), 6, &rightA);
        const auto audioB = render(*fresh.processState(), active, static_cast<int>(channels), 6, &rightB);
        require(max_difference(audioA, audioB) == 0.0, "restored instance gives the same audio");
        if (channels > 1) require(max_difference(rightA, rightB) == 0.0, "restored instance gives the same right channel");
        Instance nochunk(channels);
        configure(nochunk, 0.8f, 0.125f, 0.375f, false, {});
        const auto audioPlain = render(*nochunk.processState(), active, static_cast<int>(channels), 6);
        require(max_difference(audioA, audioPlain) > 0.01, "the plugin chunk really changes the audio");
        std::printf("AAX state blob %s: version 2 chunk of %d bytes restores parameters, bypass and a %zu-byte plugin chunk; audio difference %.3g; chunk vs none %.3g\n",
                    layout, saved.get()->fSize, chunkBytes.size(), max_difference(audioA, audioB), max_difference(audioA, audioPlain));
    }

    const std::vector<uint8_t> chunkBytes = pattern(53, 100);
    Instance source(2);
    configure(source, 0.8f, 0.125f, 0.375f, true, chunkBytes);
    const PacketBuffer sourceValues = source.packet();
    Chunk saved = source.save();
    const size_t size = static_cast<size_t>(saved.get()->fSize);
    const size_t blobStart = fixed + 4;

    // Version 1 still loads: values and bypass, plugin chunk untouched.
    {
        state::Values v1{};
        v1.count = 3;
        v1.bypass = true;
        for (uint32_t i = 0; i < 3; ++i) v1.plain[i] = sourceValues.plain()[i];
        Chunk old(state::encodedSizeV1());
        old.get()->fChunkID = state::kChunkId;
        old.get()->fVersion = 1;
        old.get()->fSize = static_cast<int32_t>(state::encodedSizeV1());
        require(state::encodeV1(v1, old.data(), state::encodedSizeV1()), "encode version 1");
        require(old.data()[4] == 1 && old.get()->fSize == 20 + 12, "version 1 layout is unchanged: 20-byte header and one float per parameter");
        Instance target(2);
        const std::vector<uint8_t> own = pattern(9, 3);
        configure(target, 0.1f, 0.2f, 0.3f, false, own);
        require(target.params->SetChunk(state::kChunkId, old.get()) == AAX_SUCCESS, "a version 1 chunk loads");
        require(same(target.packet(), sourceValues), "version 1 restores the parameters and the bypass");
        require(target.pluginChunk() == own && target.loads() == 0, "version 1 leaves the plugin's chunk alone");
        AAX_CBoolean equal = false;
        require(target.params->CompareActiveChunk(old.get(), &equal) == AAX_SUCCESS && equal, "a version 1 chunk compares equal on values");
        // The payload version and the header version must agree, in both directions.
        old.get()->fVersion = 2;
        require(target.params->SetChunk(state::kChunkId, old.get()) != AAX_SUCCESS, "a version 1 payload under header version 2 is refused");
        std::puts("AAX state blob: a version 1 chunk loads parameters and bypass and leaves the plugin chunk alone");
    }

    // Corrupt version 2 chunks.
    struct Case {
        const char* name;
        std::function<void(Chunk&)> damage;
    };
    const auto le32 = [](unsigned char* at, uint32_t v) { for (int i = 0; i < 4; ++i) at[i] = static_cast<unsigned char>(v >> (8 * i)); };
    const auto blobAt = [&](Chunk& c, size_t offset) { return c.data() + blobStart + offset; };
    const std::vector<Case> cases = {
        {"empty", [&](Chunk& c) { c.get()->fSize = 0; }},
        {"shorter than the header", [&](Chunk& c) { c.get()->fSize = 12; }},
        {"negative size", [&](Chunk& c) { c.get()->fSize = -4; }},
        {"header only", [&](Chunk& c) { c.get()->fSize = 20; }},
        {"floats only, no blob length", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(fixed); }},
        {"blob length cut in two", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(fixed + 2); }},
        {"blob length but no blob", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(blobStart); }},
        {"truncated by one byte", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size - 1); }},
        {"truncated by the plugin chunk", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size - 53); }},
        {"truncated to one blob byte", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(blobStart + 1); }},
        {"extended by one byte", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size + 1); }},
        {"extended by a float", [&](Chunk& c) { c.get()->fSize = static_cast<int32_t>(size + 4); }},
        {"blob length one too small", [&](Chunk& c) { c.data()[fixed] -= 1; }},
        {"blob length one too large", [&](Chunk& c) { c.data()[fixed] += 1; }},
        {"blob length huge", [&](Chunk& c) { le32(c.data() + fixed, 0xffffffffu); }},
        {"blob length zero", [&](Chunk& c) { le32(c.data() + fixed, 0); }},
        {"blob length zero and no blob", [&](Chunk& c) { le32(c.data() + fixed, 0); c.get()->fSize = static_cast<int32_t>(blobStart); }},
        {"blob magic damaged", [&](Chunk& c) { blobAt(c, 0)[0] ^= 0xff; }},
        {"blob parameter count wrong", [&](Chunk& c) { blobAt(c, 4)[0] = 4; }},
        {"blob value NaN", [&](Chunk& c) { const float v = std::nanf(""); std::memcpy(blobAt(c, 8), &v, 4); }},
        {"blob value out of range", [&](Chunk& c) { const float v = 2.0f; std::memcpy(blobAt(c, 8), &v, 4); }},
        {"blob chunk length lies", [&](Chunk& c) { blobAt(c, 8 + 12)[0] += 1; }},
        {"blob refused by the plugin", [&](Chunk& c) { blobAt(c, 12 + 12)[0] = 0xFF; }},
        {"blob overwritten", [&](Chunk& c) { std::memset(blobAt(c, 0), 0xA5, 16); }},
        {"wrong magic", [&](Chunk& c) { c.data()[0] ^= 0xff; }},
        {"newer payload version", [&](Chunk& c) { c.data()[4] = 3; c.get()->fVersion = 3; }},
        {"version zero", [&](Chunk& c) { c.data()[4] = 0; c.get()->fVersion = 0; }},
        {"header version differs from fVersion", [&](Chunk& c) { c.get()->fVersion = 1; }},
        {"payload version 1 over a version 2 body", [&](Chunk& c) { c.data()[4] = 1; c.get()->fVersion = 1; }},
        {"parameter count wrong", [&](Chunk& c) { c.data()[8] = 4; }},
        {"layout hash changed", [&](Chunk& c) { c.data()[12] ^= 0x01; }},
        {"bypass not 0 or 1", [&](Chunk& c) { c.data()[16] = 2; }},
        {"parameter NaN", [&](Chunk& c) { const float v = std::nanf(""); std::memcpy(c.data() + 20, &v, 4); }},
        {"parameter out of range", [&](Chunk& c) { const float v = 1.5f; std::memcpy(c.data() + 24, &v, 4); }},
    };
    const std::vector<uint8_t> targetChunk = pattern(31, 40);
    for (const Case& test : cases) {
        Chunk bad(size + 8);
        std::memcpy(bad.storage.data(), saved.storage.data(), sizeof(AAX_SPlugInChunk) + size);
        bad.get()->fChunkID = state::kChunkId;
        test.damage(bad);
        // A distinct starting state, so "unchanged" is not "still at the defaults".
        Instance target(2);
        configure(target, 0.1f, 0.2f, 0.3f, false, targetChunk);
        const PacketBuffer before = target.packet();
        const uint32_t loadsBefore = target.loads();
        const AAX_Result result = target.params->SetChunk(state::kChunkId, bad.get());
        if (result == AAX_SUCCESS) {
            std::fprintf(stderr, "AAX state blob proof failed: a %s chunk was accepted\n", test.name);
            return 1;
        }
        if (!same(target.packet(), before) || target.pluginChunk() != targetChunk || target.loads() != loadsBefore) {
            std::fprintf(stderr, "AAX state blob proof failed: a %s chunk changed the parameters, the bypass or the plugin chunk\n", test.name);
            return 1;
        }
        AAX_CBoolean cmp = true;
        require(target.params->CompareActiveChunk(bad.get(), &cmp) == AAX_SUCCESS && !cmp, "a refused chunk never compares equal");
    }
    std::printf("AAX state blob: %zu corrupt version 2 chunks refused; parameters, bypass and plugin chunk unchanged\n", cases.size());

    // The size protocol.
    {
        Instance a(2);
        configure(a, 0.8f, 0.125f, 0.375f, false, pattern(40, 9));
        // GetChunk without a size answer first promises nothing and writes nothing.
        {
            Chunk none(256);
            none.get()->fChunkID = state::kChunkId;
            require(a.params->GetChunk(state::kChunkId, none.get()) != AAX_SUCCESS, "GetChunk before GetChunkSize is refused");
            require(none.guardIntact(), "and wrote nothing");
        }
        // A state that grew since the size answer is refused, never written past the promise.
        const uint32_t promised = a.size();
        a.setChunk(pattern(48, 9));
        Chunk grown(promised);
        grown.get()->fChunkID = state::kChunkId;
        require(a.params->GetChunk(state::kChunkId, grown.get()) != AAX_SUCCESS, "a state larger than the promised size is refused");
        require(grown.guardIntact(), "and nothing was written past the promised size");
        // The host asks again and gets the new size.
        require(a.size() == promised + 8, "a new size answer follows the state");
        Chunk ok = a.save();
        require(static_cast<uint32_t>(ok.get()->fSize) == promised + 8, "saved at the new size");
        // A state that shrank since the answer: the chunk is shorter and still loads.
        a.setChunk(pattern(40, 9));
        Chunk shorter(promised + 8);
        shorter.get()->fChunkID = state::kChunkId;
        require(a.params->GetChunk(state::kChunkId, shorter.get()) == AAX_SUCCESS && shorter.get()->fSize == static_cast<int32_t>(promised), "a smaller state fits the promise and reports its real size");
        require(shorter.guardIntact(), "inside the promise");
        Instance b(2);
        require(b.params->SetChunk(state::kChunkId, shorter.get()) == AAX_SUCCESS && b.pluginChunk() == pattern(40, 9), "and that chunk restores");
        // A state larger than the last save is saved again at the size the engine asks for,
        // and growth between that answer and the copy is retried.
        a.setChunk(pattern(40, 9));
        const uint32_t reserved = a.size();
        a.setChunk(pattern(24, 9));
        AAX_CBoolean ignored = false;
        require(a.params->CompareActiveChunk(shorter.get(), &ignored) == AAX_SUCCESS, "a save at 24 bytes");
        a.setChunk(pattern(32, 9));  // larger than the last save; the too-small answer grows the state back to 40
        op_stub_grow_on_query(a.engine(), 8);
        Chunk retried(reserved);
        retried.get()->fChunkID = state::kChunkId;
        const uint32_t before = op_stub_saves(a.engine());
        require(a.params->GetChunk(state::kChunkId, retried.get()) == AAX_SUCCESS, "state growing between query and copy is retried");
        require(op_stub_saves(a.engine()) - before == 3, "two too-small answers, then the save");
        require(retried.guardIntact() && static_cast<uint32_t>(retried.get()->fSize) == reserved, "and the chunk holds the grown state");
        Instance c(2);
        require(c.params->SetChunk(state::kChunkId, retried.get()) == AAX_SUCCESS && c.pluginChunk().size() == 40, "which restores");
    }
    std::puts("AAX state blob: GetChunk never writes more than GetChunkSize promised, retries a state that grew mid-save, and refuses one that outgrew the promise");

    // One save per call, and a compare that leaves the engine alone.
    {
        Instance a(2);
        configure(a, 0.8f, 0.125f, 0.375f, true, pattern(53, 100));
        void* engine = a.engine();
        uint32_t saves = op_stub_saves(engine), sets = op_stub_sets(engine);
        // Each count since the last call of this.
        auto since = [&](uint32_t& mark, uint32_t now) {
            const uint32_t delta = now - mark;
            mark = now;
            return delta;
        };
        const uint32_t bytes = a.size();
        require(since(saves, op_stub_saves(engine)) == 1, "GetChunkSize is one save");
        Chunk saved(bytes);
        saved.get()->fChunkID = state::kChunkId;
        require(a.params->GetChunk(state::kChunkId, saved.get()) == AAX_SUCCESS, "GetChunk");
        require(since(saves, op_stub_saves(engine)) == 1, "GetChunk is one save");
        since(sets, op_stub_sets(engine));
        AAX_CBoolean equal = false;
        for (int n = 0; n < 5; ++n) require(a.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && equal, "compares equal");
        require(since(saves, op_stub_saves(engine)) == 5, "CompareActiveChunk is one save when the values are equal");
        require(proofSetPlain(a.params.get(), 1, 0.5f), "set parameter 1");
        require(a.params->CompareActiveChunk(saved.get(), &equal) == AAX_SUCCESS && !equal, "a different value compares unequal");
        require(since(saves, op_stub_saves(engine)) == 0, "and costs no save");
        require(since(sets, op_stub_sets(engine)) == 0 && op_shell_get_plain(engine, 1) == 0.125f, "CompareActiveChunk never writes to the engine");
        std::puts("AAX state blob: GetChunkSize, GetChunk and an equal CompareActiveChunk each run one op_shell_save_state; a compare with a different value runs none; compare writes nothing to the engine");
    }

    // A large plugin chunk.
    {
        const std::vector<uint8_t> big = pattern(300000, 5);
        Instance a(2);
        configure(a, 0.6f, 0.4f, 0.2f, false, big);
        Chunk saved = a.save();
        Instance b(2);
        require(b.params->SetChunk(state::kChunkId, saved.get()) == AAX_SUCCESS && b.pluginChunk() == big, "a 300000-byte plugin chunk round-trips");
        std::printf("AAX state blob: a 300000-byte plugin chunk round-trips in a %d-byte AAX chunk\n", saved.get()->fSize);
    }

    std::puts("AAX state blob proof: version 2 round trip of parameters, bypass and plugin chunk with identical audio, version 1 still loads, corrupt chunks refused unchanged, size protocol kept. Not a Pro Tools test: PASS");
    return 0;
}
