// SPDX-License-Identifier: MIT
// Helpers shared by the in-process shell proofs. Expected values come from the
// op_shell_* ABI, not from one plugin's constants. These are not DAW tests.
#pragma once
#include "op_shell.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <vector>

inline void proof_require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "shell proof failed: %s\n", message);
        std::exit(1);
    }
}

inline void fill_sine(float* dst, int frames, double sample_rate, double hz, double amplitude, double start) {
    for (int i = 0; i < frames; ++i) dst[i] = static_cast<float>(amplitude * std::sin(2.0 * M_PI * hz * (start + i) / sample_rate));
}

inline double rms_range(const float* samples, int begin, int count) {
    double sum = 0.0;
    for (int i = begin; i < begin + count; ++i) sum += static_cast<double>(samples[i]) * samples[i];
    return std::sqrt(sum / static_cast<double>(count));
}

inline double gain_db(const float* input, const float* output, int begin, int count) {
    const double in = rms_range(input, begin, count);
    const double out = rms_range(output, begin, count);
    if (in < 1e-12) return -200.0;
    return 20.0 * std::log10(out / in);
}

/// A second instance of the same plugin made straight through the C ABI. The
/// proofs use it for the values the host adapter must report (parameter count,
/// latency, tail) and for the audio the adapter must produce. Destroyed by the
/// destructor; null when the ABI refused to create it.
struct ReferenceShell {
    void* instance = nullptr;
    explicit ReferenceShell(uint32_t channels, double sample_rate = 48000.0, uint32_t max_block = 4096) {
        instance = op_shell_create(sample_rate, max_block, channels);
    }
    ~ReferenceShell() { op_shell_destroy(instance); }
    ReferenceShell(const ReferenceShell&) = delete;
    ReferenceShell& operator=(const ReferenceShell&) = delete;
};

/// Render `blocks` blocks of the proof tone through a shell instance, applying
/// every plain value before each block, and return the last output block.
inline std::vector<float> render_shell(void* instance, uint32_t channels, int blocks, const std::vector<float>& plain) {
    constexpr int kFrames = 512;
    std::vector<float> inL(kFrames), inR(kFrames), outL(kFrames), outR(kFrames);
    for (int block = 0; block < blocks; ++block) {
        fill_sine(inL.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        if (channels > 1) fill_sine(inR.data(), kFrames, 48000.0, 1000.0, 0.25, block * kFrames);
        for (size_t i = 0; i < plain.size(); ++i) {
            proof_require(op_shell_set_plain(instance, static_cast<uint32_t>(i), plain[i]) != 0, "op_shell_set_plain");
        }
        proof_require(op_shell_process(instance, inL.data(), channels > 1 ? inR.data() : nullptr,
                                       outL.data(), channels > 1 ? outR.data() : nullptr, kFrames) != 0,
                      "op_shell_process");
        for (float sample : outL) proof_require(std::isfinite(sample), "finite shell output");
        if (block + 1 == blocks) return outL;
    }
    return {};
}

/// Largest absolute sample difference between two equal-length blocks.
inline double max_difference(const std::vector<float>& a, const std::vector<float>& b) {
    proof_require(a.size() == b.size(), "equal block lengths");
    double worst = 0.0;
    for (size_t i = 0; i < a.size(); ++i) worst = std::max(worst, std::fabs(static_cast<double>(a[i]) - static_cast<double>(b[i])));
    return worst;
}
