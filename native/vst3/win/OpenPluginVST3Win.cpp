// SPDX-License-Identifier: MIT
// The VST3 shell on Windows: the shell of native/vst3/OpenPluginVST3.cpp, built for
// a Windows host. Nothing in that file is changed; this one sets up what differs
// before including it.
//
//   - Class ids. op_shell_info gives each class id as the 16 bytes of its FUID
//     string, and on macOS a TUID is those bytes. On Windows the VST3 SDK is COM
//     compatible: a TUID is a GUID, whose first 32-bit field and next two 16-bit
//     fields are little-endian (INLINE_UID in funknown.h). The shell gets the ids
//     in that order here, so a host reads the same FUID on both systems and a
//     project made on one finds the plug-in on the other.
//   - The window type. The shell's IPlugView (EditorView) hands the host's parent
//     to op_editor_attach; the one thing in it that names a platform is the type
//     it accepts. Here that is kPlatformTypeHWND, and the parent is an HWND, which
//     the plugin's HWND view (openplugin-dri/src/win32.rs) takes as its parent.
//   - Sizes. A Windows host speaks pixels, the editor points: the shell's size
//     calls go through editor_pixels.h, which converts at the view's scale.
//   - The entry points come from the VST3 SDK's dllmain.cpp (InitDll, ExitDll,
//     DllMain) in place of macmain.cpp; native/vst3/win/vst3.cmake adds it.
#include "op_shell.h"
#include "pluginterfaces/gui/iplugview.h"
#include <utility>

namespace openplugin_vst_win {
// The bytes of a FUID string as a COM GUID, and back: the same swaps both ways.
inline void com_order(uint8_t* id) {
    std::swap(id[0], id[3]);
    std::swap(id[1], id[2]);
    std::swap(id[4], id[5]);
    std::swap(id[6], id[7]);
}
// op_shell_info with the class ids in the order a TUID has on Windows.
inline uint32_t shell_info(OpShellInfo* info) {
    const uint32_t ok = op_shell_info(info);
    if (ok && info) {
        com_order(info->processor_cid);
        com_order(info->controller_cid);
    }
    return ok;
}
}
// op_shell.h is included above, so this renames only the shell's calls.
#define op_shell_info ::openplugin_vst_win::shell_info

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
