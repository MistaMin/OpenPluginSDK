// SPDX-License-Identifier: MIT
// Windows stand-in for the one pthread call the VST3 proofs and their stand-in
// plugins use. Proof builds only (see CoreFoundation/CoreFoundation.h here).
#pragma once
#ifdef __cplusplus
extern "C" {
#endif
// Non-zero on the thread that loaded this image: for a proof executable the thread
// that runs main, for a stand-in plugin the thread that called LoadLibrary. In the
// proofs both are the main thread.
int pthread_main_np(void);
#ifdef __cplusplus
}
#endif
