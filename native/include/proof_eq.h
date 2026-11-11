// SPDX-License-Identifier: MIT
// Shared numbers for the FreEQ in-process proof. Not a DAW test.
#pragma once
#include <cmath>
#include <cstddef>
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
