// SPDX-License-Identifier: MIT
// The "existing" C++ DSP of this example: a one-pole low-pass followed by a
// smoothed gain. Plain C++17 with no knowledge of Rust or cxx; this is the
// shape of code a team brings to the bridge. Written for this example.
#pragma once
#include <cstddef>
#include <cstdint>
#include <vector>

namespace opbridge {

class OnePoleGain {
public:
    static constexpr std::size_t kMaxChannels = 64;
    static constexpr float kMinCutoff = 20.0f, kMaxCutoff = 20000.0f;
    static constexpr float kMinGainDb = -24.0f, kMaxGainDb = 24.0f;

    // Allocates the per-channel state. Throws std::invalid_argument for a bad
    // rate or channel count, std::bad_alloc when memory runs out. Not for the
    // audio thread.
    void prepare(float sample_rate, std::size_t max_block, std::size_t channels);
    void reset() noexcept;

    void set_cutoff_hz(float hz) noexcept;
    void set_gain_db(float db) noexcept;
    float cutoff_hz() const noexcept { return cutoff_; }
    float gain_db() const noexcept { return gain_db_; }

    // In place, one channel. No allocation, no locks, no exceptions.
    void process(std::size_t channel, float* data, std::size_t n) noexcept;

    // 16 bytes: "OPBR", u32 version, f32 cutoff, f32 gain (host byte order).
    std::vector<std::uint8_t> save_state() const;
    // Validates everything before changing anything (strong guarantee).
    void load_state(const std::uint8_t* data, std::size_t size);

#ifdef OPB_TEST_FAULTS
    // 0 none, 1 `new` in process, 2 malloc in process, 3 std::exception in
    // prepare, 4 non-std exception in prepare, 5 non-std exception in load_state.
    void set_fault(int mode) noexcept { fault_ = mode; }
#endif

private:
    void update_coefficients() noexcept;
    float sample_rate_ = 48000.0f;
    float cutoff_ = 8000.0f, gain_db_ = 0.0f;
    float coeff_ = 0.0f, gain_target_ = 1.0f, gain_slew_ = 0.01f;
    std::vector<float> z_;     // filter memory, one per channel
    std::vector<float> gain_;  // smoothed gain, one per channel
#ifdef OPB_TEST_FAULTS
    int fault_ = 0;
#endif
};

}  // namespace opbridge
