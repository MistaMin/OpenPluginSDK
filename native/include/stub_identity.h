// SPDX-License-Identifier: MIT
// The ABI 8 identity calls of a stand-in plugin without pins: every identifier
// is the derived one (the parameter's index, OpParamDesc.id, no Audio Unit
// codes). Test builds only. A stand-in with pins of its own defines
// OP_STUB_OWN_IDENTITY before it includes this (or the header that includes it).
//
// Also the ABI 9 calls of a stand-in that never renames its parameters and
// whose latency never moves: the instance's table is the first table, and the
// largest latency is the latency. A stand-in with its own defines
// OP_STUB_OWN_ABI9.
#pragma once
#include "op_shell.h"
#include <cstring>

#ifndef OP_STUB_OWN_IDENTITY
extern "C" {
uint32_t op_shell_identity(void) { return 0; }
uint32_t op_shell_param_host_id(uint32_t format, uint32_t index) {
    return format == OP_HOST_VST3 || format == OP_HOST_AU ? index : OP_NO_HOST_ID;
}
size_t op_shell_param_aax_id(uint32_t index, char* buf, size_t cap) {
    OpParamDesc desc{};
    const size_t length = op_shell_param(index, &desc) ? std::strlen(desc.id) : 0;
    if (buf && cap > 0) {
        const size_t count = length < cap - 1 ? length : cap - 1;
        std::memcpy(buf, desc.id, count);
        buf[count] = '\0';
    }
    return length;
}
uint32_t op_shell_au_code(uint32_t) { return 0; }
}
#endif

#ifndef OP_STUB_OWN_ABI9
extern "C" {
uint32_t op_shell_instance_param(const void*, uint32_t index, OpParamDesc* out) { return op_shell_param(index, out); }
size_t op_shell_instance_param_format(const void*, uint32_t index, double plain, char* buf, size_t cap) {
    return op_shell_param_format(index, plain, buf, cap);
}
int op_shell_instance_param_parse(const void*, uint32_t index, const char* text, double* out_plain) {
    return op_shell_param_parse(index, text, out_plain);
}
uint32_t op_shell_max_latency(const void* instance) { return instance ? op_shell_latency(const_cast<void*>(instance)) : 0; }
uint32_t op_shell_latency_now(const void* instance) { return instance ? op_shell_latency(const_cast<void*>(instance)) : 0; }
}
#endif
