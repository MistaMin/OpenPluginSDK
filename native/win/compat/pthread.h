// SPDX-License-Identifier: MIT
// Windows stand-in for the pthread calls the native proofs and their stand-in
// plugins make (native/vst3/*.cpp, native/aax/host_alloc.cpp). Proof builds only: it
// lets the macOS proof sources build with MSVC as they are. Both formats' Windows
// builds put this folder on the include path of their proofs.
#pragma once
#ifdef __cplusplus
extern "C" {
#endif
// Non-zero on the thread that loaded the image: for a proof executable the thread
// that runs main, for a stand-in plugin the thread that called LoadLibrary. In the
// proofs both are the main thread. Defined in native/vst3/win/cf_compat.cpp, which
// the VST3 proofs link; the AAX proofs do not call it.
int pthread_main_np(void);

// Thread-specific values over the system's thread local storage. The three kernel32
// functions are declared here, as windows.h declares them, so that header stays out
// of the proofs. The allocation proofs read their key from inside the heap hook:
// TlsGetValue does not allocate.
__declspec(dllimport) unsigned long __stdcall TlsAlloc(void);
__declspec(dllimport) void* __stdcall TlsGetValue(unsigned long index);
__declspec(dllimport) int __stdcall TlsSetValue(unsigned long index, void* value);
#ifdef __cplusplus
}
#endif

typedef unsigned long pthread_key_t;
// The destructor is not run: the proofs pass none.
static inline int pthread_key_create(pthread_key_t* key, void (*destructor)(void*)) {
    (void)destructor;
    const unsigned long index = TlsAlloc();
    if (index == 0xFFFFFFFFul) return 11;  // EAGAIN
    *key = index;
    return 0;
}
static inline void* pthread_getspecific(pthread_key_t key) { return TlsGetValue(key); }
static inline int pthread_setspecific(pthread_key_t key, const void* value) { return TlsSetValue(key, (void*)value) ? 0 : 22; }
