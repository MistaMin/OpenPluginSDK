// SPDX-License-Identifier: MIT
// The counter of alloc_hook.cpp. Test code only.
#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
void op_alloc_arm(void);
void op_alloc_disarm(void);
// Called by the interposers, and by the proof's operator new/delete.
void op_alloc_note(void);
void op_alloc_note_free(void* p);
// Allocations and frees seen while armed since the process started.
uint64_t op_alloc_count(void);
uint64_t op_free_count(void);
#ifdef __cplusplus
}
#endif
