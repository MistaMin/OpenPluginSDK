// SPDX-License-Identifier: MIT
// Optional editor ABI. A plugin that has an editor exports these next to
// op_shell_*, and the shells are built with -DOP_EDITOR=ON. Every call is on
// the main thread. Values are plain. The view is an NSView.
#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
// How the editor reports a gesture. The shell forwards it to the host, and the
// host brings the value back to the engine as a parameter change.
typedef struct OpEditorHost {
    void* context;
    void (*begin_edit)(void* context, uint32_t index);
    void (*perform_edit)(void* context, uint32_t index, double plain);
    void (*end_edit)(void* context, uint32_t index);
} OpEditorHost;
// Size in points. The editor is not resizable.
uint32_t op_editor_size(uint32_t* width, uint32_t* height);
// The host struct is copied. Returns null on failure. The editor starts bound to no engine.
void* op_editor_create(const OpEditorHost* host);
// The editor's NSView. The editor keeps its own reference.
void* op_editor_view(void* editor);
// Adds the view to a parent NSView.
uint32_t op_editor_attach(void* editor, void* parent);
// Shows the engine from op_shell_create (meters, analysers). Null, or an
// instance that was destroyed, shows the host-reported values alone. A shell
// binds again whenever it recreates its engine.
void op_editor_bind(void* editor, void* instance);
// A parameter value from the host.
void op_editor_set_plain(void* editor, uint32_t index, float plain);
// Removes the view from its parent and frees the editor.
void op_editor_destroy(void* editor);
#ifdef __cplusplus
}
#endif
