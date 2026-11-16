// SPDX-License-Identifier: MIT
// What the probe plugin (probe_shell.cpp) tells the Audio Unit events proof about the last
// blocks it processed. The proof reads it through a symbol the test bundle exports. Test code.
#pragma once
#include "op_shell.h"

enum { kProbeMaxEvents = 1024, kProbeMaxChannels = 16 };

struct ProbeReport {
    uint32_t created;          // instances created since the last reset
    uint32_t blocks;           // op_shell_process_events calls
    uint32_t last_frames;
    uint32_t input_channels;   // as the last block carried them
    uint32_t output_channels;
    uint32_t sidechain_channels;
    uint32_t layout_selected;
    uint32_t offline;          // the last op_shell_set_offline value
    uint32_t have_transport;   // the last block had a transport
    OpTransport transport;
    // The first sample of each canonical input channel in the last block, and of the sidechain.
    float input_first[kProbeMaxChannels];
    float sidechain_first[kProbeMaxChannels];
    // Every event the plugin received since the last reset, in the order it arrived.
    uint32_t event_total;
    uint32_t event_block[kProbeMaxEvents];  // which block it came in
    OpEvent events[kProbeMaxEvents];
};

extern "C" {
const ProbeReport* op_probe_report(void);
void op_probe_reset(void);
}
