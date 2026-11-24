// SPDX-License-Identifier: MIT
// malloc_logger for the allocation proofs on Linux (native/vst3/noalloc.cpp, and
// through native/vst3/linux/new_bridge.cpp the VST3 events and expression proofs).
//
// On macOS a proof sets libmalloc's malloc_logger and hears of every malloc and free
// in the process. The GNU C library has no such hook, but it lets a program replace
// its allocation functions: the ones defined here, in the proof executable, come
// first in the process's symbol search, so every image calls them, whichever asks:
// the C library itself (strdup, getcwd), a loaded module, the C++ runtime (operator
// new ends in malloc, also in a module that carries its own libstdc++), and a Rust
// library (its System allocator calls malloc, calloc, realloc and posix_memalign).
// Each one tells the logger and does the work with the C library's own allocator
// (__libc_malloc and its relatives, which glibc exports for this).
//
// The logger hears libmalloc's event types: 2 for an allocation, 4 for a free, both
// for a realloc.
//
// Not seen: memory taken straight from mmap or brk. Each proof's self-check shows
// that the routes it relies on are counted. glibc only: another C library has other
// names behind malloc.
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <unistd.h>

extern "C" {
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t num_hot_frames_to_skip);
malloc_logger_t* malloc_logger = nullptr;

void* __libc_malloc(size_t size);
void* __libc_calloc(size_t count, size_t size);
void* __libc_realloc(void* block, size_t size);
void* __libc_memalign(size_t alignment, size_t size);
void __libc_free(void* block);
}

namespace {
// libmalloc's event types (stack_logging_type_alloc and _dealloc; a realloc is both).
constexpr uint32_t kAlloc = 2;
constexpr uint32_t kDealloc = 4;

void tell(uint32_t type, uintptr_t a, uintptr_t b, void* result) {
    if (malloc_logger_t* logger = malloc_logger) logger(type, a, b, 0, reinterpret_cast<uintptr_t>(result), 0);
}
void* aligned(size_t alignment, size_t size) {
    void* result = __libc_memalign(alignment, size);
    tell(kAlloc, alignment, size, result);
    return result;
}
}

#define OP_EXPORT __attribute__((visibility("default")))
extern "C" {
OP_EXPORT void* malloc(size_t size) {
    void* result = __libc_malloc(size);
    tell(kAlloc, size, 0, result);
    return result;
}
OP_EXPORT void* calloc(size_t count, size_t size) {
    void* result = __libc_calloc(count, size);
    tell(kAlloc, count, size, result);
    return result;
}
OP_EXPORT void* realloc(void* block, size_t size) {
    void* result = __libc_realloc(block, size);
    tell(kAlloc | kDealloc, reinterpret_cast<uintptr_t>(block), size, result);
    return result;
}
OP_EXPORT void free(void* block) {
    if (block) tell(kDealloc, reinterpret_cast<uintptr_t>(block), 0, nullptr);
    __libc_free(block);
}
OP_EXPORT void* memalign(size_t alignment, size_t size) { return aligned(alignment, size); }
OP_EXPORT void* aligned_alloc(size_t alignment, size_t size) { return aligned(alignment, size); }
OP_EXPORT int posix_memalign(void** out, size_t alignment, size_t size) {
    // A power of two that is a multiple of a pointer's size.
    if (alignment < sizeof(void*) || (alignment & (alignment - 1)) != 0) return EINVAL;
    void* result = aligned(alignment, size);
    if (!result) return ENOMEM;
    *out = result;
    return 0;
}
OP_EXPORT void* valloc(size_t size) { return aligned(static_cast<size_t>(sysconf(_SC_PAGESIZE)), size); }
OP_EXPORT void* pvalloc(size_t size) {
    const size_t page = static_cast<size_t>(sysconf(_SC_PAGESIZE));
    return aligned(page, (size + page - 1) / page * page);
}
}
