// SPDX-License-Identifier: MIT
// pthread_main_np for the VST3 proofs and stand-in plugins on Linux, which has no
// such call: the main thread is the one whose thread id is the process id. Proof
// builds only; native/vst3/linux/vst3.cmake puts this in front of those sources.
#pragma once
#include <sys/syscall.h>
#include <unistd.h>
#ifdef __cplusplus
extern "C" {
#endif
static inline int pthread_main_np(void) { return syscall(SYS_gettid) == getpid() ? 1 : 0; }
#ifdef __cplusplus
}
#endif
