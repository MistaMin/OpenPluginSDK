// SPDX-License-Identifier: MIT
// Forced into the stand-in plugins (native/vst3/*_stub.cpp) when MSVC builds them.
// They mark the functions a proof looks up with the GCC spelling
// __attribute__((visibility("default"))); MSVC's spelling of an exported function
// is __declspec(dllexport). Proof builds only, and only those files.
#pragma once
#if defined(_MSC_VER) && !defined(__clang__)
#define __attribute__(spec) __declspec(dllexport)
#endif
