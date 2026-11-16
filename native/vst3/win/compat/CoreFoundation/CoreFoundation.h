// SPDX-License-Identifier: MIT
// Windows stand-in for the CoreFoundation calls the VST3 proofs (native/vst3/*.cpp)
// use to load a bundle and run the main loop. Proof builds only: it lets the same
// proof sources build with MSVC, so the macOS files stay as they are. Implemented
// in cf_compat.cpp over LoadLibraryW, GetProcAddress and the thread's message queue.
//
// What differs from CoreFoundation, on purpose:
//   - a bundle is the VST3 folder `<Name>.vst3`; its executable is
//     `Contents\<arch>-win\<Name>.vst3` (x86_64-win, arm64-win, arm64ec-win);
//   - "bundleEntry" and "bundleExit" resolve to the module's InitDll and ExitDll,
//     the entry points a VST3 module has on Windows;
//   - releasing a bundle does not unload the library (a proof exits right after).
#pragma once
#include <cstdint>

extern "C" {
typedef unsigned char UInt8;
typedef unsigned char Boolean;
typedef long long CFIndex;
typedef double CFTimeInterval;
typedef const void* CFTypeRef;
typedef const void* CFAllocatorRef;
typedef const char* CFStringRef;
typedef CFStringRef CFRunLoopMode;
typedef const struct OpWinCFURL* CFURLRef;
typedef struct OpWinCFBundle* CFBundleRef;

#define CFSTR(text) (static_cast<CFStringRef>(text))

CFURLRef CFURLCreateFromFileSystemRepresentation(CFAllocatorRef allocator, const UInt8* utf8, CFIndex length, Boolean isDirectory);
CFBundleRef CFBundleCreate(CFAllocatorRef allocator, CFURLRef url);
Boolean CFBundleLoadExecutable(CFBundleRef bundle);
void* CFBundleGetFunctionPointerForName(CFBundleRef bundle, CFStringRef name);
void CFRelease(CFTypeRef object);

extern const CFRunLoopMode kCFRunLoopDefaultMode;
// Dispatches the calling thread's window and timer messages for up to `seconds`.
// With `returnAfterSourceHandled` it returns after the first batch of messages.
int32_t CFRunLoopRunInMode(CFRunLoopMode mode, CFTimeInterval seconds, Boolean returnAfterSourceHandled);

// Not CoreFoundation: called with the module handle (HMODULE) after each library a
// proof loads. hook_on_load.cpp uses it to count the new module's allocations.
extern void (*op_win_module_loaded)(void* module);
}
