// SPDX-License-Identifier: MIT
#pragma once
#include "AAX.h"
#include "op_shell.h"
#include <atomic>
#include <cstdint>
class AAX_CEffectParameters;
class AAX_IEffectGUI;
namespace openplugin_aax {
inline constexpr uint32_t kMaxParams = OP_SHELL_AAX_MAX_PARAMS;
inline constexpr uint32_t kMaxBlock = 2048;
// GetCustomData block the view reads: the engine pointer, valid in this process.
inline constexpr AAX_CTypeID kEngineData = 'OpEn';
struct Packet {
    float plain[kMaxParams];
    uint32_t count;
    uint32_t bypass;
};
struct ProcessState {
    void* engine = nullptr;
    uint32_t channels = 2;
    uint32_t sampleRate = 48000;
    uint32_t paramCount = 0;
    std::atomic<bool> prepared{false};
    ProcessState() = default;
    ProcessState(const ProcessState&) = delete;
    ProcessState& operator=(const ProcessState&) = delete;
    ~ProcessState();
    bool prepare(double sampleRate, uint32_t channelCount);
};
struct Context {
    float** input;
    float** output;
    const int32_t* frames;
    const Packet* controls;
    ProcessState** state;
};
enum Field : AAX_CFieldIndex {
    inputField = AAX_FIELD_INDEX(Context, input),
    outputField = AAX_FIELD_INDEX(Context, output),
    framesField = AAX_FIELD_INDEX(Context, frames),
    controlsField = AAX_FIELD_INDEX(Context, controls),
    stateField = AAX_FIELD_INDEX(Context, state)
};
void AAX_CALLBACK process(Context* const begin[], const void* end);
AAX_CEffectParameters* AAX_CALLBACK createParameters();
AAX_IEffectGUI* AAX_CALLBACK createGUI();
}
