// SPDX-License-Identifier: MIT
// Proves the AAX shell shows the plugin's own parameter text. It runs the shell's real
// AAX_CEffectParameters (no AAX controller) and asks it what Pro Tools asks: the text of a
// normalized value (GetParameterStringFromValue, with the character limit the host passes)
// and the normalized value of a typed text (GetParameterValueFromString).
//
// Built twice (native/CMakeLists.txt):
//  - AAXTextProof, over the example plugin (Gain -24..24 dB, Warm toggle): Gain at +6 dB reads
//    "6.0 dB", "6 dB" reads back as +6 dB, Warm reads Off and On.
//  - AAXTextStubProof (OP_TEXT_STUB), over a stand-in ABI (proof_stub.cpp, OP_STUB_TEXT) with a
//    choice, a toggle and a multi-byte text: labels read, short limits truncate safely.
// Not a Pro Tools test: Pro Tools itself does not take part.
//
//   ctest --test-dir build-shell -R aax_text_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CString.h"
#ifdef OP_TEXT_STUB
#include "proof_stub.h"
#endif
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
using namespace openplugin_aax;

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX parameter text proof failed: %s\n", message);
        std::exit(1);
    }
}
static std::string text(AAX_CEffectParameters* p, uint32_t index, double normalized, int32_t max = 256) {
    std::string out;
    require(proofText(p, index, normalized, max, &out), "text of a value");
    return out;
}
static double value(AAX_CEffectParameters* p, uint32_t index, const char* typed) {
    double normalized = -1.0;
    require(proofValue(p, index, typed, &normalized), typed);
    return normalized;
}
static bool refused(AAX_CEffectParameters* p, uint32_t index, const char* typed) {
    double normalized = -1.0;
    return !proofValue(p, index, typed, &normalized);
}
// A valid UTF-8 string: no stray continuation byte, no cut sequence.
static bool utf8(const std::string& s) {
    for (size_t i = 0; i < s.size();) {
        const unsigned char c = static_cast<unsigned char>(s[i]);
        const size_t n = c < 0x80 ? 1 : (c >> 5) == 6 ? 2 : (c >> 4) == 14 ? 3 : (c >> 3) == 30 ? 4 : 0;
        if (n == 0 || i + n > s.size()) return false;
        for (size_t k = 1; k < n; ++k)
            if ((static_cast<unsigned char>(s[i + k]) & 0xC0) != 0x80) return false;
        i += n;
    }
    return true;
}

int main() {
    std::unique_ptr<AAX_CEffectParameters> params(createParameters());
    require(params && proofInit(params.get(), 48000.0, 2), "parameters build without a controller");
    AAX_CEffectParameters* p = params.get();

    // Gain, the example's -24..24 dB: normalized 0.625 is +6 dB.
    require(text(p, 0, 0.625) == "6.0 dB", "Gain at +6 dB reads the plugin's text");
    require(text(p, 0, 0.0) == "-24.0 dB" && text(p, 0, 1.0) == "24.0 dB", "the ends read in dB");
    require(text(p, 0, 0.5) == "0.0 dB", "0 dB");
    require(std::fabs(value(p, 0, "6 dB") - 0.625) < 1e-6, "\"6 dB\" reads as +6 dB");
    require(std::fabs(value(p, 0, "6.0 dB") - 0.625) < 1e-6 && std::fabs(value(p, 0, "-24") - 0.0) < 1e-6, "typed values");
    require(std::fabs(value(p, 0, "100 dB") - 1.0) < 1e-6, "a typed value past the range is clamped");
    require(refused(p, 0, "loud") && refused(p, 0, ""), "text that is not a value is refused");
#ifndef OP_TEXT_STUB
    const std::string hostile(5000, '9');
    require(refused(p, 0, hostile.c_str()), "text longer than the ABI takes is refused");
#endif
    // The shell's text equals the plugin's, through shellText as well.
    require(shellText(0, 6.0) == "6.0 dB", "shellText");

    // Short text. Shorter requests shorten from the full "6.0 dB" and never exceed the limit.
    require(text(p, 0, 0.625, 6) == "6.0 dB", "a limit that fits keeps the text");
    require(text(p, 0, 0.625, 5) == "6.0dB", "one short: the space goes");
    require(text(p, 0, 0.625, 3) == "6.0", "short: the number alone");
    require(text(p, 0, 0.625, 2) == "6.", "shorter: cut");
    require(text(p, 0, 0.625, 0).empty(), "a limit of 0 is empty");
    require(text(p, 0, 0.0, 7) == "-24.0dB", "-24 dB without its space");
    for (int32_t limit = 0; limit <= 12; ++limit) {
        require(text(p, 0, 0.0, limit).size() <= static_cast<size_t>(limit), "never longer than asked");
        require(text(p, 0, 0.625, limit).size() <= static_cast<size_t>(limit), "never longer than asked (+6)");
    }
    // GetParameterValueString (the other host entry) honours the limit too.
    {
        #ifdef OP_TEXT_STUB
        const char* id = "p0";
#else
        const char* id = "gain";
#endif
        AAX_CString s;
        require(p->GetParameterValueString(id, &s, 3) == AAX_SUCCESS && std::strlen(s.Get()) <= 3, "GetParameterValueString honours the limit");
        AAX_CString full;
        require(p->GetParameterValueString(id, &full, 64) == AAX_SUCCESS && std::strcmp(full.Get(), "0.0 dB") == 0, "GetParameterValueString at the default");
    }
    // The pure helper, with multi-byte text and edge limits.
    {
        std::string out;
        require(!fitText("6.0 dB", -1, &out), "a negative limit is refused");
        require(fitText("Größe", 100, &out) && out == "Größe", "fits");
        for (int32_t limit = 0; limit <= 8; ++limit) {
            require(fitText("Größe 日本語", limit, &out) && out.size() <= static_cast<size_t>(limit) && utf8(out), "cut on a UTF-8 boundary");
            require(fitText("日本語", limit, &out) && out.size() <= static_cast<size_t>(limit) && utf8(out) && std::string("日本語").compare(0, out.size(), out) == 0, "a cut is a prefix");
        }
        require(fitText("日本語", 4, &out) && out == "日", "three bytes per character, 4 bytes: one character");
        require(fitText("日本語", 2, &out) && out.empty(), "2 bytes: no whole character");
        require(fitText("Plate  ", 5, &out) && out == "Plate", "trailing spaces dropped from a cut");
        require(fitText("-6.0 dB", 4, &out) && out == "-6.0", "a signed number alone");
    }

#ifdef OP_TEXT_STUB
    require(op_stub_param_count() == 4, "the stand-in has four parameters");
    // A choice reads its labels; the taper is linear over 0..2.
    require(text(p, 1, 0.0) == "Hall" && text(p, 1, 0.5) == "Plate" && text(p, 1, 1.0) == "Room", "choice labels");
    require(std::fabs(value(p, 1, "Plate") - 0.5) < 1e-6 && std::fabs(value(p, 1, "room") - 1.0) < 1e-6, "choice labels read back");
    require(refused(p, 1, "Cathedral"), "an unknown label is refused");
    require(text(p, 1, 0.5, 3) == "Pla", "a short label is cut");
    // A toggle reads Off and On.
    require(text(p, 2, 0.0) == "Off" && text(p, 2, 1.0) == "On", "toggle labels");
    require(std::fabs(value(p, 2, "on") - 1.0) < 1e-6 && std::fabs(value(p, 2, "Off")) < 1e-6, "toggle labels read back");
    require(text(p, 2, 1.0, 1) == "O", "a one-character toggle");
    // Multi-byte text never ends inside a character, whatever the limit.
    require(text(p, 3, 0.5) == "日本語 0.5", "multi-byte text");
    for (int32_t limit = 0; limit <= 14; ++limit) {
        const std::string cut = text(p, 3, 0.5, limit);
        require(cut.size() <= static_cast<size_t>(limit) && utf8(cut), "multi-byte text cut on a boundary");
    }
    std::puts("AAX parameter text proof (stand-in ABI): choice and toggle labels read and parse, short limits cut on a UTF-8 boundary and never exceed the limit. Not a Pro Tools test: PASS");
#else
    // Warm, the example's toggle: Off and On, and the words read back.
    require(text(p, 1, 0.0) == "Off" && text(p, 1, 1.0) == "On", "Warm labels");
    require(std::fabs(value(p, 1, "on") - 1.0) < 1e-6 && std::fabs(value(p, 1, "Off")) < 1e-6, "Warm labels read back");
    require(refused(p, 1, "maybe"), "an unknown toggle word is refused");
    require(text(p, 1, 1.0, 1) == "O", "a one-character toggle");
    std::puts("AAX parameter text proof (example plugin): Gain at +6 dB reads \"6.0 dB\" and \"6 dB\" reads as +6 dB, the ends read in dB, Warm reads Off and On, short limits shorten without exceeding the limit. Not a Pro Tools test: PASS");
#endif
    return 0;
}
