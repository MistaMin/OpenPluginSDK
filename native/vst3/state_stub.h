// SPDX-License-Identifier: MIT
// The blob layout of state_stub.cpp, shared with the VST3 state proof. The stub
// is a test plugin with a chunk of its own; none of this is shipped.
#pragma once
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

namespace state_stub {
constexpr uint32_t kMagic = 0x42555453;  // 'STUB'
constexpr uint32_t kVersion = 1;
constexpr uint32_t kHeader = 24;
constexpr uint8_t kPadByte = 0xA5;
constexpr uint32_t kMaxPad = 4096;

// magic, version, level, chunk gain, pad length, grow, then `pad` bytes of 0xA5.
// `level` is the plane (the one parameter); `gain` is the plugin's own chunk:
// the audio is the input times level times gain. `grow` is the number of
// further failed copies for which the stub reports a state 16 bytes larger than
// the last time, to prove the host loops.
struct Blob {
    float level = 0.5f;
    float gain = 1.0f;
    uint32_t pad = 0;
    uint32_t grow = 0;
};

inline std::vector<uint8_t> encode(const Blob& b) {
    std::vector<uint8_t> out(kHeader + b.pad, kPadByte);
    const uint32_t head[2] = {kMagic, kVersion};
    std::memcpy(out.data(), head, 8);
    std::memcpy(out.data() + 8, &b.level, 4);
    std::memcpy(out.data() + 12, &b.gain, 4);
    std::memcpy(out.data() + 16, &b.pad, 4);
    std::memcpy(out.data() + 20, &b.grow, 4);
    return out;
}

inline bool decode(const uint8_t* data, size_t length, Blob& b) {
    if (!data || length < kHeader) return false;
    uint32_t head[2];
    std::memcpy(head, data, 8);
    Blob r;
    std::memcpy(&r.level, data + 8, 4);
    std::memcpy(&r.gain, data + 12, 4);
    std::memcpy(&r.pad, data + 16, 4);
    std::memcpy(&r.grow, data + 20, 4);
    if (head[0] != kMagic || head[1] != kVersion) return false;
    if (!(r.level >= 0.0f && r.level <= 1.0f) || !(r.gain >= 0.0f && r.gain <= 4.0f)) return false;
    if (r.pad > kMaxPad || r.grow > 64 || length != kHeader + r.pad) return false;
    for (uint32_t i = 0; i < r.pad; ++i)
        if (data[kHeader + i] != kPadByte) return false;
    b = r;
    return true;
}
}
