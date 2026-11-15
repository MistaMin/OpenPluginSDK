// SPDX-License-Identifier: MIT
// The shared scenario of the format parity test (docs/FORMAT_PARITY.md), as
// written by scripts/format_parity/host (`parity-host gen`). Little endian:
// "OPFP", version 1, frames u32, block u32, rate f64, event count u32, events
// {frame u32, param u32, value f32} (param 0 = Gain in dB, 1 = Warm), then the
// left and right input, `frames` f32 each. The output is the left then the
// right channel, f32.
#pragma once
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

struct ParityEvent {
    uint32_t frame;
    uint32_t param;  // 0 Gain, 1 Warm
    float value;     // plain
};

struct ParityScenario {
    uint32_t frames = 0;
    uint32_t block = 0;
    double rate = 0.0;
    std::vector<ParityEvent> events;
    std::vector<float> input[2];
};

inline const char* parity_param_name(uint32_t param) { return param == 0 ? "Gain" : "Warm"; }

[[noreturn]] inline void parity_fail(const char* what) {
    std::fprintf(stderr, "format parity host failed: %s\n", what);
    std::exit(2);
}
#define PARITY_CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "format parity host failed, line %d: %s\n", __LINE__, #cond); std::exit(2); } } while (0)

inline ParityScenario parity_read(const char* path) {
    std::FILE* f = std::fopen(path, "rb");
    if (!f) parity_fail("cannot open the scenario");
    std::vector<unsigned char> b;
    unsigned char chunk[65536];
    size_t n;
    while ((n = std::fread(chunk, 1, sizeof chunk, f)) > 0) b.insert(b.end(), chunk, chunk + n);
    std::fclose(f);
    if (b.size() < 28 || std::memcmp(b.data(), "OPFP", 4) != 0) parity_fail("not a scenario file");
    auto u32 = [&](size_t o) { uint32_t v; std::memcpy(&v, &b[o], 4); return v; };
    if (u32(4) != 1) parity_fail("scenario version");
    ParityScenario s;
    s.frames = u32(8);
    s.block = u32(12);
    std::memcpy(&s.rate, &b[16], 8);
    const uint32_t count = u32(24);
    size_t o = 28;
    if (b.size() != o + count * 12ull + 8ull * s.frames) parity_fail("scenario size");
    for (uint32_t i = 0; i < count; ++i, o += 12) {
        ParityEvent e;
        e.frame = u32(o);
        e.param = u32(o + 4);
        std::memcpy(&e.value, &b[o + 8], 4);
        s.events.push_back(e);
    }
    for (int ch = 0; ch < 2; ++ch) {
        s.input[ch].resize(s.frames);
        std::memcpy(s.input[ch].data(), &b[o], 4ull * s.frames);
        o += 4ull * s.frames;
    }
    return s;
}

inline void parity_write(const char* path, const std::vector<float> out[2]) {
    std::FILE* f = std::fopen(path, "wb");
    if (!f) parity_fail("cannot write the output");
    for (int ch = 0; ch < 2; ++ch) std::fwrite(out[ch].data(), 4, out[ch].size(), f);
    std::fclose(f);
}

// Frames where a format that takes parameter values only between blocks must
// start a new sub-block: every host block edge and every event frame.
inline std::vector<uint32_t> parity_cuts(const ParityScenario& s) {
    std::vector<uint32_t> cuts;
    for (uint32_t f = 0; f < s.frames; f += s.block) cuts.push_back(f);
    for (const auto& e : s.events) if (e.frame < s.frames) cuts.push_back(e.frame);
    cuts.push_back(s.frames);
    std::sort(cuts.begin(), cuts.end());
    cuts.erase(std::unique(cuts.begin(), cuts.end()), cuts.end());
    return cuts;
}
