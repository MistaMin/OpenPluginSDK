/* C ABI for an OpenPluginSDK static library.
 *
 * Seven Halls, RadarVU and ArcVu each hand a JUCE processor a Rust static
 * library. This is that boundary for any plugin that implements Plugin:
 * the host builds its parameter objects from op_param_info, forwards
 * values with op_set_param, and calls op_process_stereo from processBlock.
 *
 * op_set_param, op_get_param, op_apply_preset and op_meters only touch
 * atomics. The audio thread applies parameter changes inside op_process_*.
 * prepare, reset and destroy are not called concurrently with process.
 *
 * One static library exports these symbols. Link one plugin per library.
 * Generate them with openplugin_ffi::export_openplugin!(MyPlugin).
 *
 * Stereo process copies through a scratch buffer, so JUCE may pass the
 * same pointer for input and output.
 */
#ifndef OPENPLUGIN_H
#define OPENPLUGIN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    OP_UNIT_NONE = 0,
    OP_UNIT_DB = 1,
    OP_UNIT_HZ = 2,
    OP_UNIT_MS = 3,
    OP_UNIT_PERCENT = 4,
    OP_UNIT_RATIO = 5,
    OP_UNIT_SEMITONES = 6,
    OP_UNIT_CENTS = 7,
    OP_UNIT_SECONDS = 8,
    OP_UNIT_TOGGLE = 9,
    OP_UNIT_CHOICE = 10
};

typedef struct OpSpec {
    char id[64];
    char name[64];
    float min;
    float max;
    float default_value;
    /* Non-zero: JUCE interval is (max - min) / (steps - 1) when steps > 1. */
    int32_t is_discrete;
    int32_t steps;
    /* Non-zero: logarithmic. A JUCE range uses skew sqrt(min * max). */
    int32_t is_log;
    int32_t unit;
} OpSpec;

typedef struct OpMeters {
    float input_db[2];
    float output_db[2];
    float reduction_db;
} OpMeters;

/* Factory. These do not need an instance. */
int32_t op_param_count(void);
int32_t op_param_info(int32_t index, OpSpec* out);
void op_plugin_name(char* buf, int32_t cap);
int32_t op_preset_count(void);
void op_preset_name(int32_t index, char* buf, int32_t cap);

/* Instance. sample_rate, max_block and channels come from prepareToPlay.
 * channels is 1..8. max_block is at least 1. */
void* op_create(double sample_rate, uint32_t max_block, uint32_t channels);
void op_destroy(void* plugin);
int32_t op_prepare(void* plugin, double sample_rate, uint32_t max_block, uint32_t channels);
void op_reset(void* plugin);
void op_set_offline(void* plugin, int32_t offline);
uint32_t op_latency(void* plugin);

int32_t op_set_param(void* plugin, int32_t index, float value);
float op_get_param(void* plugin, int32_t index);
int32_t op_apply_preset(void* plugin, int32_t index);
int32_t op_preset_index(void* plugin);

/* Returns 1 on success. frames must be <= the prepared block. */
int32_t op_process_stereo(void* plugin,
                          const float* in_l, const float* in_r,
                          float* out_l, float* out_r,
                          uint32_t frames);
/* Planar, channel-major. `input` and `output` point at `channels` pointers. */
int32_t op_process_planar(void* plugin,
                          const float* const* input, float* const* output,
                          uint32_t channels, uint32_t frames);
/* Interleaved, frame-major. Safe when input and output alias. */
int32_t op_process_interleaved(void* plugin,
                               const float* input, float* output,
                               uint32_t channels, uint32_t frames);

void op_meters(void* plugin, OpMeters* out);

/* State blob. op_save writes at most cap bytes and returns the full size,
 * or -1. A null buffer with cap 0 returns the size needed. */
int32_t op_save(void* plugin, uint8_t* buf, uint32_t cap);
int32_t op_load(void* plugin, const uint8_t* buf, uint32_t len);

#ifdef __cplusplus
}
#endif

#endif
