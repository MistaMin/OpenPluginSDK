// SPDX-License-Identifier: MIT
// malloc_logger for the AAX allocation proofs (native/aax/host_alloc.cpp) on Windows.
//
// On macOS the proof sets libmalloc's malloc_logger and hears of every malloc and
// free in the process. Windows has no such hook, so this file defines the variable
// and feeds it. Every heap allocation in a Windows process ends in ntdll's
// RtlAllocateHeap, RtlReAllocateHeap and RtlFreeHeap, and code reaches them through
// an import: the C runtime (malloc, calloc, realloc, free, and operator new behind
// them) through ucrtbase.dll's imports of HeapAlloc, HeapReAlloc and HeapFree, the
// Rust library linked into the proof through the proof's own imports (Rust's
// System allocator calls HeapAlloc). Before main, the import table entries for
// those functions are pointed at counting functions, in every module loaded in
// the process except ntdll.
//
// Not seen: what ntdll allocates for itself, and memory taken straight from
// VirtualAlloc. The proof's self-check shows malloc and operator new are counted.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <psapi.h>
#include <cstdint>
#include <cstring>

extern "C" {
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t skip);
malloc_logger_t* malloc_logger = nullptr;
}

namespace {
// libmalloc's event types (stack_logging_type_alloc and _dealloc; a realloc is both).
constexpr uint32_t kAlloc = 2;
constexpr uint32_t kDealloc = 4;

using AllocFn = void*(WINAPI*)(HANDLE, DWORD, SIZE_T);
using ReAllocFn = void*(WINAPI*)(HANDLE, DWORD, void*, SIZE_T);
using FreeFn = BOOL(WINAPI*)(HANDLE, DWORD, void*);
AllocFn g_alloc = nullptr;
ReAllocFn g_realloc = nullptr;
FreeFn g_free = nullptr;

void tell(uint32_t type, uintptr_t a, uintptr_t b, uintptr_t result) {
    if (malloc_logger_t* logger = malloc_logger) logger(type, a, b, 0, result, 0);
}
void* WINAPI countedAlloc(HANDLE heap, DWORD flags, SIZE_T size) {
    void* result = g_alloc(heap, flags, size);
    tell(kAlloc, reinterpret_cast<uintptr_t>(heap), size, reinterpret_cast<uintptr_t>(result));
    return result;
}
void* WINAPI countedReAlloc(HANDLE heap, DWORD flags, void* block, SIZE_T size) {
    void* result = g_realloc(heap, flags, block, size);
    tell(kAlloc | kDealloc, reinterpret_cast<uintptr_t>(heap), reinterpret_cast<uintptr_t>(block), reinterpret_cast<uintptr_t>(result));
    return result;
}
BOOL WINAPI countedFree(HANDLE heap, DWORD flags, void* block) {
    if (block) tell(kDealloc, reinterpret_cast<uintptr_t>(heap), reinterpret_cast<uintptr_t>(block), 0);
    return g_free(heap, flags, block);
}

void* replacement(const char* name) {
    if (!std::strcmp(name, "HeapAlloc") || !std::strcmp(name, "RtlAllocateHeap")) return reinterpret_cast<void*>(countedAlloc);
    if (!std::strcmp(name, "HeapReAlloc") || !std::strcmp(name, "RtlReAllocateHeap")) return reinterpret_cast<void*>(countedReAlloc);
    if (!std::strcmp(name, "HeapFree") || !std::strcmp(name, "RtlFreeHeap")) return reinterpret_cast<void*>(countedFree);
    return nullptr;
}

// Points one module's imports of the heap functions at the counting ones.
void patch(HMODULE module) {
    auto* base = reinterpret_cast<unsigned char*>(module);
    auto* dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return;
    auto* nt = reinterpret_cast<IMAGE_NT_HEADERS*>(base + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR_MAGIC) return;
    const IMAGE_DATA_DIRECTORY& dir = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
    if (!dir.VirtualAddress || !dir.Size) return;
    for (auto* import = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + dir.VirtualAddress); import->Name; ++import) {
        if (!import->OriginalFirstThunk || !import->FirstThunk) continue;
        auto* names = reinterpret_cast<IMAGE_THUNK_DATA*>(base + import->OriginalFirstThunk);
        auto* slots = reinterpret_cast<IMAGE_THUNK_DATA*>(base + import->FirstThunk);
        for (; names->u1.AddressOfData; ++names, ++slots) {
            if (IMAGE_SNAP_BY_ORDINAL(names->u1.Ordinal)) continue;
            auto* byName = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + names->u1.AddressOfData);
            void* to = replacement(reinterpret_cast<const char*>(byName->Name));
            if (!to) continue;
            DWORD old = 0;
            if (!VirtualProtect(&slots->u1.Function, sizeof(slots->u1.Function), PAGE_READWRITE, &old)) continue;
            slots->u1.Function = reinterpret_cast<ULONG_PTR>(to);
            VirtualProtect(&slots->u1.Function, sizeof(slots->u1.Function), old, &old);
        }
    }
}

// Runs before main, so the proof's self-check is already counted.
struct Install {
    Install() {
        HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
        if (!ntdll) return;
        g_alloc = reinterpret_cast<AllocFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlAllocateHeap")));
        g_realloc = reinterpret_cast<ReAllocFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlReAllocateHeap")));
        g_free = reinterpret_cast<FreeFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlFreeHeap")));
        if (!g_alloc || !g_realloc || !g_free) return;
        HMODULE modules[1024];
        DWORD bytes = 0;
        if (!EnumProcessModules(GetCurrentProcess(), modules, sizeof(modules), &bytes)) return;
        if (bytes > sizeof(modules)) bytes = sizeof(modules);
        for (DWORD i = 0; i < bytes / sizeof(HMODULE); ++i) {
            if (modules[i] != ntdll) patch(modules[i]);
        }
    }
} g_install;
}
