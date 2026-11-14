// SPDX-License-Identifier: MIT
// Allocation counter for the Audio Unit allocation proof. This is a shared
// library the proof links. It interposes the malloc family with dyld
// interposing, so every image in the process is counted: the proof, the
// bundle, the Rust archive and the system frameworks. (dyld applies the
// tuples of a linked dylib to all images; the main executable's own tuples
// are not applied, which is why this is not part of the executable.)
//
// The counters only run while armed. Test code only, never part of a product.
#include "alloc_hook.h"
#include <atomic>
#include <cstdint>
#include <cstdlib>
#include <malloc/malloc.h>

namespace {
std::atomic<bool> gArmed{false};
std::atomic<uint64_t> gAllocs{0};
std::atomic<uint64_t> gFrees{0};

// dyld does not interpose the image that holds the tuples, so the calls to
// malloc and friends inside these functions reach the real ones.
void* hook_malloc(size_t n) { op_alloc_note(); return malloc(n); }
void* hook_calloc(size_t c, size_t n) { op_alloc_note(); return calloc(c, n); }
void* hook_realloc(void* p, size_t n) { op_alloc_note(); return realloc(p, n); }
void* hook_valloc(size_t n) { op_alloc_note(); return valloc(n); }
int hook_posix_memalign(void** out, size_t align, size_t n) { op_alloc_note(); return posix_memalign(out, align, n); }
void* hook_aligned_alloc(size_t align, size_t n) { op_alloc_note(); return aligned_alloc(align, n); }
void hook_free(void* p) { op_alloc_note_free(p); free(p); }
void* hook_zone_malloc(malloc_zone_t* z, size_t n) { op_alloc_note(); return malloc_zone_malloc(z, n); }
void* hook_zone_calloc(malloc_zone_t* z, size_t c, size_t n) { op_alloc_note(); return malloc_zone_calloc(z, c, n); }
void* hook_zone_valloc(malloc_zone_t* z, size_t n) { op_alloc_note(); return malloc_zone_valloc(z, n); }
void* hook_zone_realloc(malloc_zone_t* z, void* p, size_t n) { op_alloc_note(); return malloc_zone_realloc(z, p, n); }
void* hook_zone_memalign(malloc_zone_t* z, size_t align, size_t n) { op_alloc_note(); return malloc_zone_memalign(z, align, n); }
void hook_zone_free(malloc_zone_t* z, void* p) { op_alloc_note_free(p); malloc_zone_free(z, p); }

struct Interpose {
    const void* replacement;
    const void* replacee;
};
#define OP_INTERPOSE(replacement, replacee)                                                        \
    __attribute__((used)) Interpose interpose_##replacee __attribute__((section("__DATA,__interpose"))) = { \
        reinterpret_cast<const void*>(&replacement), reinterpret_cast<const void*>(&replacee)}
OP_INTERPOSE(hook_malloc, malloc);
OP_INTERPOSE(hook_calloc, calloc);
OP_INTERPOSE(hook_realloc, realloc);
OP_INTERPOSE(hook_valloc, valloc);
OP_INTERPOSE(hook_posix_memalign, posix_memalign);
OP_INTERPOSE(hook_aligned_alloc, aligned_alloc);
OP_INTERPOSE(hook_free, free);
OP_INTERPOSE(hook_zone_malloc, malloc_zone_malloc);
OP_INTERPOSE(hook_zone_calloc, malloc_zone_calloc);
OP_INTERPOSE(hook_zone_valloc, malloc_zone_valloc);
OP_INTERPOSE(hook_zone_realloc, malloc_zone_realloc);
OP_INTERPOSE(hook_zone_memalign, malloc_zone_memalign);
OP_INTERPOSE(hook_zone_free, malloc_zone_free);
}

extern "C" {
void op_alloc_arm(void) { gArmed.store(true); }
void op_alloc_disarm(void) { gArmed.store(false); }
void op_alloc_note(void) {
    if (gArmed.load(std::memory_order_relaxed)) gAllocs.fetch_add(1, std::memory_order_relaxed);
}
void op_alloc_note_free(void* p) {
    if (p && gArmed.load(std::memory_order_relaxed)) gFrees.fetch_add(1, std::memory_order_relaxed);
}
uint64_t op_alloc_count(void) { return gAllocs.load(); }
uint64_t op_free_count(void) { return gFrees.load(); }
}
