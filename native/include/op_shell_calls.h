// SPDX-License-Identifier: MIT
// The op_shell_* call patterns every shell repeats: the two-call text protocol
// (call with a small buffer, call again with the length it asked for) and the
// state save loop. Main thread only; all of it allocates.
#pragma once
#include "op_shell.h"
#include <algorithm>
#include <cstdint>
#include <string>
#include <vector>

namespace op_shell_calls {
// `call(buffer, capacity)` writes at most `capacity` bytes and returns the full length.
template <class Call> std::string text(Call call) {
    char small[128];
    const size_t length = call(small, sizeof(small));
    if (length < sizeof(small)) return std::string(small, length);
    std::string big(length + 1, '\0');
    big.resize(std::min<size_t>(call(big.data(), big.size()), length));
    return big;
}
// The plugin's text for plain value `plain` of parameter `index` ("6.0 dB").
inline std::string param_text(uint32_t index, double plain) {
    return text([=](char* buffer, size_t capacity) { return op_shell_param_format(index, plain, buffer, capacity); });
}
// The group path of parameter `index` ("Filter/Envelope"; "" is the top level).
inline std::string param_group(uint32_t index) {
    return text([=](char* buffer, size_t capacity) { return op_shell_param_group(index, buffer, capacity); });
}
// Calls to op_shell_save_state before a state that keeps growing is given up.
inline constexpr int kSaveAttempts = 8;
// The whole state of `instance`. The first call uses a buffer of `guess` bytes (0 is
// the size query); a state that is larger, or grew in between, is asked for again
// at the size the plugin then reports.
inline bool save_blob(void* instance, std::vector<uint8_t>& out, uint32_t guess = 0) {
    if (!instance) return false;
    uint32_t size = guess;
    for (int attempt = 0; attempt < kSaveAttempts; ++attempt) {
        out.assign(size, 0);
        uint32_t needed = 0;
        const uint32_t result = op_shell_save_state(instance, size ? out.data() : nullptr, size, &needed);
        if (result == OP_STATE_OK) {
            if (needed == 0 || needed > size) return false;
            out.resize(needed);
            return true;
        }
        if (result != OP_STATE_TOO_SMALL || needed == 0) return false;
        size = needed;
    }
    return false;
}
}
