// SPDX-License-Identifier: MIT
#include "OpenPluginAAXState.h"
#include <cmath>
#include <cstring>
namespace openplugin_aax::state {
namespace {
void put32(uint8_t* out, uint32_t value) {
    for (int i = 0; i < 4; ++i) out[i] = static_cast<uint8_t>(value >> (8 * i));
}
uint32_t get32(const uint8_t* in) {
    uint32_t value = 0;
    for (int i = 0; i < 4; ++i) value |= static_cast<uint32_t>(in[i]) << (8 * i);
    return value;
}
// Count, ranges and layout hash of the table the ABI exposes now.
struct Table {
    uint32_t count = 0;
    uint32_t hash = 0;
    float min[kMaxParams] = {};
    float max[kMaxParams] = {};
    bool valid = false;
};
Table readTable() {
    Table table;
    OpShellInfo info{};
    if (!op_shell_info(&info) || info.abi != OP_SHELL_ABI || info.param_count == 0 || info.param_count > kMaxParams) return table;
    uint32_t hash = 2166136261u;
    const auto mix = [&](uint8_t byte) { hash = (hash ^ byte) * 16777619u; };
    for (int i = 0; i < 4; ++i) mix(static_cast<uint8_t>(info.param_count >> (8 * i)));
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        if (!op_shell_param(i, &desc)) return table;
        for (const char* c = desc.id; *c; ++c) mix(static_cast<uint8_t>(*c));
        mix(0);
        table.min[i] = static_cast<float>(desc.min);
        table.max[i] = static_cast<float>(desc.max);
    }
    table.count = info.param_count;
    table.hash = hash;
    table.valid = true;
    return table;
}
}
const char* describe(Status status) {
    switch (status) {
        case Status::ok: return "ok";
        case Status::badSize: return "wrong length";
        case Status::badMagic: return "not an OpenPlugin chunk";
        case Status::badVersion: return "unknown version";
        case Status::badLayout: return "different parameter layout";
        case Status::badValue: return "value not finite or out of range";
    }
    return "unknown";
}
size_t encodedSizeV1() {
    const Table table = readTable();
    return table.valid ? kHeaderBytes + table.count * sizeof(float) : 0;
}
size_t encodedSize(size_t blobBytes) {
    const Table table = readTable();
    if (!table.valid || blobBytes == 0 || blobBytes > UINT32_MAX) return 0;
    return kHeaderBytes + table.count * sizeof(float) + sizeof(uint32_t) + blobBytes;
}
namespace {
bool writeHeader(const Table& table, const Values& values, uint32_t version, uint8_t* out) {
    if (values.count != table.count) return false;
    put32(out, kMagic);
    put32(out + 4, version);
    put32(out + 8, table.count);
    put32(out + 12, table.hash);
    put32(out + 16, values.bypass ? 1u : 0u);
    for (uint32_t i = 0; i < table.count; ++i) {
        uint32_t bits = 0;
        std::memcpy(&bits, &values.plain[i], sizeof bits);
        put32(out + kHeaderBytes + i * 4, bits);
    }
    return true;
}
}
bool encodeV1(const Values& values, uint8_t* out, size_t capacity) {
    const Table table = readTable();
    if (!table.valid || !out || capacity != kHeaderBytes + table.count * sizeof(float)) return false;
    return writeHeader(table, values, kVersion1, out);
}
bool encode(const Values& values, const uint8_t* blob, size_t blobBytes, uint8_t* out, size_t capacity) {
    const Table table = readTable();
    if (!table.valid || !out || !blob || blobBytes == 0 || blobBytes > UINT32_MAX) return false;
    const size_t fixed = kHeaderBytes + table.count * sizeof(float);
    if (capacity != fixed + sizeof(uint32_t) + blobBytes || !writeHeader(table, values, kVersion, out)) return false;
    put32(out + fixed, static_cast<uint32_t>(blobBytes));
    std::memcpy(out + fixed + sizeof(uint32_t), blob, blobBytes);
    return true;
}
Status decode(const uint8_t* data, size_t size, Values& out) {
    if (!data || size < kHeaderBytes) return Status::badSize;
    if (get32(data) != kMagic) return Status::badMagic;
    const uint32_t version = get32(data + 4);
    const Table table = readTable();
    if (!table.valid) return Status::badLayout;
    if (version != kVersion1 && version != kVersion) return Status::badVersion;
    const uint32_t count = get32(data + 8);
    if (count != table.count || get32(data + 12) != table.hash) return Status::badLayout;
    const size_t fixed = kHeaderBytes + static_cast<size_t>(count) * sizeof(float);
    size_t blobSize = 0;
    if (version == kVersion1) {
        if (size != fixed) return Status::badSize;
    } else {
        if (size < fixed + sizeof(uint32_t)) return Status::badSize;
        blobSize = get32(data + fixed);
        if (blobSize == 0 || size - fixed - sizeof(uint32_t) != blobSize) return Status::badSize;
    }
    const uint32_t bypass = get32(data + 16);
    if (bypass > 1) return Status::badValue;
    Values next{};
    next.count = count;
    next.bypass = bypass == 1;
    next.version = version;
    for (uint32_t i = 0; i < count; ++i) {
        const uint32_t bits = get32(data + kHeaderBytes + i * 4);
        float value = 0.f;
        std::memcpy(&value, &bits, sizeof value);
        if (!std::isfinite(value) || value < table.min[i] || value > table.max[i]) return Status::badValue;
        next.plain[i] = value;
    }
    if (version == kVersion) {
        next.blob = data + fixed + sizeof(uint32_t);
        next.blobSize = blobSize;
    }
    out = next;
    return Status::ok;
}
}
