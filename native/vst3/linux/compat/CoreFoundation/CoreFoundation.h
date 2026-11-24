// SPDX-License-Identifier: MIT
// Linux stand-in for the CoreFoundation calls the VST3 proofs (native/vst3/*.cpp)
// use to load a bundle and run the main loop. Proof builds only: it lets the same
// proof sources build on Linux, so the macOS files stay as they are. Implemented in
// cf_compat.cpp over dlopen, dlsym and poll.
//
// What differs from CoreFoundation, on purpose:
//   - a bundle is the VST3 folder `<Name>.vst3`; its executable is
//     `Contents/<machine>-linux/<Name>.so`;
//   - "bundleEntry" and "bundleExit" resolve to the module's ModuleEntry and
//     ModuleExit, the entry points a VST3 module has on Linux;
//   - the main run loop is the one a Linux host lends its plug-ins
//     (Steinberg::Linux::IRunLoop). "bundleEntry" hands it to the module's factory
//     (IPluginFactory3::setHostContext), as a host does after loading a module, and
//     CFRunLoopRunInMode runs it;
//   - releasing a bundle does not unload the library (a proof exits right after).
#pragma once
#include <cstdint>

namespace Steinberg {
namespace Linux {
class IRunLoop;
}
}

extern "C" {
typedef unsigned char UInt8;
typedef unsigned char Boolean;
typedef long long CFIndex;
typedef double CFTimeInterval;
typedef const void* CFTypeRef;
typedef const void* CFAllocatorRef;
typedef const char* CFStringRef;
typedef CFStringRef CFRunLoopMode;
typedef const struct OpLinuxCFURL* CFURLRef;
typedef struct OpLinuxCFBundle* CFBundleRef;

#define CFSTR(text) (static_cast<CFStringRef>(text))

CFURLRef CFURLCreateFromFileSystemRepresentation(CFAllocatorRef allocator, const UInt8* utf8, CFIndex length, Boolean isDirectory);
CFBundleRef CFBundleCreate(CFAllocatorRef allocator, CFURLRef url);
Boolean CFBundleLoadExecutable(CFBundleRef bundle);
void* CFBundleGetFunctionPointerForName(CFBundleRef bundle, CFStringRef name);
void CFRelease(CFTypeRef object);

extern const CFRunLoopMode kCFRunLoopDefaultMode;
// Runs the calling thread's timers and descriptor handlers for up to `seconds`.
// With `returnAfterSourceHandled` it returns after the first that ran.
int32_t CFRunLoopRunInMode(CFRunLoopMode mode, CFTimeInterval seconds, Boolean returnAfterSourceHandled);
}

// Not CoreFoundation: the run loop CFRunLoopRunInMode runs, for a proof that plays
// the host of an editor (IPlugFrame answers a query for IRunLoop with it).
Steinberg::Linux::IRunLoop* op_linux_run_loop();
