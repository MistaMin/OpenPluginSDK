// SPDX-License-Identifier: MIT
// The editor ABI on Windows. op_editor.h holds on Windows as it is written,
// with these readings, and this header adds the three calls a Windows shell
// needs for scale and pixel sizes. A plugin built with
// export_shell!(P, editor = ...) exports all of them. Every call is on the
// thread that runs the host's window messages.
//
// - The view is a child HWND, of a window class the plugin binary registers
//   for itself. It is made by op_editor_attach, because a child window needs
//   its parent: op_editor_view returns null before the first attach.
// - op_editor_attach(editor, parent): parent is the host's HWND (VST3
//   IPlugView::attached with kPlatformTypeHWND). The view sits at the
//   parent's top left and is points x scale pixels. A second attach moves the
//   view to the new parent.
// - op_editor_destroy destroys the window. A parent the host destroyed first
//   is fine: the window is gone already and the editor is freed.
// - Sizes in op_editor.h (op_editor_size, _size_range, _get_size,
//   _adjust_size, _resize, OpEditorRequestResize) stay in points. A Windows
//   host works in pixels (VST3 ViewRect): pixels = round(points x scale).
// - OpEditorContextMenu's x, y are points in the view; multiply by the scale
//   for client pixels.
#pragma once
#include <stdint.h>
#include "op_editor.h"
#ifdef __cplusplus
extern "C" {
#endif
// Pixels per point, from the host (VST3
// IPlugViewContentScaleSupport::setContentScaleFactor). The view's window
// becomes points x scale pixels at once. Without this call the view uses the
// DPI of its window (GetDpiForWindow / 96) and follows it when it changes.
// May be called before op_editor_attach. Returns 1, or 0 for a scale that is
// not a positive number.
uint32_t op_editor_set_scale(void* editor, double scale);
// The scale in use: the host's, else the window's DPI over 96 (the system's
// before the view has a window). 0 for a null editor.
double op_editor_scale(void* editor);
// The host made the view's place this many pixels (VST3 IPlugView::onSize).
// The window takes exactly that size; the editor takes the nearest size in
// points its range allows (round(pixels / scale), brought into the range).
// Returns 1, or 0 for a null editor.
uint32_t op_editor_set_pixel_size(void* editor, uint32_t width, uint32_t height);
#ifdef __cplusplus
}
#endif
