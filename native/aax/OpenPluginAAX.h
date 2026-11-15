// SPDX-License-Identifier: MIT
#pragma once
#include "AAX.h"
#include "AAX_IMIDINode.h"
#include "AAX_ITransport.h"
#include "op_shell.h"
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>
class AAX_CEffectParameters;
class AAX_IController;
class AAX_IEffectGUI;
namespace openplugin_aax {
// The most parameters this shell takes. It is the shell's own bound (it sizes the
// state chunk's scratch tables), not an AAX one: the AAX SDK sets no limit on the
// number of parameters or on a data port's packet size for a native plug-in.
inline constexpr uint32_t kMaxParams = 4096;
inline constexpr uint32_t kMaxBlock = 2048;
// Most channels one AAX bus carries here (the widest stem the shell maps is 9.1.6, 16).
inline constexpr uint32_t kMaxChannels = 16;
// The library must be at least as new as this header (docs/SHELL.md, Versioning).
inline bool abiOk() { return op_shell_abi_version() >= static_cast<uint32_t>(OP_SHELL_ABI); }
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
// One AAX stem format and what each of its channels is, in AAX (Pro Tools film)
// order. roles[i] is the CLAP surround id of channel i, which is the SDK's
// canonical channel role (OP_ROLE_NONE for mono and ambisonic). The order comes
// from the comments on AAX_EStemFormat in AAX_Enums.h.
struct Stem {
    AAX_EStemFormat format;
    uint32_t kind;      // OP_BUS_*
    uint32_t channels;
    uint8_t roles[kMaxChannels];
    const char* name;
};
const Stem* findStem(AAX_EStemFormat format);
// The stem that carries `bus` (same kind, count and set of roles); null if none.
const Stem* stemForBus(const OpBus& bus);
// map[k] = the AAX channel that holds canonical channel k of `bus`. False if the
// bus does not fit the stem.
bool channelMap(const OpBus& bus, const Stem& stem, uint8_t* map);
// One component the shell describes, and how the engine is set up for it.
struct Config {
    AAX_EStemFormat input = AAX_eStemFormat_None;
    AAX_EStemFormat output = AAX_eStemFormat_None;
    int32_t layout = -1;        // op_shell_set_layout index; -1: the channel-count engine of ABI 1 (mono or stereo)
    bool sidechain = false;     // the layout has a sidechain bus
    uint32_t pluginId = 0;
};
// Every component for the plugin's layouts, from op_shell_layout*. Empty when no layout has an AAX stem.
std::vector<Config> buildConfigs(const OpShellInfo& info);
struct ProcessState {
    void* engine = nullptr;
    uint32_t channels = 2;     // AAX output channels
    uint32_t sampleRate = 48000;
    uint32_t paramCount = 0;
    std::atomic<bool> prepared{false};
    // One bus as the shell wires it: AAX channel count, engine channel count, and
    // map[k], the AAX channel of the engine's canonical channel k.
    struct Bus {
        uint32_t aax = 0;
        uint32_t engine = 0;
        uint8_t map[kMaxChannels] = {};
    };
    Bus in, out;
    bool sidechain = false;    // the selected layout has a sidechain bus
    bool noteOnly = false;     // the layout has no audio output: audio passes through, MIDI is the point
    // The plugin's note ports (op_shell_note_ports), so process() reads the MIDI node fields only
    // when Describe registered them: AAX leaves an unregistered context field undefined, not null.
    uint32_t notePorts = 0;
    // Bypass: the input delayed by the reported latency, crossfaded with the
    // engine output. All of it is allocated in prepare(); process() only reads
    // and writes it.
    uint32_t latency = 0;
    std::vector<std::vector<float>> dry;
    std::vector<std::vector<float>> line;
    size_t linePos = 0;
    float mix = 0.f;
    float mixStep = 1.f;
    bool mixPending = true;
    // Block scratch, sized in prepare(): the events of the whole AAX buffer and of one piece of it.
    std::vector<OpEvent> events;
    std::vector<OpEvent> pieceEvents;
    ProcessState() = default;
    ProcessState(const ProcessState&) = delete;
    ProcessState& operator=(const ProcessState&) = delete;
    ~ProcessState();
    // Mono or stereo in and out, the same channel count on both sides.
    bool prepare(double sampleRate, uint32_t channelCount);
    // The component whose stems are `input` and `output` (Describe built it from buildConfigs).
    bool prepareStems(double sampleRate, AAX_EStemFormat input, AAX_EStemFormat output);
    // Engine reset plus an empty delay line; the next block starts the bypass mix at its target.
    void reset();
};
// The algorithm context. The first five fields are always described. The last
// four are described when the plugin needs them, and are null otherwise:
// midiIn and midiOut when op_shell_note_ports() says so, transport always (AAX
// gives the transport through a transport MIDI node), sideChain when a layout
// has a sidechain bus.
struct Context {
    float** input;
    float** output;
    const int32_t* frames;
    const Packet* controls;
    ProcessState** state;
    AAX_IMIDINode* midiIn;
    AAX_IMIDINode* midiOut;
    AAX_IMIDINode* transport;
    const int32_t* sideChain;
};
enum Field : AAX_CFieldIndex {
    inputField = AAX_FIELD_INDEX(Context, input),
    outputField = AAX_FIELD_INDEX(Context, output),
    framesField = AAX_FIELD_INDEX(Context, frames),
    controlsField = AAX_FIELD_INDEX(Context, controls),
    stateField = AAX_FIELD_INDEX(Context, state),
    midiInField = AAX_FIELD_INDEX(Context, midiIn),
    midiOutField = AAX_FIELD_INDEX(Context, midiOut),
    transportField = AAX_FIELD_INDEX(Context, transport),
    sideChainField = AAX_FIELD_INDEX(Context, sideChain)
};
// AAX ticks per quarter note (AAX_ITransport::GetCurrentTickPosition).
inline constexpr double kTicksPerQuarter = 960000.0;
// Reads the transport node with the version 1 AAX_ITransport methods only (the node's
// interface is not versioned). False when there is no node or it gave nothing usable.
bool readTransport(AAX_IMIDINode* node, double sampleRate, OpTransport* out);
// Converts one AAX MIDI packet to a shell event. False when the packet is not one the ABI carries
// (SysEx, a data byte over 127, an empty packet).
bool midiToEvent(const AAX_CMidiPacket& packet, uint32_t frames, OpEvent* out);
// The packet for one shell MIDI output event, with its length from the status byte.
AAX_CMidiPacket midiFromEvent(const OpMidiEvent& event, uint32_t offset);
void AAX_CALLBACK process(Context* const begin[], const void* end);
AAX_CEffectParameters* AAX_CALLBACK createParameters();
AAX_IEffectGUI* AAX_CALLBACK createGUI();
// Hooks for the proofs, which have no AAX controller. They act on an object from
// createParameters(). proofInit does what EffectInit does without the controller.
bool proofInit(AAX_CEffectParameters* parameters, double rate, uint32_t channels);
bool proofInitStems(AAX_CEffectParameters* parameters, double rate, AAX_EStemFormat input, AAX_EStemFormat output);
// The offline notifications the host sends the data model (AAX_eNotificationEvent_Entering/ExitingOfflineMode).
bool proofNotify(AAX_CEffectParameters* parameters, AAX_CTypeID event);
// The pointer to the engine the process state holds (what GetCustomData(kEngineData) returns).
void* proofEngine(AAX_CEffectParameters* parameters);
bool proofSetPlain(AAX_CEffectParameters* parameters, uint32_t index, float value);
bool proofSetBypass(AAX_CEffectParameters* parameters, bool on);
bool proofPacket(AAX_CEffectParameters* parameters, PacketBuffer* out);
// What GenerateCoefficients would post: false when no value or bypass changed since the last take.
bool proofTakePacket(AAX_CEffectParameters* parameters, PacketBuffer* out);
// Parameter text. shellText is the plugin's own text for a plain value
// (op_shell_param_format). fitText shortens it for a host that asks for at most
// maxChars bytes (see OpenPluginAAXParameters.cpp); false for a negative limit.
std::string shellText(uint32_t index, double plain);
bool fitText(const std::string& text, int32_t maxChars, std::string* out);
// Text through the host entry points of the data model: the text of a normalized value
// (maxChars as the host passes it) and a typed text read back as a normalized value.
bool proofText(AAX_CEffectParameters* parameters, uint32_t index, double normalized, int32_t maxChars, std::string* out);
bool proofValue(AAX_CEffectParameters* parameters, uint32_t index, const char* text, double* normalized);
// The host's parameter update entry (normalized value), without a controller.
// kProofBypass as the index addresses the master bypass.
inline constexpr uint32_t kProofBypass = 0xFFFFFFFFu;
bool proofUpdate(AAX_CEffectParameters* parameters, uint32_t index, double normalized);
// What TimerWakeup does (ABI 6), with `controller` standing in for the host's: tell it what
// changed. Returns the OP_CHANGE_* bits it told.
uint32_t proofWakeup(AAX_CEffectParameters* parameters, AAX_IController* controller);
// The data model's own TimerWakeup, which has no controller in a proof.
AAX_Result proofTimerWakeup(AAX_CEffectParameters* parameters);
}
