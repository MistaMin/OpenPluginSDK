// SPDX-License-Identifier: MIT
// Proves the AAX process callback does not allocate. It counts allocations
// only while armed, and only around calls to the callback. Two counters watch:
//   - the global operator new and delete, which this file replaces;
//   - the malloc family, through libsystem_malloc's malloc_logger hook, which
//     sees malloc, calloc, realloc, valloc, posix_memalign and free from C++,
//     Rust and the SDK alike.
// A self-check allocates while armed to show both counters really count, so the
// test cannot pass because the counting is broken. Not a Pro Tools test.
//
// Run from the native build (see native/CMakeLists.txt):
//   ctest --test-dir build-shell -R aax_alloc_proof --output-on-failure
#include "OpenPluginAAX.h"
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <new>
#include <vector>
#ifdef OP_STUB_PARAMS
// Built over the stand-in ABI (AAXAllocLatencyProof): the ABI 2 paths can be driven there.
#include "host_fakes.h"
#include "proof_stub.h"
#endif

extern "C" {
// libsystem_malloc calls this for every allocation event when it is non-null.
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t skip);
extern malloc_logger_t* malloc_logger;
}

namespace {
std::atomic<bool> g_armed{false};
std::atomic<unsigned long> g_new_events{0};
std::atomic<unsigned long> g_malloc_events{0};

void on_malloc_event(uint32_t, uintptr_t, uintptr_t, uintptr_t, uintptr_t, uint32_t) {
    if (g_armed.load(std::memory_order_relaxed)) g_malloc_events.fetch_add(1, std::memory_order_relaxed);
}
void note_new() {
    if (g_armed.load(std::memory_order_relaxed)) g_new_events.fetch_add(1, std::memory_order_relaxed);
}
void* checked_alloc(std::size_t size) {
    note_new();
    if (void* p = std::malloc(size ? size : 1)) return p;
    throw std::bad_alloc();
}
void release(void* p) {
    if (p) note_new();
    std::free(p);
}
}

void* operator new(std::size_t size) { return checked_alloc(size); }
void* operator new[](std::size_t size) { return checked_alloc(size); }
void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
    note_new();
    return std::malloc(size ? size : 1);
}
void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
    note_new();
    return std::malloc(size ? size : 1);
}
void operator delete(void* p) noexcept { release(p); }
void operator delete[](void* p) noexcept { release(p); }
void operator delete(void* p, std::size_t) noexcept { release(p); }
void operator delete[](void* p, std::size_t) noexcept { release(p); }
void operator delete(void* p, const std::nothrow_t&) noexcept { release(p); }
void operator delete[](void* p, const std::nothrow_t&) noexcept { release(p); }

namespace {
void fail(const char* message) {
    std::fprintf(stderr, "AAX allocation proof failed: %s\n", message);
    std::exit(1);
}
void require(bool ok, const char* message) {
    if (!ok) fail(message);
}

struct Counts {
    unsigned long news;
    unsigned long mallocs;
};
void arm() {
    g_new_events.store(0);
    g_malloc_events.store(0);
    g_armed.store(true, std::memory_order_seq_cst);
}
Counts disarm() {
    g_armed.store(false, std::memory_order_seq_cst);
    return {g_new_events.load(), g_malloc_events.load()};
}

}

// The armed region holds only the callback. Everything else is set up before.
struct Fixture {
    openplugin_aax::ProcessState state;
    openplugin_aax::PacketBuffer packet;
    std::vector<float> inL, inR, outL, outR;
    int32_t frames = 0;
    float* input[2] = {nullptr, nullptr};
    float* output[2] = {nullptr, nullptr};
    openplugin_aax::ProcessState* statePtr = nullptr;
    openplugin_aax::Context context{};
    openplugin_aax::Context* contexts[1] = {nullptr};

    Fixture(uint32_t channels, int32_t blockFrames) : frames(blockFrames) {
        require(state.prepare(48000.0, channels), "prepare");
        const auto n = static_cast<size_t>(blockFrames);
        inL.assign(n, 0.25f);
        inR.assign(n, 0.25f);
        outL.assign(n, 0.f);
        outR.assign(n, 0.f);
        for (size_t i = 0; i < n; ++i) inL[i] = inR[i] = 0.25f * std::sin(0.13f * static_cast<float>(i));
        input[0] = inL.data();
        output[0] = outL.data();
        input[1] = channels > 1 ? inR.data() : nullptr;
        output[1] = channels > 1 ? outR.data() : nullptr;
        OpShellInfo info{};
        require(op_shell_info(&info) != 0 && info.param_count > 0 && info.param_count <= openplugin_aax::kMaxParams, "shell info");
        packet = openplugin_aax::PacketBuffer(info.param_count);
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            require(op_shell_param(i, &desc) != 0, "shell param");
            packet.plain()[i] = static_cast<float>(desc.default_plain);
            lo.push_back(static_cast<float>(desc.min));
            hi.push_back(static_cast<float>(desc.max));
        }
        statePtr = &state;
        context = {input, output, &frames, packet.get(), &statePtr};
        contexts[0] = &context;
    }
    std::vector<float> lo, hi;
    // One armed call of the real process callback. Returns what was counted.
    Counts call() {
        arm();
        openplugin_aax::process(contexts, contexts + 1);
        return disarm();
    }
};

static unsigned long g_total = 0;
static unsigned long g_calls = 0;

static void expect_clean(Counts counts, const char* what) {
    ++g_calls;
    g_total += counts.news + counts.mallocs;
    if (counts.news != 0 || counts.mallocs != 0) {
        std::fprintf(stderr, "AAX allocation proof failed: %s allocated (operator new/delete events %lu, malloc events %lu)\n",
                     what, counts.news, counts.mallocs);
        std::exit(1);
    }
}

#ifdef OP_STUB_PARAMS
static const char* const kExtra = " plus, on the stand-in ABI, MIDI in and out, parameter events, transport, sidechain, a 5.1 stem and a note effect";
#else
static const char* const kExtra = "";
#endif

int main() {
    using namespace openplugin_aax;

    // The malloc hook is only installed for the duration of the proof.
    malloc_logger = on_malloc_event;

    // Self-check: both counters must see an allocation, or nothing below means anything.
    {
        void* volatile sink = nullptr;
        arm();
        sink = std::malloc(64);
        const Counts viaMalloc = disarm();
        std::free(sink);
        require(viaMalloc.mallocs > 0, "self-check: the malloc counter did not see malloc(64); the proof is vacuous");

        arm();
        sink = ::operator new(64);
        const Counts viaNew = disarm();
        ::operator delete(sink);
        require(viaNew.news > 0, "self-check: the operator new counter did not see operator new; the proof is vacuous");

        arm();
        auto* grown = new std::vector<int>(100);
        const Counts viaVector = disarm();
        delete grown;
        require(viaVector.news > 0 && viaVector.mallocs > 0, "self-check: a vector allocation was not counted by both counters");

        // And a disarmed region must count nothing.
        g_new_events.store(0);
        g_malloc_events.store(0);
        sink = std::malloc(64);
        std::free(sink);
        require(g_new_events.load() == 0 && g_malloc_events.load() == 0, "self-check: counters moved while disarmed");
        std::printf("AAX allocation self-check: malloc counted %lu, operator new counted %lu, vector counted new %lu malloc %lu\n",
                    viaMalloc.mallocs, viaNew.news, viaVector.news, viaVector.mallocs);
    }

    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.param_count > 0, "shell info");

    for (uint32_t channels = 1; channels <= 2; ++channels) {
        const char* layout = channels == 1 ? "mono" : "stereo";
        // 512 is a typical block, 1 the smallest, 3000 is split inside the callback.
        for (int32_t blockFrames : {512, 1, 3000}) {
            Fixture rig(channels, blockFrames);
            char what[96];

            // The first call on a fresh instance is counted too, so lazy setup would show.
            std::snprintf(what, sizeof what, "%s %d frames, default packet, first call", layout, blockFrames);
            expect_clean(rig.call(), what);
            std::snprintf(what, sizeof what, "%s %d frames, default packet", layout, blockFrames);
            for (int i = 0; i < 20; ++i) expect_clean(rig.call(), what);

            // Parameter changes between calls: every parameter sweeps its whole range.
            std::snprintf(what, sizeof what, "%s %d frames, parameters changing", layout, blockFrames);
            for (int step = 0; step < 64; ++step) {
                const float t = static_cast<float>(step) / 63.f;
                for (uint32_t p = 0; p < info.param_count; ++p) {
                    const float phase = (p % 2 == 0) ? t : 1.f - t;
                    rig.packet.plain()[p] = rig.lo[p] + (rig.hi[p] - rig.lo[p]) * phase;
                }
                expect_clean(rig.call(), what);
            }

            // Bypass on, off, on, off, with and without changing parameters.
            for (int i = 0; i < 6; ++i) {
                rig.packet.get()->bypass = (i % 2 == 0) ? 1u : 0u;
                for (uint32_t p = 0; p < info.param_count; ++p) rig.packet.plain()[p] = (i % 3 == 0) ? rig.lo[p] : rig.hi[p];
                std::snprintf(what, sizeof what, "%s %d frames, bypass %s", layout, blockFrames, rig.packet.get()->bypass ? "on" : "off");
                expect_clean(rig.call(), what);
            }
            // Switching both ways with a crossfade (480 samples) that spans several
            // calls: the delay line, the dry buffer and the mix all run while armed.
            for (int i = 0; i < 24; ++i) {
                rig.packet.get()->bypass = ((i / 4) % 2 == 0) ? 1u : 0u;
                if (i % 5 == 4) rig.packet.get()->bypass ^= 1u;
                std::snprintf(what, sizeof what, "%s %d frames, bypass crossfade %d", layout, blockFrames, i);
                expect_clean(rig.call(), what);
            }
            // A refused packet while bypassed still outputs the dry path.
            rig.packet.get()->bypass = 1;
            const float keep = rig.packet.plain()[0];
            rig.packet.plain()[0] = std::numeric_limits<float>::quiet_NaN();
            std::snprintf(what, sizeof what, "%s %d frames, bypassed non-finite packet", layout, blockFrames);
            expect_clean(rig.call(), what);
            rig.packet.plain()[0] = keep;
            rig.packet.get()->count = info.param_count + 1;
            std::snprintf(what, sizeof what, "%s %d frames, bypassed oversized packet", layout, blockFrames);
            expect_clean(rig.call(), what);
            rig.packet.get()->count = info.param_count;
            rig.packet.get()->bypass = 0;

            // Refused packets take the silence path, which must not allocate either.
            const float saved = rig.packet.plain()[0];
            rig.packet.plain()[0] = std::numeric_limits<float>::quiet_NaN();
            std::snprintf(what, sizeof what, "%s %d frames, non-finite packet", layout, blockFrames);
            expect_clean(rig.call(), what);
            rig.packet.plain()[0] = saved;
            rig.packet.get()->count = info.param_count + 1;
            std::snprintf(what, sizeof what, "%s %d frames, oversized packet", layout, blockFrames);
            expect_clean(rig.call(), what);
            rig.packet.get()->count = info.param_count;
            expect_clean(rig.call(), what);
        }
    }

#ifdef OP_STUB_PARAMS
    // ABI 2: MIDI in and out, parameter events, the transport, a sidechain, a 5.1 stem and a note effect
    // through the callback, with the counters armed. The fakes are built, and their buffers reserved, first.
    {
        auto bus = [](uint32_t kind, std::vector<uint8_t> roles) {
            OpBus b{};
            b.kind = kind;
            b.channels = static_cast<uint32_t>(roles.size());
            std::memset(b.roles, OP_ROLE_NONE, sizeof b.roles);
            for (size_t i = 0; i < roles.size(); ++i) b.roles[i] = roles[i];
            return b;
        };
        const OpBus st = bus(OP_BUS_STEREO, {0, 1});
        const OpBus five = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5});
        const OpBus side = bus(OP_BUS_MONO, {OP_ROLE_NONE});
        struct Case { const char* what; uint32_t kind, ports; OpLayout layout; AAX_EStemFormat stem; uint32_t inCh, outCh; bool sidechain; };
        OpLayout instrument{}, fx{}, surround{}, note{};
        instrument.output = st;
        fx.input = st; fx.output = st; fx.sidechain = side;
        surround.input = five; surround.output = five;
        const Case cases[] = {
            {"instrument", OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, instrument, AAX_eStemFormat_Stereo, 2, 2, false},
            {"effect with sidechain", OP_KIND_EFFECT, 0, fx, AAX_eStemFormat_Stereo, 3, 2, true},
            {"5.1 effect", OP_KIND_EFFECT, 0, surround, AAX_eStemFormat_5_1, 6, 6, false},
            {"note effect", OP_KIND_NOTE_EFFECT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, note, AAX_eStemFormat_Stereo, 2, 2, false},
        };
        for (const Case& c : cases) {
            op_stub_configure(c.kind, c.ports, 1, &c.layout);
            for (int32_t blockFrames : {512, 3000}) {
                ProcessState state;
                require(state.prepareStems(48000.0, c.stem, c.stem), "prepare (events)");
                PacketBuffer packet(op_stub_param_count());
                std::vector<std::vector<float>> in(c.inCh, std::vector<float>(static_cast<size_t>(blockFrames), 0.1f));
                std::vector<std::vector<float>> out(c.outCh, std::vector<float>(static_cast<size_t>(blockFrames), 0.f));
                std::vector<float*> inPtr, outPtr;
                for (auto& ch : in) inPtr.push_back(ch.data());
                for (auto& ch : out) outPtr.push_back(ch.data());
                FakeTransport transport;
                transport.ticks = 960000 * 12;
                transport.sample = 48000;
                FakeMidiNode midiIn, midiOut, transportNode;
                transportNode.transport = &transport;
                midiOut.reserve(8192);
                std::vector<AAX_CMidiPacket> packets;
                for (int i = 0; i < 400; ++i) {
                    const auto at = static_cast<uint32_t>((i * 37) % blockFrames);
                    packets.push_back(i % 3 == 0 ? midiPacket(at, 3, 0x90, 40 + i % 40, 100) : i % 3 == 1 ? midiPacket(at, 3, 0xB0, 1, i % 128) : midiPacket(at, 3, 0x80, 40 + i % 40, 0));
                }
                midiIn.setInput(packets);
                int32_t sideChain = 2;
                int32_t frames = blockFrames;
                ProcessState* statePtr = &state;
                Context context{inPtr.data(), outPtr.data(), &frames, packet.get(), &statePtr,
                                c.ports & OP_NOTE_PORT_INPUT ? &midiIn : nullptr, c.ports & OP_NOTE_PORT_OUTPUT ? &midiOut : nullptr,
                                &transportNode, c.sidechain ? &sideChain : nullptr};
                Context* contexts[1] = {&context};
                char what[128];
                std::snprintf(what, sizeof what, "%s, %d frames, MIDI, transport, sidechain", c.what, blockFrames);
                for (int i = 0; i < 12; ++i) {
                    packet.plain()[0] = 0.25f + 0.05f * static_cast<float>(i % 7);  // a parameter event nearly every call
                    midiOut.posted.clear();
                    arm();
                    openplugin_aax::process(contexts, contexts + 1);
                    expect_clean(disarm(), what);
                }
                require(c.ports & OP_NOTE_PORT_OUTPUT ? !midiOut.posted.empty() : midiOut.posted.empty(), "MIDI went out when it should");
            }
        }
        OpLayout stereoFx{};
        stereoFx.input = st;
        stereoFx.output = st;
        op_stub_configure(OP_KIND_EFFECT, 0, 1, &stereoFx);
    }
#endif

    malloc_logger = nullptr;
    require(g_total == 0, "allocation total is not zero");
    {
        Fixture probe(1, 1);
        std::printf("AAX allocation proof: reported latency %u samples, %u parameters\n", op_shell_latency(probe.state.engine), info.param_count);
    }
    std::printf("AAX allocation proof: %lu armed process calls (mono and stereo%s; 1, 512 and 3000 frames; default, changing, non-finite and oversized packets; bypass on and off, crossfades across calls, bypassed refused packets), 0 allocations by operator new and by malloc: PASS\n", g_calls, kExtra);
    return 0;
}
