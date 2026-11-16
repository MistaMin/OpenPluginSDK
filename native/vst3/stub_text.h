// SPDX-License-Identifier: MIT
// The ABI 3 parameter text calls (and the ABI 5 group call: no groups) for the stand-in plugins. Test builds only. The
// text is the plain value with two decimals and " stub" (with an accent, a check mark and an emoji: 2, 3 and 4 UTF-8 bytes), so the shell's use of
// the plugin's own text (not the normalized number) is visible.
#pragma once
#include "op_shell.h"
#include "stub_identity.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>

extern "C" {
size_t op_shell_param_format(uint32_t, double plain, char* buf, size_t cap) {
    char text[64];
    const int length = std::snprintf(text, sizeof(text), "%.2f \xC3\xA9\xE2\x9C\x93\xF0\x9F\x98\x80 stub", plain);
    if (buf && cap > 0) {
        const size_t count = static_cast<size_t>(length) < cap - 1 ? static_cast<size_t>(length) : cap - 1;
        std::memcpy(buf, text, count);
        buf[count] = 0;
    }
    return static_cast<size_t>(length);
}
int op_shell_param_parse(uint32_t, const char* text, double* out_plain) {
    if (!text || !out_plain) return 0;
    char* end = nullptr;
    const double value = std::strtod(text, &end);
    if (end == text) return 0;
    *out_plain = value;
    return 1;
}
// ---- ABI 5: no parameter has a group (every parameter is top level).
size_t op_shell_param_group(uint32_t, char* buf, size_t cap) {
    if (buf && cap > 0) buf[0] = '\0';
    return 0;
}
// ---- ABI 7: no per-note expression (the controller offers no INoteExpressionController).
uint32_t op_shell_expressions(void) { return 0; }
uint32_t op_shell_expression_info(uint32_t, OpExpressionInfo*) { return 0; }
size_t op_shell_expression_format(uint32_t, double, char* buf, size_t cap) {
    if (buf && cap > 0) buf[0] = '\0';
    return 0;
}
int op_shell_expression_parse(uint32_t, const char*, double*) { return 0; }
}
