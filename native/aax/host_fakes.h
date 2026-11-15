// SPDX-License-Identifier: MIT
// Stand-ins for what Pro Tools gives the process callback: a MIDI node and the
// transport behind a transport node. Test code only. The transport counts every
// call beyond the version 1 interface (AAX_IACFTransport), because the node's
// interface is not versioned and the shell must not call what an older host
// lacks.
#pragma once
#include "AAX_IMIDINode.h"
#include "AAX_ITransport.h"
#include "AAX_Errors.h"
#include <cstdint>
#include <vector>

struct FakeTransport final : AAX_ITransport {
    bool available = true;      // false: every call answers AAX_ERROR_UNIMPLEMENTED
    bool playing = true;
    double tempo = 120.0;
    int32_t numerator = 3, denominator = 4;
    int64_t ticks = 0;
    int64_t sample = 0;
    mutable unsigned v1Calls = 0;
    mutable unsigned laterCalls = 0;  // methods the shell does not use (all version 2 and later ones, and the unused version 1 ones)
    AAX_Result v1() const {
        ++v1Calls;
        return available ? AAX_SUCCESS : AAX_ERROR_UNIMPLEMENTED;
    }
    AAX_Result later() const {
        ++laterCalls;
        return AAX_ERROR_UNIMPLEMENTED;
    }
    AAX_Result GetCurrentTempo(double* t) const override { *t = tempo; return v1(); }
    AAX_Result GetCurrentMeter(int32_t* n, int32_t* d) const override { *n = numerator; *d = denominator; return v1(); }
    AAX_Result IsTransportPlaying(bool* p) const override { *p = playing; return v1(); }
    AAX_Result GetCurrentTickPosition(int64_t* t) const override { *t = ticks; return v1(); }
    AAX_Result GetCurrentLoopPosition(bool*, int64_t*, int64_t*) const override { return later(); }
    AAX_Result GetCurrentNativeSampleLocation(int64_t* s) const override { *s = sample; return v1(); }
    AAX_Result GetCustomTickPosition(int64_t*, int64_t) const override { return later(); }
    AAX_Result GetBarBeatPosition(int32_t*, int32_t*, int64_t*, int64_t) const override { return later(); }
    AAX_Result GetTicksPerQuarter(uint32_t*) const override { return later(); }
    AAX_Result GetCurrentTicksPerBeat(uint32_t*) const override { return later(); }
    AAX_Result GetTimelineSelectionStartPosition(int64_t*) const override { return later(); }
    AAX_Result GetTimeCodeInfo(AAX_EFrameRate*, int32_t*) const override { return later(); }
    AAX_Result GetFeetFramesInfo(AAX_EFeetFramesRate*, int64_t*) const override { return later(); }
    AAX_Result IsMetronomeEnabled(int32_t*) const override { return later(); }
    AAX_Result GetHDTimeCodeInfo(AAX_EFrameRate*, int64_t*) const override { return later(); }
    AAX_Result RequestTransportStart() override { return later(); }
    AAX_Result RequestTransportStop() override { return later(); }
    AAX_Result GetTimelineSelectionEndPosition(int64_t*) const override { return later(); }
    AAX_Result GetKeySignature(int64_t, uint32_t*) const override { return later(); }
};

// A local MIDI node. As an input it holds the packets the host gives; as an output it
// records what the shell posted. The buffers are sized up front so the callback under
// test allocates nothing through this object.
struct FakeMidiNode final : AAX_IMIDINode {
    std::vector<AAX_CMidiPacket> in;
    std::vector<AAX_CMidiPacket> posted;
    AAX_CMidiStream stream{0, nullptr};
    FakeTransport* transport = nullptr;
    bool nullBuffer = false;
    void reserve(size_t n) { posted.reserve(n); }
    void setInput(std::vector<AAX_CMidiPacket> packets) {
        in = std::move(packets);
        stream.mBufferSize = static_cast<uint32_t>(in.size());
        stream.mBuffer = in.data();
    }
    AAX_CMidiStream* GetNodeBuffer() override { return nullBuffer ? nullptr : &stream; }
    AAX_Result PostMIDIPacket(AAX_CMidiPacket* packet) override {
        posted.push_back(*packet);
        return AAX_SUCCESS;
    }
    AAX_ITransport* GetTransport() override { return transport; }
};

inline AAX_CMidiPacket midiPacket(uint32_t timestamp, uint32_t length, unsigned a, unsigned b = 0, unsigned c = 0) {
    AAX_CMidiPacket p{};
    p.mTimestamp = timestamp;
    p.mLength = length;
    p.mData[0] = static_cast<unsigned char>(a);
    p.mData[1] = static_cast<unsigned char>(b);
    p.mData[2] = static_cast<unsigned char>(c);
    p.mIsImmediate = timestamp == 0;
    return p;
}
