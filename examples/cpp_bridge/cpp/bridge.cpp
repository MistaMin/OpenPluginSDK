// SPDX-License-Identifier: MIT
#include "bridge.h"

#include <exception>
#include <stdexcept>
#include <utility>

namespace opbridge {

namespace {
// Runs f; anything it throws leaves as a std::exception (cxx catches those and
// returns Err on the Rust side). A non-std throw would otherwise terminate.
template <class F>
auto guard(F&& f) -> decltype(f()) {
    try {
        return f();
    } catch (const std::exception&) {
        throw;
    } catch (...) {
        throw std::runtime_error("unknown (non-std) C++ exception");
    }
}
}  // namespace

std::unique_ptr<OnePoleGain> new_dsp() noexcept { return std::make_unique<OnePoleGain>(); }

void prepare(OnePoleGain& dsp, float sample_rate, std::size_t max_block, std::size_t channels) {
    guard([&] { dsp.prepare(sample_rate, max_block, channels); });
}
void reset(OnePoleGain& dsp) noexcept { dsp.reset(); }
void set_cutoff_hz(OnePoleGain& dsp, float hz) noexcept { dsp.set_cutoff_hz(hz); }
void set_gain_db(OnePoleGain& dsp, float db) noexcept { dsp.set_gain_db(db); }
float cutoff_hz(const OnePoleGain& dsp) noexcept { return dsp.cutoff_hz(); }
float gain_db(const OnePoleGain& dsp) noexcept { return dsp.gain_db(); }

void process(OnePoleGain& dsp, std::size_t channel, rust::Slice<float> samples) noexcept {
    dsp.process(channel, samples.data(), samples.size());
}

rust::Vec<std::uint8_t> save_state(const OnePoleGain& dsp) {
    return guard([&] {
        const auto bytes = dsp.save_state();
        rust::Vec<std::uint8_t> out;
        out.reserve(bytes.size());
        for (auto b : bytes) out.push_back(b);
        return out;
    });
}

void load_state(OnePoleGain& dsp, rust::Slice<const std::uint8_t> bytes) {
    guard([&] { dsp.load_state(bytes.data(), bytes.size()); });
}

void set_fault(OnePoleGain& dsp, std::int32_t mode) noexcept {
#ifdef OPB_TEST_FAULTS
    dsp.set_fault(mode);
#else
    (void)dsp;
    (void)mode;
#endif
}

}  // namespace opbridge
