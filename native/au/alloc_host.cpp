// SPDX-License-Identifier: MIT
// Proof that AudioUnitRender of the shell does not allocate. Every allocation
// in the process is counted while the counter is armed, and it is armed only
// around the render calls. The count has two sources:
//   - the global operator new/delete of this executable, and
//   - dyld interposing of the malloc family (malloc, calloc, realloc, valloc,
//     posix_memalign, aligned_alloc, malloc_zone_*) by the library in
//     alloc_hook.cpp, which reaches the bundle, the Rust archive and the
//     system frameworks, because they all call malloc.
// A self-check first proves the counter sees an allocation from each path,
// including one made by Rust and one made inside a system framework, so the
// proof cannot pass because the counter is blind.
//
//   ctest --test-dir build-shell -R au_alloc_proof --output-on-failure
#include "alloc_hook.h"
#include "proof_au.h"
#include <cstdint>
#include <cstring>
#include <malloc/malloc.h>
#include <new>
#include <vector>

namespace {
struct Counts { uint64_t allocs, frees; };
Counts counts() { return {op_alloc_count(), op_free_count()}; }
void arm() { op_alloc_arm(); }
void disarm() { op_alloc_disarm(); }
}
inline void note_alloc() { op_alloc_note(); }
inline void note_free(void* p) { op_alloc_note_free(p); }

// Global operator new/delete of this executable. They count first and then
// allocate through malloc, which is itself counted by the interposer when
// the caller is another image; a single allocation may count twice, never zero.
void* operator new(size_t n) {
    note_alloc();
    if (void* p = malloc(n ? n : 1)) return p;
    throw std::bad_alloc();
}
void* operator new[](size_t n) { return operator new(n); }
void* operator new(size_t n, const std::nothrow_t&) noexcept { note_alloc(); return malloc(n ? n : 1); }
void* operator new[](size_t n, const std::nothrow_t&) noexcept { note_alloc(); return malloc(n ? n : 1); }
void* operator new(size_t n, std::align_val_t a) {
    note_alloc();
    void* p = nullptr;
    if (posix_memalign(&p, std::max(static_cast<size_t>(a), sizeof(void*)), n ? n : 1) != 0) throw std::bad_alloc();
    return p;
}
void operator delete(void* p) noexcept { note_free(p); free(p); }
void operator delete[](void* p) noexcept { note_free(p); free(p); }
void operator delete(void* p, size_t) noexcept { note_free(p); free(p); }
void operator delete[](void* p, size_t) noexcept { note_free(p); free(p); }
void operator delete(void* p, std::align_val_t) noexcept { note_free(p); free(p); }

namespace {
// Runs `body` armed and returns how many allocations it made.
template <typename F>
uint64_t allocations_during(F&& body) {
    const Counts before = counts();
    arm();
    body();
    disarm();
    return counts().allocs - before.allocs;
}

// The compiler may remove an allocation whose result is never observed, and then
// the self-check would test nothing. Passing the pointer through an empty asm
// statement makes it observed.
void escape(void* p) { asm volatile("" : : "g"(p) : "memory"); }

// The counter must see each kind of allocation, or a pass means nothing.
void self_check() {
    void* p = nullptr;
    CHECK(allocations_during([&] { p = malloc(64); escape(p); }) >= 1);
    free(p);
    CHECK(allocations_during([&] { p = calloc(4, 16); escape(p); }) >= 1);
    free(p);
    CHECK(allocations_during([&] { p = realloc(nullptr, 64); escape(p); }) >= 1);
    free(p);
    CHECK(allocations_during([&] { p = strdup("openplugin"); escape(p); }) >= 1);
    free(p);
    CHECK(allocations_during([&] { p = CFAllocatorAllocate(kCFAllocatorSystemDefault, 64, 0); escape(p); }) >= 1);
    CFAllocatorDeallocate(kCFAllocatorSystemDefault, p);
    char* chars = nullptr;
    CHECK(allocations_during([&] { chars = new char[64]; escape(chars); }) >= 1);
    delete[] chars;
    CHECK(allocations_during([&] { std::vector<int> v; v.push_back(1); escape(v.data()); }) >= 1);
    // An allocation made inside a system framework, in another image.
    CFDataRef data = nullptr;
    CHECK(allocations_during([&] { data = CFDataCreate(nullptr, reinterpret_cast<const UInt8*>("openplugin"), 10); escape(const_cast<void*>(static_cast<const void*>(data))); }) >= 1);
    CFRelease(data);
    // An allocation made by the Rust archive.
    void* engine = nullptr;
    CHECK(allocations_during([&] { engine = op_shell_create(48000.0, 512, 2); escape(engine); }) >= 1);
    op_shell_destroy(engine);
    // Disarmed means not counted, and a free is counted while armed.
    const Counts before = counts();
    void* quiet = strdup("quiet");
    escape(quiet);
    free(quiet);
    CHECK(counts().allocs == before.allocs && counts().frees == before.frees);
    void* loud = strdup("loud");
    escape(loud);
    arm();
    free(loud);
    disarm();
    CHECK(counts().frees == before.frees + 1);
    std::puts("AU alloc proof self-check: libc, operator new, std::vector, CoreFoundation and Rust allocations are all counted");
}

void run_phase(AudioUnit unit, ProofBuffers& buffers, const char* label, const char* phase, UInt32 frames, int blocks, double& clock, int param_id, int param_every) {
    uint64_t allocs = 0;
    uint64_t renders = 0;
    const Counts before = counts();
    for (int i = 0; i < blocks; ++i) {
        if (param_every > 0 && i % param_every == 0) {
            // A parameter change from the message thread, then a render that must apply it.
            const float value = (i / param_every) % 2 == 0 ? -6.0f : 3.0f;
            CHECK(AudioUnitSetParameter(unit, static_cast<AudioUnitParameterID>(param_id), kAudioUnitScope_Global, 0, value, 0) == noErr);
        }
        allocs += allocations_during([&] { CHECK(proof_render(unit, buffers, frames, clock) == noErr); });
        ++renders;
        clock += frames;
        double energy = 0.0;
        for (UInt32 ch = 0; ch < buffers.channels; ++ch) {
            for (UInt32 n = 0; n < frames; ++n) {
                CHECK(std::isfinite(buffers.data[ch][n]));
                energy += std::fabs(buffers.data[ch][n]);
            }
        }
        CHECK(energy > 0.0);  // the render did real work
    }
    const uint64_t frees = counts().frees - before.frees;
    std::printf("AU %s %s: %llu renders of %u frames, %llu allocations, %llu frees while armed\n", label, phase,
                static_cast<unsigned long long>(renders), frames, static_cast<unsigned long long>(allocs), static_cast<unsigned long long>(frees));
    CHECK(allocs == 0);
    CHECK(frees == 0);
}
}

int main(int argc, char** argv) {
    CHECK(argc == 2);
    self_check();
    AudioComponent component = proof_register_component(argv[1]);
    for (UInt32 channels : {1u, 2u}) {
        const char* label = channels == 1 ? "mono" : "stereo";
        AudioUnit unit = proof_make_unit(component, channels);
        CHECK(AudioUnitInitialize(unit) == noErr);
        ProofBuffers buffers(channels, 4096);
        double clock = 0.0;
        // The parameter to move is the first one the unit lists.
        UInt32 listSize = 0;
        CHECK(AudioUnitGetPropertyInfo(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, &listSize, nullptr) == noErr);
        CHECK(listSize >= sizeof(AudioUnitParameterID));
        std::vector<AudioUnitParameterID> ids(listSize / sizeof(AudioUnitParameterID));
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, 0, ids.data(), &listSize) == noErr);
        AudioUnitParameterInfo paramInfo{};
        UInt32 paramInfoSize = sizeof(paramInfo);
        CHECK(AudioUnitGetProperty(unit, kAudioUnitProperty_ParameterInfo, kAudioUnitScope_Global, ids[0], &paramInfo, &paramInfoSize) == noErr);
        const int param = static_cast<int>(ids[0]);
        // Warm up every size and path once, not armed: lazy first-use setup is not the render path.
        for (UInt32 frames : {kProofBlock, 100u, 4096u}) {
            for (int i = 0; i < 4; ++i) {
                CHECK(proof_render(unit, buffers, frames, clock) == noErr);
                clock += frames;
            }
        }
        proof_set_bypass(unit, true);
        for (int i = 0; i < 4; ++i) { CHECK(proof_render(unit, buffers, kProofBlock, clock) == noErr); clock += kProofBlock; }
        proof_set_bypass(unit, false);
        for (int i = 0; i < 4; ++i) { CHECK(proof_render(unit, buffers, kProofBlock, clock) == noErr); clock += kProofBlock; }

        run_phase(unit, buffers, label, "processing", kProofBlock, 64, clock, param, 0);
        run_phase(unit, buffers, label, "odd block size", 100, 16, clock, param, 0);
        run_phase(unit, buffers, label, "maximum block size", 4096, 8, clock, param, 0);
        // The unit's first parameter moves between 2 values; this keeps within its range for the example (Gain, dB).
        const float low = paramInfo.minValue;
        const float high = paramInfo.maxValue;
        CHECK(low < high);
        for (int i = 0; i < 16; ++i) {
            const float value = i % 2 == 0 ? low + 0.25f * (high - low) : low + 0.75f * (high - low);
            CHECK(AudioUnitSetParameter(unit, static_cast<AudioUnitParameterID>(param), kAudioUnitScope_Global, 0, value, 0) == noErr);
            run_phase(unit, buffers, label, "parameter change", kProofBlock, 1, clock, param, 0);
        }
        proof_set_bypass(unit, true);
        run_phase(unit, buffers, label, "bypass on, crossfading and bypassed", kProofBlock, 16, clock, param, 0);
        CHECK(AudioUnitSetParameter(unit, static_cast<AudioUnitParameterID>(param), kAudioUnitScope_Global, 0, low + 0.5f * (high - low), 0) == noErr);
        run_phase(unit, buffers, label, "bypassed with a parameter change", kProofBlock, 4, clock, param, 0);
        proof_set_bypass(unit, false);
        run_phase(unit, buffers, label, "bypass off, crossfading and processing", kProofBlock, 16, clock, param, 0);
        CHECK(AudioUnitReset(unit, kAudioUnitScope_Global, 0) == noErr);
        run_phase(unit, buffers, label, "render after a reset", kProofBlock, 4, clock, param, 0);
        CHECK(AudioUnitUninitialize(unit) == noErr);
        CHECK(AudioComponentInstanceDispose(unit) == noErr);
    }
    std::puts("AU alloc proof: AudioUnitRender made no allocation, mono and stereo, with parameter changes and bypass on and off: PASS");
    return 0;
}
