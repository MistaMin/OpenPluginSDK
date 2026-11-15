// SPDX-License-Identifier: MIT
// C ABI exported by openplugin_targets::export_shell!.
#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
enum {
    OP_SHELL_ABI = 7,
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

// ---- ABI 4: the host's track ---------------------------------------------
// What the host says about the track the plugin sits on: its name, colour and
// kind (VST3 Vst::ChannelContext::IInfoListener, CLAP track-info). Delivered
// to Plugin::track_info_changed on the main thread, never on the audio thread.
enum {
    OP_TRACK_RETURN = 1,        // a return (send) track
    OP_TRACK_BUS = 2,           // a bus (group) track
    OP_TRACK_MASTER = 4,        // the master track
    OP_TRACK_HAS_COLOUR = 0x100 // `rgba` is valid
};
// Main thread only; allocates. Call it again whenever the host says something
// changed. `name_utf8` is NUL-terminated UTF-8, or NULL for no name (invalid
// bytes are replaced, more than 255 characters are cut). `rgba` is 0xRRGGBBAA
// and counts only with OP_TRACK_HAS_COLOUR in `flags`; bits of `flags` this
// header does not define are ignored. The plugin object is taken between
// blocks, like a state call: a block that starts meanwhile is passed through
// dry, and the call waits at most 2 seconds for one already running. Returns 1
// when delivered; 0 for a NULL instance, or when it gave up and delivered
// nothing. Safe to call while the audio thread is inside op_shell_process.
uint32_t op_shell_set_track_info(void* instance, const char* name_utf8, uint32_t rgba, uint32_t flags);

// ---- ABI 5: parameter groups -----------------------------------------------
// The group parameter `index` is listed under: names separated by '/',
// outermost first ("Filter/Envelope"), "" for the top level. Writes it to
// `buf` as UTF-8 with a NUL (room for `cap` bytes) and returns the length of
// the whole path in bytes, not counting the NUL; it was cut when that is
// >= `cap`. A cut falls on a UTF-8 boundary and the NUL is always written
// when `buf` is set and `cap` > 0; `buf` NULL with `cap` 0 asks for the
// length. An unknown `index` returns 0 and writes an empty string. A path is
// at most 255 bytes, no name in it is empty, and no name starts or ends with
// a space. No instance needed; any thread, no allocation after the first
// descriptor read.
size_t op_shell_param_group(uint32_t index, char* buf, size_t cap);

// ---- ABI 6: telling the host about changes --------------------------------
// The plugin's latency (op_shell_latency) or its parameter info (names, units,
// value text) changed at run time. The plugin says so through its
// HostNotifier, from any thread, process included; the library also compares
// the latency after every block. The shell polls op_shell_take_changes on its
// main thread (a timer, an idle call) and tells the host the way the format
// asks: VST3 restartComponent, Audio Unit property notifications, AAX
// SetSignalLatency and AAX_eNotificationEvent_ParameterNameChanged.
enum {
    OP_CHANGE_LATENCY = 1,      // read op_shell_latency again and tell the host
    OP_CHANGE_PARAM_INFO = 2    // tell the host to read the parameter info again
};
// Returns the OP_CHANGE_* bits raised since the last call and clears them, so
// each change is reported once; 0 for a NULL instance. A latency equal to the
// one op_shell_latency last returned is not reported (the host knows it), so a
// new engine that comes up with the latency the host already read is quiet.
// Bits this header does not define may appear in a newer library; ignore them.
// Any thread; never blocks, never allocates. op_shell_reset now applies the
// parameter values stored so far before it resets, so op_shell_latency right
// after a reset is the latency of those values.
uint32_t op_shell_take_changes(void* instance);
// ---- end of ABI 6 -----------------------------------------------------------

// ---- ABI 7: per-note expression ---------------------------------------------
// Notes with the host's note id, and per-note expressions addressed by that id
// (VST3 kNoteExpressionValueEvent). Three event kinds for op_shell_process_events,
// in the same OpEvent:
//   OP_EVENT_NOTE_ON_ID, OP_EVENT_NOTE_OFF_ID: `channel`, `note`, `velocity` as
//     OP_EVENT_NOTE_ON / OFF (the off's velocity is the release velocity);
//     `index` is the host's note id as int32 bits, 0xFFFFFFFF (-1) for none.
//     A note with an id never goes through MPE; one without (-1) does, for a
//     plugin that asked for MPE, like OP_EVENT_NOTE_ON.
//   OP_EVENT_NOTE_EXPRESSION: `index` the note id (or -1), `midi[0]` the
//     expression (OP_EXPRESSION_*), `channel` and `note` the channel and key
//     the expression is for, OP_NOTE_ANY for any (a host with ids sends
//     OP_NOTE_ANY in both), `value` in the expression's own units
//     (OpExpressionInfo.min .. max). Reaches the plugin only for an expression
//     it lists (op_shell_expressions); the value is clamped, NaN is skipped.
// A plugin that asked for MPE gets OP_EVENT_NOTE_ON, OP_EVENT_NOTE_OFF and
// OP_EVENT_MIDI through the library's MPE receiver: MIDI is all the Audio Unit
// and AAX shells pass, so they need nothing new for MPE.
enum {
    OP_EVENT_NOTE_ON_ID = 6,
    OP_EVENT_NOTE_OFF_ID = 7,
    OP_EVENT_NOTE_EXPRESSION = 8,
    OP_NOTE_ANY = 0xFF,
    // Expression ids: the CLAP note expression ids.
    OP_EXPRESSION_VOLUME = 0,      // linear gain 0..4, 1 is unity (VST3 kVolumeTypeID: norm * 4)
    OP_EXPRESSION_PAN = 1,         // 0 left, 0.5 centre, 1 right
    OP_EXPRESSION_TUNING = 2,      // semitones -120..120
    OP_EXPRESSION_VIBRATO = 3,     // 0..1
    OP_EXPRESSION_EXPRESSION = 4,  // 0..1
    OP_EXPRESSION_BRIGHTNESS = 5,  // 0..1
    OP_EXPRESSION_PRESSURE = 6,    // 0..1
    OP_EXPRESSION_COUNT = 7,
    OP_EXPRESSION_BIPOLAR = 1      // OpExpressionInfo.flags: centred (pan, tuning)
};
// One expression as a host shows it. The range is linear in the expression's
// own units, so a normalized value is (value - min) / (max - min).
typedef struct OpExpressionInfo {
    double min;
    double max;
    double default_value;
    uint32_t flags;      // OP_EXPRESSION_BIPOLAR
    uint32_t reserved;
    char name[64];
    char short_name[16];
    char unit[16];
} OpExpressionInfo;
// Bit (1 << OP_EXPRESSION_*) for each expression the plugin reacts to; 0 for
// none. No instance; message thread (it builds a plugin to ask it).
uint32_t op_shell_expressions(void);
// Fills `out` for expression `id`. Returns 1, or 0 writing nothing for a NULL
// `out`, an unknown id or one the plugin does not list. No instance.
uint32_t op_shell_expression_info(uint32_t id, OpExpressionInfo* out);
// The text of `value` (the expression's units) with the length rules of
// op_shell_param_format; an unknown id returns 0 and writes "". Message thread.
size_t op_shell_expression_format(uint32_t id, double value, char* buf, size_t cap);
// Reads NUL-terminated UTF-8 as a value of expression `id`, clamped. Returns 1,
// or 0 leaving *out_value untouched for NULL, an unknown id, text over 1024
// bytes, invalid UTF-8 or text that is not a value. Message thread.
int op_shell_expression_parse(uint32_t id, const char* text, double* out_value);
// ---- end of ABI 7 -----------------------------------------------------------

#if OP_HAS_TAIL
// Optional, with -DOP_TAIL=ON: how long the output rings after the input
// stops, in seconds. Infinity means it never ends. Safe to call from any
// thread while the instance exists.
double op_shell_tail_seconds(void* instance);
#endif
#ifdef __cplusplus
}
#endif
