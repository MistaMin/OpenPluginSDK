// SPDX-License-Identifier: MIT
// What the Audio Unit and its Cocoa view share. Both are in one binary, in one process.
#pragma once
#include <stdint.h>
// Private property: the address of the Audio Unit's C++ object, as a void*.
enum { kOpenPluginAUProperty_Instance = 64001 };
// Null or zero once the Audio Unit is gone.
void* op_au_engine(void* instance);
uint32_t op_au_param_count(void* instance);
uint32_t op_au_param(void* instance, uint32_t index, float* value);
// The AudioUnitParameterID of the parameter at `index` of the plugin's table.
uint32_t op_au_param_id(uint32_t index);
// The last failed initialize or render status since the previous call, or 0.
int32_t op_au_take_error(void* instance);
// The host's name for the track (kAudioUnitProperty_ContextName) as UTF-8 in `name`
// (empty for none), and in `serial` a count that moves whenever it changes. Returns 0
// until the host has said anything, and once the Audio Unit is gone.
uint32_t op_au_track(void* instance, char* name, uint32_t capacity, uint32_t* serial);
