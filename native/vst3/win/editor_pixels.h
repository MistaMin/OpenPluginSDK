// SPDX-License-Identifier: MIT
// The editor ABI as the VST3 shell's view uses it on Windows.
//
// A VST3 view on Windows talks to its host in pixels of the parent window
// (IPlugView::getSize, onSize, checkSizeConstraint, IPlugFrame::resizeView, and
// the position of IContextMenu::popup). The plugin's editor works in points; its
// HWND view (openplugin-dri/src/win32.rs) is `points x scale` pixels, where the
// scale is the DPI of the window it sits in over 96.
//
// The shell's view (EditorView in native/vst3/OpenPluginVST3.cpp) passes sizes
// between host and editor as they are, which is right on macOS, where both are
// points. On Windows the calls below stand between the two and convert:
// OpenPluginVST3Win.cpp points the shell's op_editor_* size calls at them. With a
// scale of 1 every one of them is the op_editor_* call it stands for.
//
// Beyond op_editor.h they use the three calls of op_editor_win.h: op_editor_scale,
// op_editor_set_scale and op_editor_set_pixel_size.
#pragma once
#ifndef OP_EDITOR_RESIZE
#define OP_EDITOR_RESIZE 1
#endif
#ifndef OP_EDITOR_MENU
#define OP_EDITOR_MENU 1
#endif
#include "op_editor_win.h"

namespace openplugin_vst_win {
// op_editor_size and op_editor_size_range in pixels at the system's scale: no
// editor exists yet, so its window's scale is not known. Once the editor is open
// the view asks editor_get_size and editor_adjust_size, which use the real one.
uint32_t editor_size(uint32_t* width, uint32_t* height);
uint32_t editor_size_range(uint32_t* min_width, uint32_t* min_height, uint32_t* max_width, uint32_t* max_height,
                           uint32_t* aspect_width, uint32_t* aspect_height);
// op_editor_get_size in pixels.
uint32_t editor_get_size(void* editor, uint32_t* width, uint32_t* height);
// op_editor_adjust_size on a size in pixels: the nearest size the editor takes, in pixels.
uint32_t editor_adjust_size(void* editor, uint32_t* width, uint32_t* height);
// op_editor_resize to a size in pixels (op_editor_set_pixel_size).
uint32_t editor_resize(void* editor, uint32_t width, uint32_t height);
// op_editor_set_request_resize: the editor asks in points, `request` hears pixels.
void editor_set_request_resize(void* editor, void* context, OpEditorRequestResize request);
// op_editor_set_context_menu: the editor gives the click in points, `show` hears pixels.
void editor_set_context_menu(void* editor, void* context, OpEditorContextMenu show);
// op_editor_destroy, and forgets what the two calls above kept for this editor.
void editor_destroy(void* editor);
}
