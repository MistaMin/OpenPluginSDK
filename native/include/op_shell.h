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
#ifdef __cplusplus
}
#endif
