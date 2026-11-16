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
//
// Version 2 payload: the version 1 layout with version = 2, followed by
//   u32 blob length     u8 blob x length
// The blob is exactly what op_shell_save_state wrote: the plugin's parameters
// plus its own chunk, one opaque unit. The blob length is at least 1 and the
// payload ends with the blob. The shell writes version 2 and reads versions 1
// and 2. A version 1 chunk loads its parameters and the bypass and leaves the
// plugin's own chunk alone.
//
// A plugin with identity pins (OP_IDENTITY_KEYED_STATE, ABI 8) may reorder its
// parameter table between releases; its blob names every value by parameter.
// A version 2 chunk whose count or layout hash is not this build's is then read
// through its blob alone (Values::byBlob): the positional values are skipped,
// the blob is loaded, and the parameters are read back from the engine. Without
// pins such a chunk is refused, as before.
#pragma once
#include "OpenPluginAAX.h"
#include <cstddef>
#include <cstdint>
namespace openplugin_aax::state {
inline constexpr AAX_CTypeID kChunkId = 'OpSt';
inline constexpr uint32_t kMagic = 0x5453504Fu;  // bytes 'O','P','S','T'
inline constexpr uint32_t kVersion1 = 1;
inline constexpr uint32_t kVersion = 2;  // written by the shell
inline constexpr size_t kHeaderBytes = 5 * sizeof(uint32_t);
struct Values {
    float plain[kMaxParams];
    uint32_t count;
    bool bypass;
    uint32_t version;       // set by decode
    const uint8_t* blob;    // set by decode for version 2: points into the decoded data
    size_t blobSize;
    bool byBlob;            // set by decode: another layout's chunk; `plain` is not filled
};
enum class Status { ok, badSize, badMagic, badVersion, badLayout, badValue };
const char* describe(Status status);
// Bytes in a version 2 chunk carrying `blobBytes` of plugin state; 0 if the table is
// unusable or `blobBytes` is 0.
size_t encodedSize(size_t blobBytes);
// Bytes in a version 1 chunk; 0 if the table is unusable.
size_t encodedSizeV1();
// Writes exactly encodedSize(blobBytes) bytes. False if `capacity` differs, the blob is
// empty or null, or the table is unusable.
bool encode(const Values& values, const uint8_t* blob, size_t blobBytes, uint8_t* out, size_t capacity);
// Writes a version 1 chunk (what the shell wrote before version 2), for readers and proofs.
bool encodeV1(const Values& values, uint8_t* out, size_t capacity);
// Checks the whole payload against the current ABI table and writes `out` only on success.
// Every value must be finite and inside its parameter's range. A version 2 `out.blob`
// points into `data`.
Status decode(const uint8_t* data, size_t size, Values& out);
}
