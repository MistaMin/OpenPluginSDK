// SPDX-License-Identifier: MIT
// The editor half of a stand-in plugin: op_editor_* that puts no view on screen
// and records what the shell tells it about the host's track
// (op_editor_set_track_info). The SDK example's editor keeps no track, so it
// cannot show what a shell passed. Test builds only; include it in one source
// file of a target built with OP_EDITOR. The op_stub_editor_* functions are test
// hooks, not part of the editor ABI.
#pragma once
#ifndef OP_EDITOR_RESIZE
#define OP_EDITOR_RESIZE 1
#endif
#ifndef OP_EDITOR_MENU
#define OP_EDITOR_MENU 1
#endif
#include "op_editor.h"
#include <cstring>
#include <new>
#ifdef __APPLE__
#include <pthread.h>
#endif

#ifdef _WIN32
#define OP_STUB_EDITOR_EXPORT __declspec(dllexport)
#else
#define OP_STUB_EDITOR_EXPORT __attribute__((visibility("default")))
#endif

namespace op_stub_editor {
constexpr uint32_t kMaxCalls = 64;
struct Call {
    void* editor;
    char name[300];
    bool has_name;
    uint32_t rgba;
    uint32_t flags;
    bool main_thread;
};
inline Call gCalls[kMaxCalls];
inline uint32_t gCount = 0;
inline uint32_t gLive = 0;
struct Editor {
    uint32_t width = 200;
    uint32_t height = 100;
};
}

extern "C" {
// How many editors exist, how many times an editor was told about the track, and
// the n-th call (0 is the first). op_stub_editor_track_call returns 0 for a call
// that does not exist. `main_thread` is always 0 where the system has no main
// thread to ask about.
OP_STUB_EDITOR_EXPORT uint32_t op_stub_editor_live(void) { return op_stub_editor::gLive; }
OP_STUB_EDITOR_EXPORT uint32_t op_stub_editor_track_count(void) { return op_stub_editor::gCount; }
OP_STUB_EDITOR_EXPORT uint32_t op_stub_editor_track_call(uint32_t n, char* name, uint32_t capacity, uint32_t* has_name, uint32_t* rgba,
                                                         uint32_t* flags, uint32_t* main_thread) {
    if (n >= op_stub_editor::gCount) return 0;
    const op_stub_editor::Call& c = op_stub_editor::gCalls[n];
    if (name && capacity) {
        std::strncpy(name, c.name, capacity - 1);
        name[capacity - 1] = 0;
    }
    if (has_name) *has_name = c.has_name;
    if (rgba) *rgba = c.rgba;
    if (flags) *flags = c.flags;
    if (main_thread) *main_thread = c.main_thread;
    return 1;
}

uint32_t op_editor_size(uint32_t* width, uint32_t* height) {
    if (width) *width = 200;
    if (height) *height = 100;
    return 1;
}
void* op_editor_create(const OpEditorHost*) {
    auto* editor = new (std::nothrow) op_stub_editor::Editor;
    if (editor) ++op_stub_editor::gLive;
    return editor;
}
void* op_editor_view(void*) { return nullptr; }
uint32_t op_editor_attach(void* editor, void* parent) { return editor && parent ? 1 : 0; }
void op_editor_bind(void*, void*) {}
void op_editor_set_plain(void*, uint32_t, float) {}
void op_editor_destroy(void* editor) {
    if (!editor) return;
    --op_stub_editor::gLive;
    delete static_cast<op_stub_editor::Editor*>(editor);
}
uint32_t op_editor_size_range(uint32_t* min_width, uint32_t* min_height, uint32_t* max_width, uint32_t* max_height, uint32_t* aspect_width,
                              uint32_t* aspect_height) {
    if (min_width) *min_width = 200;
    if (max_width) *max_width = 200;
    if (min_height) *min_height = 100;
    if (max_height) *max_height = 100;
    if (aspect_width) *aspect_width = 0;
    if (aspect_height) *aspect_height = 0;
    return 1;
}
uint32_t op_editor_get_size(void* editor, uint32_t* width, uint32_t* height) {
    if (!editor) return 0;
    if (width) *width = static_cast<op_stub_editor::Editor*>(editor)->width;
    if (height) *height = static_cast<op_stub_editor::Editor*>(editor)->height;
    return 1;
}
uint32_t op_editor_adjust_size(void* editor, uint32_t* width, uint32_t* height) { return op_editor_get_size(editor, width, height); }
uint32_t op_editor_resize(void* editor, uint32_t width, uint32_t height) { return editor && width == 200 && height == 100; }
void op_editor_set_request_resize(void*, void*, OpEditorRequestResize) {}
void op_editor_set_context_menu(void*, void*, OpEditorContextMenu) {}
void op_editor_set_track_info(void* editor, const char* name, uint32_t rgba, uint32_t flags) {
    if (!editor || op_stub_editor::gCount >= op_stub_editor::kMaxCalls) return;
    op_stub_editor::Call& c = op_stub_editor::gCalls[op_stub_editor::gCount++];
    std::memset(&c, 0, sizeof(c));
    c.editor = editor;
    c.has_name = name != nullptr;
    if (name) std::strncpy(c.name, name, sizeof(c.name) - 1);
    c.rgba = rgba;
    c.flags = flags;
#ifdef __APPLE__
    c.main_thread = pthread_main_np() != 0;
#endif
}
}
