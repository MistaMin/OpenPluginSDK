// SPDX-License-Identifier: MIT
// Test hooks of the stand-in ABI in proof_stub.cpp. Not part of the shell ABI.
#pragma once
#include "op_shell.h"
#ifdef __cplusplus
extern "C" {
#endif
// What the stand-in saw. Positions in `log` are in frames since the instance was created, so a
// block the shell split in pieces still shows each event at its place in the AAX buffer.
enum { OP_STUB_LOG = 512, OP_STUB_CHANNELS = 16 };
typedef struct StubLogEvent {
    uint64_t position;
    uint32_t kind;
    uint32_t index;
    float value;
    uint8_t channel, note, velocity, midi[4];
} StubLogEvent;
typedef struct StubSeen {
    uint32_t calls;              // op_shell_process_events calls
    uint32_t refused;            // calls whose channel counts did not match the layout
    uint32_t frames;             // the last call
    uint32_t inputChannels, outputChannels, sidechainChannels;
    uint32_t transportCalls;     // calls with a transport
    uint32_t transportMissing;   // calls without one
    OpTransport transport;       // the last one
    uint32_t offline;
    uint32_t logCount;
    StubLogEvent log[OP_STUB_LOG];
    float input0[OP_STUB_CHANNELS];     // sample 0 of each canonical input channel, last call
    float sidechain0;                   // sample 0 of the sidechain, last call; 0 if none
    uint64_t frameCounter;
} StubSeen;
// The plugin's kind, note ports and layouts (default: effect, no notes, stereo in and out).
void op_stub_configure(uint32_t kind, uint32_t ports, uint32_t layoutCount, const OpLayout* layouts);
// The version op_shell_abi_version reports (default OP_SHELL_ABI).
void op_stub_set_abi(uint32_t version);
// Output = input * gain + 100 * (canonical channel + 1) when on: shows which canonical channel each AAX channel reached.
void op_stub_tag(uint32_t on);
// OpShellInfo.category (default "Fx").
void op_stub_set_category(const char* category);
void op_stub_seen(void* instance, StubSeen* out);
// ABI 6: give the engine a new latency (raises OP_CHANGE_LATENCY when it moves), raise any
// OP_CHANGE_* bits, and count the op_shell_take_changes calls so far.
void op_stub_set_latency(void* instance, uint32_t samples);
void op_stub_raise_changes(void* instance, uint32_t flags);
uint32_t op_stub_takes(void);
uint32_t op_stub_param_count(void);
#ifdef __cplusplus
}
#endif
