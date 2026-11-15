// SPDX-License-Identifier: MIT
// What the events stub plugin (events_stub.cpp) saw in its last block. The stub
// returns it as its whole plugin state, so a host test reads it through the
// shell's getState trailer. Test builds only.
#pragma once
#include "op_shell.h"

struct EventsProbe {
    uint32_t magic;          // kEventsProbeMagic
    uint32_t have_transport; // the block carried a transport
    OpTransport transport;
    uint32_t offline;        // the offline flag as the stub last saw it
    uint32_t sidechain_channels;
    uint32_t input_channels;
    uint32_t output_channels;
    uint32_t event_count;    // events in the last block
    uint32_t frames;
    uint32_t blocks;         // blocks processed
    uint32_t layout;         // selected layout
};
constexpr uint32_t kEventsProbeMagic = 0x45565054;  // 'EVPT'
