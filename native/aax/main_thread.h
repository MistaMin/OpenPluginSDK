// SPDX-License-Identifier: MIT
// The main thread, for the AAX data model. AAX lets the host send a notification
// on any thread and gives the data model no main-thread callback of its own, so
// what must run on the main thread is handed to the system's main queue.
//
// On macOS that is the main dispatch queue, which the host's run loop serves.
// Elsewhere there is no call here to ask which thread is the main one or to
// reach it: onMainThread() answers true and nothing is ever posted, so the
// work runs on the calling thread.
#pragma once
#ifdef __APPLE__
#include <dispatch/dispatch.h>
#include <pthread.h>
#endif
namespace openplugin_aax {
#ifdef __APPLE__
inline bool onMainThread() { return pthread_main_np() != 0; }
// Runs work(context) on the main thread, later. `context` must stay valid until then.
inline void postToMainThread(void* context, void (*work)(void*)) { dispatch_async_f(dispatch_get_main_queue(), context, work); }
#else
inline bool onMainThread() { return true; }
inline void postToMainThread(void* context, void (*work)(void*)) { work(context); }
#endif
}
