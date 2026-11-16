// SPDX-License-Identifier: MIT
// The editor ABI on Linux. op_editor.h holds on Linux as it is written, with
// these readings, and this header adds the calls a Linux shell needs for the
// scale and for driving the view. A plugin built with
// export_shell!(P, editor = ...) exports all of them. Every call is on the
// host's main thread.
//
// - The view is an X11 window (openplugin-dri/src/x11), in a Wayland session
//   too, where hosts embed plug-in editors through Xwayland. A window id has
//   32 bits; where op_editor.h has a view pointer, Linux has the id as a
//   pointer-sized integer.
// - op_editor_view returns the view's window id. The window itself is made by
//   op_editor_attach, inside the parent.
// - op_editor_attach(editor, parent): parent is the host's window id (VST3
//   IPlugView::attached with kPlatformTypeX11EmbedWindowID). The view sits at
//   the parent's top left and is points x scale pixels. A second attach moves
//   the view to the new parent.
// - op_editor_create returns null when there is no X server to connect to.
// - Sizes in op_editor.h (op_editor_size, _size_range, _get_size,
//   _adjust_size, _resize, OpEditorRequestResize) stay in points. A Linux host
//   works in pixels (VST3 ViewRect): pixels = round(points x scale).
// - OpEditorContextMenu's x, y are points in the view; multiply by the scale
//   for pixels.
// - Nobody runs an event loop for the view's X connection. The shell asks its
//   host for a timer of op_editor_tick_ms() and for the descriptor op_editor_fd()
//   (VST3 Linux::IRunLoop), and calls op_editor_pump from both. A view that is
//   never pumped draws nothing and hears no input.
#pragma once
#include <stdint.h>
#include "op_editor.h"
#ifdef __cplusplus
extern "C" {
#endif
// Pixels per point of this editor's view. The view decides it, from the
// display (Xft.dpi over 96, 1 when the display names none); a host cannot set
// it. 0 for a null editor.
double op_editor_scale(void* editor);
// The scale an editor opened now would have, for the sizes a host asks before
// one exists. 1 when there is no X server.
double op_editor_display_scale(void);
// How often to call op_editor_pump from the host's timer, in milliseconds.
uint32_t op_editor_tick_ms(void);
// The descriptor of the view's X connection: call op_editor_pump when it is
// readable. Negative for a null editor.
int32_t op_editor_fd(void* editor);
// Handles the X events that have arrived, ticks the editor if a tick is due and
// presents what it drew. Never blocks; calling it more often than the tick is
// fine.
void op_editor_pump(void* editor);
#ifdef __cplusplus
}
#endif
