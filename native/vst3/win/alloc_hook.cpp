// SPDX-License-Identifier: MIT
// malloc_logger for the VST3 no-allocation proof (native/vst3/noalloc.cpp) on Windows.
//
// On macOS the proof sets libmalloc's malloc_logger and hears of every allocation in
// the process. Windows has no such hook, so this file provides the variable and
// feeds it: every heap allocation in a Windows process, whichever image asks, ends
// in ntdll's RtlAllocateHeap or RtlReAllocateHeap, reached through an import of
// HeapAlloc, HeapReAlloc (kernel32 forwards both to ntdll) or of the Rtl functions
// themselves. The C runtime's malloc, calloc, realloc and operator new reach it
// through ucrtbase.dll's import of HeapAlloc; the Rust engine inside the module
// through the module's own import (Rust's System allocator calls HeapAlloc).
// Those import table entries are pointed at counting functions here, in every
// module loaded in the process except ntdll, and again after each module a proof
// loads (op_win_module_loaded).
//
// Not seen: allocations ntdll makes for itself, and memory taken straight from
// VirtualAlloc. The proof's self-check shows that operator new, malloc, calloc,
// realloc, strdup, getcwd and the module's own instance setup are counted.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <psapi.h>
#include "CoreFoundation/CoreFoundation.h"
#include <cstdint>
#include <cstring>

extern "C" {
typedef void(malloc_logger_t)(uint32_t type, uintptr_t arg1, uintptr_t arg2, uintptr_t arg3, uintptr_t result, uint32_t num_hot_frames_to_skip);
malloc_logger_t* malloc_logger = nullptr;
}

namespace {
constexpr uint32_t kLogAlloc = 2;  // stack_logging_type_alloc; libmalloc sets it on a realloc too
constexpr uint32_t kLogDealloc = 4;

using AllocFn = void* (WINAPI*)(HANDLE heap, DWORD flags, SIZE_T size);
using ReAllocFn = void* (WINAPI*)(HANDLE heap, DWORD flags, void* block, SIZE_T size);
AllocFn gAlloc = nullptr;
ReAllocFn gReAlloc = nullptr;

void* WINAPI countedAlloc(HANDLE heap, DWORD flags, SIZE_T size) {
    void* result = gAlloc(heap, flags, size);
    if (malloc_logger_t* logger = malloc_logger) logger(kLogAlloc, reinterpret_cast<uintptr_t>(heap), size, 0, reinterpret_cast<uintptr_t>(result), 0);
    return result;
}
void* WINAPI countedReAlloc(HANDLE heap, DWORD flags, void* block, SIZE_T size) {
    void* result = gReAlloc(heap, flags, block, size);
    if (malloc_logger_t* logger = malloc_logger) {
        logger(kLogAlloc | kLogDealloc, reinterpret_cast<uintptr_t>(heap), reinterpret_cast<uintptr_t>(block), size, reinterpret_cast<uintptr_t>(result), 0);
    }
    return result;
}

void* replacementFor(const char* name) {
    if (std::strcmp(name, "HeapAlloc") == 0 || std::strcmp(name, "RtlAllocateHeap") == 0) return reinterpret_cast<void*>(countedAlloc);
    if (std::strcmp(name, "HeapReAlloc") == 0 || std::strcmp(name, "RtlReAllocateHeap") == 0) return reinterpret_cast<void*>(countedReAlloc);
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

void patchAll(void*) {
    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    if (!ntdll) return;
    if (!gAlloc) gAlloc = reinterpret_cast<AllocFn>(GetProcAddress(ntdll, "RtlAllocateHeap"));
    if (!gReAlloc) gReAlloc = reinterpret_cast<ReAllocFn>(GetProcAddress(ntdll, "RtlReAllocateHeap"));
    if (!gAlloc || !gReAlloc) return;
    HMODULE modules[1024];
    DWORD bytes = 0;
    if (!EnumProcessModules(GetCurrentProcess(), modules, sizeof(modules), &bytes)) return;
    const DWORD count = (bytes < sizeof(modules) ? bytes : static_cast<DWORD>(sizeof(modules))) / sizeof(HMODULE);
    for (DWORD i = 0; i < count; ++i) {
        if (modules[i] != ntdll) patchModule(modules[i]);
    }
}

// Runs before main, so the proof's self-check is already counted.
struct Install {
    Install() {
        op_win_module_loaded = patchAll;
        patchAll(nullptr);
    }
} gInstall;
}
