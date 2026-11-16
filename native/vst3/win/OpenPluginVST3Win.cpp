// SPDX-License-Identifier: MIT
// The VST3 shell on Windows: the shell of native/vst3/OpenPluginVST3.cpp, built for
// a Windows host. Nothing in that file is changed; this one sets up what differs
// before including it.
//
//   - The window type. The shell's IPlugView (EditorView) hands the host's parent
//     to op_editor_attach; the one thing in it that names a platform is the type
//     it accepts. Here that is kPlatformTypeHWND, and the parent is an HWND, which
//     the plugin's HWND view (openplugin-dri/src/win32.rs) takes as its parent.
//   - Sizes. A Windows host speaks pixels, the editor points: the shell's size
//     calls go through editor_pixels.h, which converts at the view's scale.
//   - The entry points come from the VST3 SDK's dllmain.cpp (InitDll, ExitDll,
//     DllMain) in place of macmain.cpp; native/vst3/win/vst3.cmake adds it.
//
// Not here: IPlugViewContentScaleSupport. The view takes its scale from the DPI of
// the host's window, which is what a host passes to setContentScaleFactor.
#include "pluginterfaces/gui/iplugview.h"

// iplugview.h is included above, so this renames only the shell's use of the name.
#define kPlatformTypeNSView kPlatformTypeHWND

#if OP_HAS_EDITOR
// op_editor.h is included by editor_pixels.h, so these rename only the shell's calls.
#include "editor_pixels.h"
#define op_editor_size ::openplugin_vst_win::editor_size
#define op_editor_size_range ::openplugin_vst_win::editor_size_range
#define op_editor_get_size ::openplugin_vst_win::editor_get_size
#define op_editor_adjust_size ::openplugin_vst_win::editor_adjust_size
#define op_editor_resize ::openplugin_vst_win::editor_resize
#define op_editor_set_request_resize ::openplugin_vst_win::editor_set_request_resize
#define op_editor_set_context_menu ::openplugin_vst_win::editor_set_context_menu
#define op_editor_destroy ::openplugin_vst_win::editor_destroy
#endif

#include "../OpenPluginVST3.cpp"
