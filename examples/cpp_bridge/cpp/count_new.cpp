// SPDX-License-Identifier: MIT
// Test only (feature test-faults). Replaces the global operator new/delete so
// that every C++ `new`, std::vector and std::string allocation goes through
// Rust's global allocator, where the conformance kit's counting allocator sees
// it. Without this the kit cannot see C++ allocations at all.
#include <cstddef>
#include <new>

extern "C" void* opb_rust_alloc(std::size_t size) noexcept;
extern "C" void opb_rust_free(void* p) noexcept;

void* operator new(std::size_t n) {
    void* p = opb_rust_alloc(n);
    if (!p) throw std::bad_alloc();
    return p;
}
void* operator new[](std::size_t n) { return operator new(n); }
void* operator new(std::size_t n, const std::nothrow_t&) noexcept { return opb_rust_alloc(n); }
void* operator new[](std::size_t n, const std::nothrow_t&) noexcept { return opb_rust_alloc(n); }
void operator delete(void* p) noexcept { opb_rust_free(p); }
void operator delete[](void* p) noexcept { opb_rust_free(p); }
void operator delete(void* p, std::size_t) noexcept { opb_rust_free(p); }
void operator delete[](void* p, std::size_t) noexcept { opb_rust_free(p); }
