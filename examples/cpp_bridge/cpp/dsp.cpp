// SPDX-License-Identifier: MIT
#include "dsp.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <new>
#include <stdexcept>

namespace opbridge {

namespace {
constexpr char kMagic[4] = {'O', 'P', 'B', 'R'};
constexpr std::uint32_t kVersion = 1;
constexpr std::size_t kStateSize = 16;
float db_to_lin(float db) noexcept { return std::pow(10.0f, db / 20.0f); }
}  // namespace

void OnePoleGain::prepare(float sample_rate, std::size_t max_block, std::size_t channels) {
#ifdef OPB_TEST_FAULTS
    if (fault_ == 3) throw std::runtime_error("injected std::exception in prepare");
    if (fault_ == 4) throw 42;
#endif
    if (!std::isfinite(sample_rate) || sample_rate < 1000.0f || sample_rate > 768000.0f)
        throw std::invalid_argument("sample rate out of range");
    if (channels == 0 || channels > kMaxChannels) throw std::invalid_argument("channel count out of range");
    if (max_block == 0) throw std::invalid_argument("max block must be positive");
    // All allocation happens here. Build first, then swap in: a throw leaves
    // the old state alone.
    std::vector<float> z(channels, 0.0f), g(channels, db_to_lin(gain_db_));
    sample_rate_ = sample_rate;
    z_.swap(z);
    gain_.swap(g);
    update_coefficients();
}

void OnePoleGain::reset() noexcept {
    std::fill(z_.begin(), z_.end(), 0.0f);
    std::fill(gain_.begin(), gain_.end(), gain_target_);
}

void OnePoleGain::update_coefficients() noexcept {
    const float fc = std::min(cutoff_, 0.45f * sample_rate_);
    coeff_ = std::min(1.0f, 1.0f - std::exp(-6.2831853f * fc / sample_rate_));
    gain_slew_ = 1.0f - std::exp(-1.0f / (0.005f * sample_rate_));  // 5 ms
}

void OnePoleGain::set_cutoff_hz(float hz) noexcept {
    if (!std::isfinite(hz)) return;
    cutoff_ = std::min(std::max(hz, kMinCutoff), kMaxCutoff);
    update_coefficients();
}

void OnePoleGain::set_gain_db(float db) noexcept {
    if (!std::isfinite(db)) return;
    gain_db_ = std::min(std::max(db, kMinGainDb), kMaxGainDb);
    gain_target_ = db_to_lin(gain_db_);
}

void OnePoleGain::process(std::size_t channel, float* data, std::size_t n) noexcept {
    if (channel >= z_.size()) return;  // channels past prepare() are untouched
#ifdef OPB_TEST_FAULTS
    if (fault_ == 1) {
        volatile float* p = new float[16];  // a real heap allocation on the audio path
        p[0] = 1.0f;
        delete[] const_cast<float*>(p);
    } else if (fault_ == 2) {
        void* p = std::malloc(64);  // and one that bypasses operator new
        *static_cast<volatile char*>(p) = 1;
        std::free(p);
    }
#endif
    float z = z_[channel], g = gain_[channel];
    const float a = coeff_, target = gain_target_, slew = gain_slew_;
    for (std::size_t i = 0; i < n; ++i) {
        float x = data[i];
        if (!std::isfinite(x)) x = 0.0f;  // never let NaN into the state
        z += a * (x - z);
        if (std::fabs(z) < 1e-20f) z = 0.0f;
        g += slew * (target - g);
        float y = z * g;
        data[i] = std::fabs(y) < 1e-30f ? 0.0f : y;  // no subnormals out
    }
    z_[channel] = z;
    gain_[channel] = g;
}

std::vector<std::uint8_t> OnePoleGain::save_state() const {
    std::vector<std::uint8_t> out(kStateSize);
    std::memcpy(out.data(), kMagic, 4);
    std::memcpy(out.data() + 4, &kVersion, 4);
    std::memcpy(out.data() + 8, &cutoff_, 4);
    std::memcpy(out.data() + 12, &gain_db_, 4);
    return out;
}

void OnePoleGain::load_state(const std::uint8_t* data, std::size_t size) {
#ifdef OPB_TEST_FAULTS
    if (fault_ == 5) throw 7;
#endif
    if (size != kStateSize) throw std::invalid_argument("state: wrong size");
    if (std::memcmp(data, kMagic, 4) != 0) throw std::invalid_argument("state: bad magic");
    std::uint32_t version;
    float cutoff, gain;
    std::memcpy(&version, data + 4, 4);
    std::memcpy(&cutoff, data + 8, 4);
    std::memcpy(&gain, data + 12, 4);
    if (version != kVersion) throw std::invalid_argument("state: unknown version");
    if (!std::isfinite(cutoff) || !std::isfinite(gain)) throw std::invalid_argument("state: not finite");
    set_cutoff_hz(cutoff);
    set_gain_db(gain);
}

}  // namespace opbridge
