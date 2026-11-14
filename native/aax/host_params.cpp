// SPDX-License-Identifier: MIT
// Proves the AAX shell carries more than 64 parameters. It runs the shell's real
// AAX_CEffectParameters (no AAX controller) and process callback over a stand-in
// ABI (proof_stub.cpp) that declares 200 parameters and reports 37 samples of
// latency. Not a Pro Tools test: Pro Tools itself does not take part, so what it
// accepts for a data port of this size is not shown here.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_params_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "OpenPluginAAXState.h"
#include "AAX_CEffectParameters.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <vector>
using namespace openplugin_aax;

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX parameter-count proof failed: %s\n", message);
        std::exit(1);
    }
}

static std::vector<float> render(ProcessState& state, const PacketBuffer& packet, uint32_t channels, const std::vector<float>& in,
                                 unsigned block) {
    std::vector<float> a(in), b(in.size(), 0.f), a2(in), b2(in.size(), 0.f), out;
    for (size_t at = 0; at < in.size(); at += block) {
        const auto frames = static_cast<int32_t>(std::min<size_t>(block, in.size() - at));
        float* input[2] = {a.data() + at, channels > 1 ? a2.data() + at : nullptr};
        float* output[2] = {b.data() + at, channels > 1 ? b2.data() + at : nullptr};
        auto* statePtr = &state;
        Context context{input, output, &frames, packet.get(), &statePtr};
        Context* contexts[] = {&context};
        process(contexts, contexts + 1);
    }
    return b;
}

int main() {
    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.abi == OP_SHELL_ABI, "shell info");
    const uint32_t n = info.param_count;
    require(n == 200, "the stand-in declares 200 parameters");
    require(n > OP_SHELL_AAX_MAX_PARAMS, "more parameters than the old 64-value packet");
    require(n <= kMaxParams, "the shell takes them");
    std::vector<OpParamDesc> descs(n);
    for (uint32_t i = 0; i < n; ++i) require(op_shell_param(i, &descs[i]) != 0, "shell param");

    for (uint32_t channels = 1; channels <= 2; ++channels) {
        const char* layout = channels == 1 ? "mono" : "stereo";
        std::unique_ptr<AAX_CEffectParameters> params(createParameters());
        require(params && proofInit(params.get(), 48000.0, channels), "200 parameters build without a controller");

        // The packet holds every parameter: its size is the header plus one float each.
        PacketBuffer packet;
        require(proofPacket(params.get(), &packet) && packet.count() == n, "packet carries every parameter");
        require(packet.bytes() == packetBytes(n) && packet.bytes() == 8 + 4 * 200, "packet size follows the parameter count");
        for (uint32_t i = 0; i < n; ++i)
            require(std::fabs(packet.plain()[i] - static_cast<float>(descs[i].default_plain)) < 1e-6f, "every default reaches the packet");

        // The initial packet the host asks for (ResetFieldData) is the same size and the defaults.
        std::vector<uint32_t> initial(packetBytes(n) / 4, 0xDEADBEEFu);
        require(params->ResetFieldData(controlsField, initial.data(), static_cast<uint32_t>(packetBytes(n))) == AAX_SUCCESS, "initial packet");
        const auto* header = reinterpret_cast<const Packet*>(initial.data());
        require(header->count == n && header->bypass == 0, "initial packet header");
        for (uint32_t i = 0; i < n; ++i) require(header->plain()[i] == packet.plain()[i], "initial packet holds the defaults");
        require(params->ResetFieldData(controlsField, initial.data(), static_cast<uint32_t>(sizeof(Packet) + 64 * sizeof(float))) != AAX_SUCCESS,
                "a 64-value packet size is refused for 200 parameters");

        // Parameter updates through the host entry point post one packet, once, with every value.
        PacketBuffer posted;
        require(proofTakePacket(params.get(), &posted), "the first take posts the initial packet");
        require(!proofTakePacket(params.get(), &posted), "nothing changed, nothing posted");
        std::vector<float> wanted(n);
        for (uint32_t i = 0; i < n; ++i) {
            const double normalized = static_cast<double>((i * 37u) % 200u) / 200.0;  // distinct, inside 0..1
            wanted[i] = static_cast<float>(normalized);
            require(proofUpdate(params.get(), i, normalized), "host parameter update");
        }
        require(proofTakePacket(params.get(), &posted) && posted.count() == n && posted.bytes() == packetBytes(n), "one packet after the updates");
        for (uint32_t i = 0; i < n; ++i) require(std::fabs(posted.plain()[i] - wanted[i]) < 1e-6f, "every updated value is in the packet, including those past 64");
        require(!proofTakePacket(params.get(), &posted), "posted once");
        require(proofUpdate(params.get(), kProofBypass, 1.0) && proofTakePacket(params.get(), &posted) && posted.get()->bypass == 1, "bypass change posts a packet");

        // The audio thread sees all of them.
        ProcessState state;
        require(state.prepare(48000.0, channels), "prepare");
        require(state.paramCount == n, "process state knows 200 parameters");
        PacketBuffer active = posted;
        active.get()->bypass = 0;
        std::vector<float> in(3000);
        for (size_t i = 0; i < in.size(); ++i) in[i] = 0.25f * std::sin(0.05f * static_cast<float>(i));
        const auto out = render(state, active, channels, in, 333);
        for (uint32_t i = 0; i < n; ++i)
            require(std::fabs(op_shell_get_plain(state.engine, i) - wanted[i]) < 1e-6f, "every value, including those past 64, reaches the engine");
        const float gain = wanted[0] + 0.5f * wanted[n - 1];
        double worst = 0.0;
        for (size_t i = 0; i < in.size(); ++i) worst = std::fmax(worst, std::fabs(out[i] - in[i] * gain));
        require(worst < 1e-6, "audio uses the first and the last of the 200 values");
        require(std::fabs(gain - wanted[0]) > 0.01f, "the last parameter matters to the audio");

        // The state chunk holds all 200 and the bypass; a fresh instance restores them.
        int32_t chunks = 0;
        require(params->GetNumberOfChunks(&chunks) == AAX_SUCCESS && chunks == 1, "one chunk");
        uint32_t size = 0;
        require(params->GetChunkSize(state::kChunkId, &size) == AAX_SUCCESS && size == state::encodedSize(12 + 4 * n), "chunk is header, 200 floats and the stub blob (12 + 4 * 200 bytes)");
        std::vector<unsigned char> storage(sizeof(AAX_SPlugInChunk) + size + 8, 0);
        auto* chunk = reinterpret_cast<AAX_SPlugInChunk*>(storage.data());
        chunk->fChunkID = state::kChunkId;
        require(params->GetChunk(state::kChunkId, chunk) == AAX_SUCCESS && chunk->fSize == static_cast<int32_t>(size), "GetChunk");
        std::unique_ptr<AAX_CEffectParameters> fresh(createParameters());
        require(proofInit(fresh.get(), 48000.0, channels), "fresh instance");
        require(fresh->SetChunk(state::kChunkId, chunk) == AAX_SUCCESS, "SetChunk with 200 values");
        PacketBuffer restored;
        require(proofPacket(fresh.get(), &restored) && restored.count() == n && restored.get()->bypass == 1, "restored count and bypass");
        for (uint32_t i = 0; i < n; ++i) require(restored.plain()[i] == posted.plain()[i], "restored values are bit-identical");
        require(proofTakePacket(fresh.get(), &posted), "a loaded chunk posts a packet");
        // A chunk for another layout is refused: here, the count field says 64.
        auto bad = storage;
        auto* badChunk = reinterpret_cast<AAX_SPlugInChunk*>(bad.data());
        reinterpret_cast<unsigned char*>(badChunk->fData)[8] = 64;
        require(fresh->SetChunk(state::kChunkId, badChunk) != AAX_SUCCESS, "a chunk for 64 parameters is refused");
        auto truncated = *chunk;
        truncated.fSize = static_cast<int32_t>(size - 4);
        require(fresh->SetChunk(state::kChunkId, &truncated) != AAX_SUCCESS, "a truncated chunk is refused");

        // Restored bypass delays by the reported latency (37 samples) once the fade is done.
        ProcessState bypassState;
        require(bypassState.prepare(48000.0, channels), "prepare");
        const auto delayed = render(bypassState, restored, channels, in, 777);
        for (size_t i = 0; i < in.size(); ++i) {
            const float want = i >= 37 ? in[i - 37] : 0.f;
            require(delayed[i] == want, "restored bypass outputs the input delayed by the latency");
        }
        std::printf("AAX %s, 200 parameters: packet %zu bytes, all values reach the engine, chunk %u bytes round-trips with bypass, bypass delays by %u samples\n",
                    layout, packetBytes(n), size, bypassState.latency);

        // A refused packet: count larger than the parameters is silence, never an over-read.
        PacketBuffer oversized = active;
        oversized.get()->count = n + 1;
        for (float v : render(state, oversized, channels, in, 512)) require(v == 0.f, "oversized packet writes silence");
    }
    std::puts("AAX parameter-count proof: 200 parameters in one packet (8 + 4 * 200 bytes), posted once per change, applied by the process callback, saved and restored by the chunk. Not a Pro Tools test: PASS");
    return 0;
}
