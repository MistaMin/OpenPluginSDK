// SPDX-License-Identifier: MIT
// The AAX state chunk: every plugin parameter value plus the master bypass,
// in a small versioned binary layout. This file does not use the AAX SDK
// beyond its type names, so the proofs can exercise it directly. Fields are
// little-endian.
//
// Version 1 payload (the chunk's fData):
//   u32 magic 'OPST'    u32 version     u32 parameter count
//   u32 layout hash     u32 bypass      f32 plain value x count
// The layout hash is FNV-1a over the parameter count and every parameter id in
// declared order. A chunk written for another parameter layout is refused.
// A later format change adds a version and keeps a reader for version 1.
#pragma once
#include "OpenPluginAAX.h"
#include <cstddef>
#include <cstdint>
namespace openplugin_aax::state {
inline constexpr AAX_CTypeID kChunkId = 'OpSt';
inline constexpr uint32_t kMagic = 0x5453504Fu;  // bytes 'O','P','S','T'
inline constexpr uint32_t kVersion = 1;
inline constexpr size_t kHeaderBytes = 5 * sizeof(uint32_t);
struct Values {
    float plain[kMaxParams];
    uint32_t count;
    bool bypass;
};
enum class Status { ok, badSize, badMagic, badVersion, badLayout, badValue };
const char* describe(Status status);
// Bytes in a chunk for the current ABI parameter table; 0 if the table is unusable.
size_t encodedSize();
// Writes exactly encodedSize() bytes. False if `capacity` differs or the table is unusable.
bool encode(const Values& values, uint8_t* out, size_t capacity);
// Checks the whole payload against the current ABI table and writes `out` only on success.
// Every value must be finite and inside its parameter's range.
Status decode(const uint8_t* data, size_t size, Values& out);
}
