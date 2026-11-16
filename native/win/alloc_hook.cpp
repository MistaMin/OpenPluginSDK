// SPDX-License-Identifier: MIT
// malloc_logger for the allocation proofs on Windows (native/vst3/noalloc.cpp,
// native/aax/host_alloc.cpp, and through native/vst3/win/new_bridge.cpp the VST3
// events and expression proofs). One file for both formats.
//
// On macOS a proof sets libmalloc's malloc_logger and hears of every malloc and free
// in the process. Windows has no such hook, so this file defines the variable and
// feeds it. Every heap allocation in a Windows process, whichever image asks, ends
// in ntdll's RtlAllocateHeap, RtlReAllocateHeap and RtlFreeHeap, and code reaches
// them through an import: the C runtime (malloc, calloc, realloc, free, and operator
// new behind them) through ucrtbase.dll's imports of HeapAlloc, HeapReAlloc and
// HeapFree (kernel32 forwards those to ntdll); a Rust library through the imports of
// the image it is linked into (Rust's System allocator calls HeapAlloc). The import
// table entries for those functions are pointed at counting functions here, in every
// module loaded in the process except ntdll: before main, and again whenever
// op_win_alloc_hook_patch is called, which a proof does after it loads a plug-in
// module (native/vst3/win/hook_on_load.cpp).
//
// The logger hears libmalloc's event types: 2 for an allocation, 4 for a free, both
// for a realloc.
//
// Not seen: what ntdll allocates for itself, and memory taken straight from
// VirtualAlloc. Each proof's self-check shows that the routes it relies on (operator
// new, malloc, calloc, realloc, and a module's own instance setup) are counted.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <psapi.h>
#include <cstdint>
#include <cstring>

extern "C" {
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t num_hot_frames_to_skip);
malloc_logger_t* malloc_logger = nullptr;
// Points the heap imports of every module loaded so far at the counting functions.
// Modules already done are left as they are, so it may be called again and again.
void op_win_alloc_hook_patch(void);
}

namespace {
// libmalloc's event types (stack_logging_type_alloc and _dealloc; a realloc is both).
constexpr uint32_t kAlloc = 2;
constexpr uint32_t kDealloc = 4;

using AllocFn = void* (WINAPI*)(HANDLE heap, DWORD flags, SIZE_T size);
using ReAllocFn = void* (WINAPI*)(HANDLE heap, DWORD flags, void* block, SIZE_T size);
using FreeFn = BOOL (WINAPI*)(HANDLE heap, DWORD flags, void* block);
AllocFn gAlloc = nullptr;
ReAllocFn gReAlloc = nullptr;
FreeFn gFree = nullptr;

void tell(uint32_t type, uintptr_t a, uintptr_t b, uintptr_t result) {
    if (malloc_logger_t* logger = malloc_logger) logger(type, a, b, 0, result, 0);
}
void* WINAPI countedAlloc(HANDLE heap, DWORD flags, SIZE_T size) {
    void* result = gAlloc(heap, flags, size);
    tell(kAlloc, reinterpret_cast<uintptr_t>(heap), size, reinterpret_cast<uintptr_t>(result));
    return result;
}
void* WINAPI countedReAlloc(HANDLE heap, DWORD flags, void* block, SIZE_T size) {
    void* result = gReAlloc(heap, flags, block, size);
    tell(kAlloc | kDealloc, reinterpret_cast<uintptr_t>(heap), reinterpret_cast<uintptr_t>(block), reinterpret_cast<uintptr_t>(result));
    return result;
}
BOOL WINAPI countedFree(HANDLE heap, DWORD flags, void* block) {
    if (block) tell(kDealloc, reinterpret_cast<uintptr_t>(heap), reinterpret_cast<uintptr_t>(block), 0);
    return gFree(heap, flags, block);
}

void* replacementFor(const char* name) {
    if (std::strcmp(name, "HeapAlloc") == 0 || std::strcmp(name, "RtlAllocateHeap") == 0) return reinterpret_cast<void*>(countedAlloc);
    if (std::strcmp(name, "HeapReAlloc") == 0 || std::strcmp(name, "RtlReAllocateHeap") == 0) return reinterpret_cast<void*>(countedReAlloc);
    if (std::strcmp(name, "HeapFree") == 0 || std::strcmp(name, "RtlFreeHeap") == 0) return reinterpret_cast<void*>(countedFree);
    return nullptr;
}

// Points one module's imports of the heap functions at the counting ones.
void patchModule(HMODULE module) {
    auto* base = reinterpret_cast<unsigned char*>(module);
    auto* dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return;
    auto* nt = reinterpret_cast<IMAGE_NT_HEADERS*>(base + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR_MAGIC) return;
    const IMAGE_DATA_DIRECTORY& directory = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT];
    if (directory.VirtualAddress == 0 || directory.Size == 0) return;
    for (auto* import = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + directory.VirtualAddress); import->Name; ++import) {
        if (!import->OriginalFirstThunk || !import->FirstThunk) continue;
        auto* names = reinterpret_cast<IMAGE_THUNK_DATA*>(base + import->OriginalFirstThunk);
        auto* slots = reinterpret_cast<IMAGE_THUNK_DATA*>(base + import->FirstThunk);
        for (; names->u1.AddressOfData; ++names, ++slots) {
            if (IMAGE_SNAP_BY_ORDINAL(names->u1.Ordinal)) continue;
            auto* byName = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + names->u1.AddressOfData);
            void* replacement = replacementFor(reinterpret_cast<const char*>(byName->Name));
            if (!replacement || reinterpret_cast<void*>(slots->u1.Function) == replacement) continue;
            DWORD old = 0;
            if (!VirtualProtect(&slots->u1.Function, sizeof(slots->u1.Function), PAGE_READWRITE, &old)) continue;
            slots->u1.Function = reinterpret_cast<ULONG_PTR>(replacement);
            VirtualProtect(&slots->u1.Function, sizeof(slots->u1.Function), old, &old);
        }
    }
}

// Runs before main, so a proof's self-check is already counted.
struct Install {
    Install() { op_win_alloc_hook_patch(); }
} gInstall;
}

extern "C" void op_win_alloc_hook_patch(void) {
    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    if (!ntdll) return;
    if (!gAlloc) gAlloc = reinterpret_cast<AllocFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlAllocateHeap")));
    if (!gReAlloc) gReAlloc = reinterpret_cast<ReAllocFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlReAllocateHeap")));
    if (!gFree) gFree = reinterpret_cast<FreeFn>(reinterpret_cast<void*>(GetProcAddress(ntdll, "RtlFreeHeap")));
    if (!gAlloc || !gReAlloc || !gFree) return;
    HMODULE modules[1024];
    DWORD bytes = 0;
    if (!EnumProcessModules(GetCurrentProcess(), modules, sizeof(modules), &bytes)) return;
    if (bytes > sizeof(modules)) bytes = sizeof(modules);
    for (DWORD i = 0; i < bytes / sizeof(HMODULE); ++i) {
        if (modules[i] != ntdll) patchModule(modules[i]);
    }
}
