// SPDX-License-Identifier: MIT
// C ABI exported by openplugin_targets::export_shell!.
#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
enum {
    OP_SHELL_ABI = 3,
    OP_PARAM_LOG = 1,
    OP_PARAM_DISCRETE = 2,
    OP_PARAM_AUTOMATABLE = 4,
    OP_SHELL_AAX_MAX_PARAMS = 64
};
typedef struct OpShellInfo {
    uint32_t abi;
    uint32_t param_count;
    uint8_t processor_cid[16];
    uint8_t controller_cid[16];
    uint32_t manufacturer_id;
    uint32_t product_id;
    uint32_t plugin_id_mono;
    uint32_t plugin_id_stereo;
    char name[64];
    char vendor[64];
    char version[32];
    char category[128];
    char id[128];
} OpShellInfo;
typedef struct OpParamDesc {
    double min;
    double max;
    double default_plain;
    double default_normalized;
    uint32_t steps;
    uint32_t flags;
    char name[64];
    char unit[16];
    char id[64];
} OpParamDesc;
uint32_t op_shell_info(OpShellInfo* out);
uint32_t op_shell_param(uint32_t index, OpParamDesc* out);
double op_shell_to_plain(uint32_t index, double normalized);
double op_shell_to_normalized(uint32_t index, double plain);
void* op_shell_create(double sample_rate, uint32_t max_block, uint32_t channels);
void op_shell_destroy(void* instance);
void op_shell_reset(void* instance);
uint32_t op_shell_latency(void* instance);
uint32_t op_shell_set_plain(void* instance, uint32_t index, float plain);
float op_shell_get_plain(void* instance, uint32_t index);
uint32_t op_shell_set_normalized(void* instance, uint32_t index, float normalized);
float op_shell_get_normalized(void* instance, uint32_t index);
uint32_t op_shell_process(void* instance, const float* in_left, const float* in_right, float* out_left, float* out_right, uint32_t frames);
// Whole plugin state: the parameter values plus the plugin's own chunk, as one
// opaque blob. Main thread only. Both calls allocate. Safe to call while the
// audio thread is inside op_shell_process: a block that starts during a call
// is skipped (the input is copied to the output, op_shell_process still
// returns 1), and a call waits at most 2 seconds for a block already running
// (then fails and changes nothing). Never call them from the audio thread.
enum {
    OP_STATE_FAILED = 0,
    OP_STATE_OK = 1,
    OP_STATE_TOO_SMALL = 2
};
// Writes the state at `out` (room for `capacity` bytes) and its length to
// *needed (may be NULL). Returns OP_STATE_OK, or OP_STATE_TOO_SMALL with
// *needed set and nothing written, or OP_STATE_FAILED. `out` NULL with
// capacity 0 asks for the size. The size can change between that query and
// the copy: allocate *needed bytes, call again, and repeat while the result
// is OP_STATE_TOO_SMALL.
uint32_t op_shell_save_state(void* instance, uint8_t* out, uint32_t capacity, uint32_t* needed);
// Restores a blob from op_shell_save_state. Returns 1 or 0. All or nothing:
// a corrupt, truncated, foreign or refused blob returns 0 and changes no
// parameter and no plugin state (a plugin whose load_state refuses a chunk
// must itself have changed nothing).
uint32_t op_shell_load_state(void* instance, const uint8_t* data, uint32_t length);
// ---- ABI 2: events, transport, sidechain, MIDI out, layouts, kind --------
//
// Versioning: OP_SHELL_ABI is the version of this header. op_shell_abi_version()
// is the version of the library. A version only ever adds functions, enum
// values and struct fields at the end; nothing is removed or changed. A shell
// compiled against OP_SHELL_ABI = N works with a library of version >= N and
// must refuse one below N. A change that cannot be additive gets new symbol
// names instead. Channel order on every bus is the SDK's canonical order (the
// CLAP surround ids); the shell reorders from the host's order.
uint32_t op_shell_abi_version(void);

enum {
    OP_KIND_EFFECT = 0,
    OP_KIND_INSTRUMENT = 1,
    OP_KIND_NOTE_EFFECT = 2,
    OP_NOTE_PORT_INPUT = 1,
    OP_NOTE_PORT_OUTPUT = 2
};
// Plugin kind (OP_KIND_*) and note ports (OP_NOTE_PORT_* bits). No instance needed.
uint32_t op_shell_kind(void);
uint32_t op_shell_note_ports(void);

enum {
    OP_EVENT_NOTE_ON = 1,
    OP_EVENT_NOTE_OFF = 2,
    OP_EVENT_MIDI = 3,
    OP_EVENT_PARAM_PLAIN = 4,
    OP_EVENT_PARAM_NORMALIZED = 5,
    OP_MAX_EVENTS = 1024,
    OP_MAX_MIDI_OUT = 1024,
    OP_MAX_CHANNELS = 64
};
typedef struct OpEvent {
    uint32_t offset;     // sample offset in the block, < frames
    uint32_t kind;       // OP_EVENT_*
    uint32_t index;      // parameter index (parameter kinds)
    float value;         // plain or normalized value (parameter kinds)
    uint8_t channel;     // MIDI channel 0..15 (note kinds)
    uint8_t note;        // 0..127
    uint8_t velocity;    // 0..127
    uint8_t reserved;
    uint8_t midi[4];     // status + 2 data bytes (OP_EVENT_MIDI)
} OpEvent;
typedef struct OpMidiEvent {
    uint32_t offset;     // sample offset in the block just processed
    uint8_t data[4];     // 3 MIDI bytes + 0
} OpMidiEvent;

enum {
    OP_TRANSPORT_PLAYING = 1,
    OP_TRANSPORT_RECORDING = 2,
    OP_TRANSPORT_HAS_TEMPO = 4,
    OP_TRANSPORT_HAS_SIGNATURE = 8,
    OP_TRANSPORT_HAS_BEATS = 16,     // quarter_notes, bar_start, bar_number
    OP_TRANSPORT_HAS_SECONDS = 32
};
typedef struct OpTransport {
    uint32_t flags;
    uint32_t numerator;
    uint32_t denominator;
    uint32_t reserved;
    double tempo;
    int64_t sample_pos;
    double seconds;
    double quarter_notes;
    double bar_start;
    int64_t bar_number;
} OpTransport;

typedef struct OpProcess {
    uint32_t struct_size;            // sizeof(OpProcess)
    uint32_t frames;
    const float* const* inputs;      // input_channels pointers; NULL when 0 (instrument)
    uint32_t input_channels;
    uint32_t output_channels;
    float* const* outputs;           // output_channels pointers
    const float* const* sidechain;   // NULL and 0 for none
    uint32_t sidechain_channels;
    uint32_t event_count;
    const OpEvent* events;           // any order; at most OP_MAX_EVENTS
    const OpTransport* transport;    // NULL keeps the previous transport
} OpProcess;

// Audio thread. No allocation. Returns 1, or 0 when refused (struct too small,
// a NULL array with a non-zero count, too many events or channels, a block
// longer than the size given to op_shell_create); a refused block touches
// nothing. An event with offset >= frames, an unknown kind or an invalid value
// is skipped. Events are applied sample-accurately: the block is cut at each
// distinct offset. Inputs may alias outputs. One input channel feeds a wider
// engine bus on every channel. A block that starts during a state call is
// passed through dry and returns 1.
uint32_t op_shell_process_events(void* instance, const OpProcess* args);

// MIDI the plugin produced in the last block, after op_shell_process_events,
// on the same thread. Copies up to `capacity` events to `out` and returns the
// count; call again for the rest. The next block clears what was not read.
// No allocation. op_shell_midi_dropped returns the events lost to a full
// buffer (OP_MAX_MIDI_OUT per block) since its last call.
uint32_t op_shell_midi_out(void* instance, OpMidiEvent* out, uint32_t capacity);
uint32_t op_shell_midi_dropped(const void* instance);

// Layouts. Queries need no instance. A bus is absent (kind 0, 0 channels) or has
// a kind, a channel count and roles[i]: the CLAP surround id of channel i in the
// canonical order (0xFF: none, as for mono, ambisonic, discrete and channels
// past 16).
enum {
    OP_BUS_ABSENT = 0, OP_BUS_MONO = 1, OP_BUS_STEREO = 2,
    OP_BUS_SURROUND = 3, OP_BUS_AMBISONIC = 4, OP_BUS_DISCRETE = 5,
    OP_ROLE_NONE = 0xFF, OP_ROLE_SLOTS = 16
};
typedef struct OpBus {
    uint32_t kind;
    uint32_t channels;
    uint8_t roles[OP_ROLE_SLOTS];
} OpBus;
typedef struct OpLayout {
    OpBus input;
    OpBus output;
    OpBus sidechain;
    char name[64];
} OpLayout;
uint32_t op_shell_layout_count(void);
uint32_t op_shell_layout(uint32_t index, OpLayout* out);
// Select a layout before the first block (allocates; main thread). Returns 1 or 0.
uint32_t op_shell_set_layout(void* instance, uint32_t index);
uint32_t op_shell_layout_selected(void* instance);

// Offline (bounce) rendering; takes effect at the next block.
void op_shell_set_offline(void* instance, uint32_t offline);

// ---- ABI 3: the plugin's own parameter text ------------------------------
// Main thread only; both calls may allocate. Never call them from the audio thread.
//
// Writes the plugin's text for the plain value `plain` of parameter `index`
// (what its own display shows, e.g. "6.0 dB", "Hall", "On") to `buf` as UTF-8
// with a NUL (room for `cap` bytes). Returns the length of the whole text in
// bytes, not counting the NUL; the text was cut when that is >= `cap`. A cut
// falls on a UTF-8 boundary and the NUL is always written when `buf` is set
// and `cap` > 0. `buf` NULL writes nothing: with `cap` 0 it asks for the
// length. An unknown `index` returns 0 and writes an empty string. A
// non-finite `plain` shows the default.
size_t op_shell_param_format(uint32_t index, double plain, char* buf, size_t cap);
// Reads `text` (NUL-terminated UTF-8; units, choice labels, on/off; clamped to
// the range) as a value of parameter `index` and stores the plain value in
// *out_plain. Returns 1, or 0 and leaves *out_plain untouched for a NULL
// pointer, an unknown index, text that is not UTF-8 or is longer than 1024
// bytes, or text that is not a value. Never crashes on hostile input.
int op_shell_param_parse(uint32_t index, const char* text, double* out_plain);

#if OP_HAS_TAIL
// Optional, with -DOP_TAIL=ON: how long the output rings after the input
// stops, in seconds. Infinity means it never ends. Safe to call from any
// thread while the instance exists.
double op_shell_tail_seconds(void* instance);
#endif
#ifdef __cplusplus
}
#endif
