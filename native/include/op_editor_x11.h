// SPDX-License-Identifier: MIT
// The editor ABI on Linux. op_editor.h holds as written, and so do the three
// scale calls of op_editor_win.h, with these readings; this header adds the
// three calls a Linux shell needs to drive the view. A plugin built with
// export_shell!(P, editor = ...) exports all of them. Every call is on the
// host's GUI thread, the one that runs its event loop.
//
// - The view is an X11 child window on the plugin's own X connection
//   (openplugin-dri/src/x11). op_editor_attach(editor, parent): parent is the
//   host's X window id (VST3 IPlugView::attached with
//   kPlatformTypeX11EmbedWindowID), passed as a pointer-sized integer.
// - The view takes its scale from the display (Xft.dpi over 96), not from the
//   host: op_editor_set_scale always answers 0, op_editor_scale is the view's.
//   Sizes in op_editor.h stay in points; a VST3 host on Linux works in pixels:
//   pixels = round(points x scale).
// - Nothing runs the view's X connection but the host's loop: call
//   op_editor_pump every OP_EDITOR_TICK_MS and whenever op_editor_fd is
//   readable (VST3 Linux::IRunLoop timer and event handler).
#pragma once
#include <stdint.h>
#include "op_editor_win.h"
#ifdef __cplusplus
extern "C" {
#endif
// The view's tick: openplugin-dri x11::TICK_INTERVAL_MS.
#define OP_EDITOR_TICK_MS 33
// The scale a view opened now would use, for sizes asked before an editor exists.
double op_editor_display_scale(void);
// The X connection's descriptor to watch for reading; -1 for a null editor or a
// lost connection (stop watching it then).
int32_t op_editor_fd(void* editor);
// Handle the X events that arrived and draw a frame when one is due. Never blocks.
void op_editor_pump(void* editor);
#ifdef __cplusplus
}
#endif
