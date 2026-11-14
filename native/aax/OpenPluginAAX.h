// SPDX-License-Identifier: MIT
#pragma once
#include "AAX.h"
#include "op_shell.h"
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <vector>
class AAX_CEffectParameters;
class AAX_IEffectGUI;
namespace openplugin_aax {
// The most parameters this shell takes. It is the shell's own bound (it sizes the
// state chunk's scratch tables), not an AAX one: the AAX SDK sets no limit on the
// number of parameters or on a data port's packet size for a native plug-in.
inline constexpr uint32_t kMaxParams = 4096;
inline constexpr uint32_t kMaxBlock = 2048;
// Length of the switch between processed and bypassed audio.
inline constexpr double kBypassFadeSeconds = 0.010;
// GetCustomData block the view reads: the engine pointer, valid in this process.
inline constexpr AAX_CTypeID kEngineData = 'OpEn';
// The controls packet is a header followed by `count` plain values, so its size
// follows the plug-in's parameter count (packetBytes). The data port is declared
// with that size in Describe and the packet is posted by the shell itself.
struct Packet {
    uint32_t count;
    uint32_t bypass;
    float* plain() { return reinterpret_cast<float*>(this + 1); }
    const float* plain() const { return reinterpret_cast<const float*>(this + 1); }
};
static_assert(sizeof(Packet) == 8 && alignof(Packet) == 4, "packet header layout");
inline constexpr size_t packetBytes(uint32_t count) { return sizeof(Packet) + static_cast<size_t>(count) * sizeof(float); }
// Owning storage for one packet, used by the shell and the proofs. The audio
// thread only ever reads a packet; it never builds one.
struct PacketBuffer {
    std::vector<uint32_t> words;
    explicit PacketBuffer(uint32_t count = 0) : words(packetBytes(count) / sizeof(uint32_t), 0) { get()->count = count; }
    Packet* get() { return reinterpret_cast<Packet*>(words.data()); }
    const Packet* get() const { return reinterpret_cast<const Packet*>(words.data()); }
    uint32_t count() const { return get()->count; }
    size_t bytes() const { return words.size() * sizeof(uint32_t); }
    float* plain() { return get()->plain(); }
    const float* plain() const { return get()->plain(); }
};
struct ProcessState {
    void* engine = nullptr;
    uint32_t channels = 2;
    uint32_t sampleRate = 48000;
    uint32_t paramCount = 0;
    std::atomic<bool> prepared{false};
    // Bypass: the input delayed by the reported latency, crossfaded with the
    // engine output. All of it is allocated in prepare(); process() only reads
    // and writes it.
    uint32_t latency = 0;
    std::vector<float> dry[2];
    std::vector<float> line[2];
    size_t linePos = 0;
    float mix = 0.f;
    float mixStep = 1.f;
    bool mixPending = true;
    ProcessState() = default;
    ProcessState(const ProcessState&) = delete;
    ProcessState& operator=(const ProcessState&) = delete;
    ~ProcessState();
    bool prepare(double sampleRate, uint32_t channelCount);
    // Engine reset plus an empty delay line; the next block starts the bypass mix at its target.
    void reset();
};
struct Context {
    float** input;
    float** output;
    const int32_t* frames;
    const Packet* controls;
    ProcessState** state;
};
enum Field : AAX_CFieldIndex {
    inputField = AAX_FIELD_INDEX(Context, input),
    outputField = AAX_FIELD_INDEX(Context, output),
    framesField = AAX_FIELD_INDEX(Context, frames),
    controlsField = AAX_FIELD_INDEX(Context, controls),
    stateField = AAX_FIELD_INDEX(Context, state)
};
void AAX_CALLBACK process(Context* const begin[], const void* end);
AAX_CEffectParameters* AAX_CALLBACK createParameters();
AAX_IEffectGUI* AAX_CALLBACK createGUI();
// Hooks for the proofs, which have no AAX controller. They act on an object from
// createParameters(). proofInit does what EffectInit does without the controller.
bool proofInit(AAX_CEffectParameters* parameters, double rate, uint32_t channels);
bool proofSetPlain(AAX_CEffectParameters* parameters, uint32_t index, float value);
bool proofSetBypass(AAX_CEffectParameters* parameters, bool on);
bool proofPacket(AAX_CEffectParameters* parameters, PacketBuffer* out);
// What GenerateCoefficients would post: false when no value or bypass changed since the last take.
bool proofTakePacket(AAX_CEffectParameters* parameters, PacketBuffer* out);
// The host's parameter update entry (normalized value), without a controller.
// kProofBypass as the index addresses the master bypass.
inline constexpr uint32_t kProofBypass = 0xFFFFFFFFu;
bool proofUpdate(AAX_CEffectParameters* parameters, uint32_t index, double normalized);
}
