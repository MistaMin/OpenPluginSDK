// SPDX-License-Identifier: MIT
// The ABI 8 identity calls of a stand-in plugin without pins: every identifier
// is the derived one (the parameter's index, OpParamDesc.id, no Audio Unit
// codes). Test builds only. A stand-in with pins of its own defines
// OP_STUB_OWN_IDENTITY before it includes this (or the header that includes it).
#pragma once
#include "op_shell.h"
#include <cstring>

#ifndef OP_STUB_OWN_IDENTITY
extern "C" {
uint32_t op_shell_identity(void) { return 0; }
uint32_t op_shell_param_host_id(uint32_t format, uint32_t index) {
    return format == OP_HOST_VST3 || format == OP_HOST_AU ? index : static_cast<uint32_t>(OP_NO_HOST_ID);
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
