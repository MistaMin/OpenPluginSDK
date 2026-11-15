// SPDX-License-Identifier: MIT
// The shim between dsp.h and cxx. Free functions, so the DSP class stays
// untouched. Rules:
//  - audio-thread functions are noexcept: a throw terminates instead of
//    unwinding into Rust;
//  - everything that may throw is declared `-> Result<..>` on the Rust side and
//    goes through guard(), which turns *any* exception into a std::exception
//    that cxx converts to an Err.
#pragma once
#include <cstddef>
#include <cstdint>
#include <memory>

#include "dsp.h"
#include "rust/cxx.h"

namespace opbridge {

std::unique_ptr<OnePoleGain> new_dsp() noexcept;
void prepare(OnePoleGain& dsp, float sample_rate, std::size_t max_block, std::size_t channels);
void reset(OnePoleGain& dsp) noexcept;
void set_cutoff_hz(OnePoleGain& dsp, float hz) noexcept;
void set_gain_db(OnePoleGain& dsp, float db) noexcept;
float cutoff_hz(const OnePoleGain& dsp) noexcept;
float gain_db(const OnePoleGain& dsp) noexcept;
void process(OnePoleGain& dsp, std::size_t channel, rust::Slice<float> samples) noexcept;
rust::Vec<std::uint8_t> save_state(const OnePoleGain& dsp);
void load_state(OnePoleGain& dsp, rust::Slice<const std::uint8_t> bytes);
// Test hook. A no-op unless built with OPB_TEST_FAULTS (cxx-build does not
// evaluate #[cfg] in the bridge, so the declaration is always there).
void set_fault(OnePoleGain& dsp, std::int32_t mode) noexcept;

}  // namespace opbridge
