// SPDX-License-Identifier: MIT
// C ABI exported by openplugin_targets::export_shell!.
#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
enum {
    OP_SHELL_ABI = 1,
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
#if OP_HAS_TAIL
// Optional, with -DOP_TAIL=ON: how long the output rings after the input
// stops, in seconds. Infinity means it never ends. Safe to call from any
// thread while the instance exists.
double op_shell_tail_seconds(void* instance);
#endif
#ifdef __cplusplus
}
#endif
