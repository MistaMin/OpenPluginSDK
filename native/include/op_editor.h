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

// Resizable editors. Optional: a shell defines OP_EDITOR_RESIZE before
// including this header to use them, and a shell that does not still builds.
// The Rust side exports them with the rest of op_editor_*. Sizes are points.
#ifdef OP_EDITOR_RESIZE
// The sizes any editor of this plugin takes. The aspect is 0, 0 for any
// shape. The editor is fixed when min equals max. Call before op_editor_create.
uint32_t op_editor_size_range(uint32_t* min_width, uint32_t* min_height,
                              uint32_t* max_width, uint32_t* max_height,
                              uint32_t* aspect_width, uint32_t* aspect_height);
// The editor's current size (op_editor_size is the size it opens at).
uint32_t op_editor_get_size(void* editor, uint32_t* width, uint32_t* height);
// Replaces a proposed size with the nearest one the editor takes, without
// changing the editor (VST3 checkSizeConstraint, a host's adjust-size call).
uint32_t op_editor_adjust_size(void* editor, uint32_t* width, uint32_t* height);
// Gives the editor and its view a new size. Returns 1 if the size is inside
// the range, 0 and no change if not.
uint32_t op_editor_resize(void* editor, uint32_t width, uint32_t height);
// How an editor asks the host for a size. Return non-zero if the host agrees;
// the shell then calls op_editor_resize. Null removes it. Replaces nothing in
// OpEditorHost, so a shell that does not set it is unchanged.
typedef uint32_t (*OpEditorRequestResize)(void* context, uint32_t width, uint32_t height);
void op_editor_set_request_resize(void* editor, void* context, OpEditorRequestResize request);
#endif

#ifdef __cplusplus
}
#endif
