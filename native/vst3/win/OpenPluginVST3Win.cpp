// SPDX-License-Identifier: MIT
// The VST3 shell on Windows: the shell of native/vst3/OpenPluginVST3.cpp, built for
// a Windows host. This file sets up what differs, includes that one, and adds the
// Windows view.
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
//   - The scale. WinEditorView below is EditorView with
//     IPlugViewContentScaleSupport: the host's scale goes to the editor
//     (op_editor_set_scale). A host that gives none gets the DPI of its window.
//   - The entry points come from the VST3 SDK's dllmain.cpp (InitDll, ExitDll,
//     DllMain) in place of macmain.cpp; native/vst3/win/vst3.cmake adds it.
#include "op_shell.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
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
#define op_editor_attach ::openplugin_vst_win::editor_attach
#define op_editor_destroy ::openplugin_vst_win::editor_destroy
// This file defines Controller::createView, to make the view below.
#define OP_VST3_OWN_EDITOR_VIEW 1
#endif

#include "../OpenPluginVST3.cpp"

#if OP_HAS_EDITOR
namespace openplugin_vst {
// The shell's view, taking its scale from the host.
class WinEditorView final : public EditorView, public IPlugViewContentScaleSupport {
public:
    explicit WinEditorView(Controller* controller) : EditorView(controller) {}
    // Pixels per point from the host, before or after the view is attached and
    // whenever the scale of its window changes. An open editor's window becomes
    // points x factor pixels at once; the frame is asked for that size.
    tresult PLUGIN_API setContentScaleFactor(ScaleFactor factor) SMTG_OVERRIDE {
        if (!(factor > 0.0f)) return kInvalidArgument;
        ::openplugin_vst_win::editor_set_host_scale(factor);
        uint32_t width = 0;
        uint32_t height = 0;
        if (void* editor = ownEditor()) {
            if (!op_editor_set_scale(editor, factor) || !op_editor_get_size(editor, &width, &height)) return kResultFalse;
            ViewRect scaled(rect.left, rect.top, rect.left + static_cast<int32>(width), rect.top + static_cast<int32>(height));
            const bool changed = scaled.getWidth() != rect.getWidth() || scaled.getHeight() != rect.getHeight();
            // The window has the new size already, whatever the frame answers.
            rect = scaled;
            if (changed && plugFrame) plugFrame->resizeView(this, &scaled);
            return kResultTrue;
        }
        // No editor yet: the size and the range the host reads are at this scale.
        op_editor_size(&width, &height);
        rect = ViewRect(rect.left, rect.top, rect.left + static_cast<int32>(width), rect.top + static_cast<int32>(height));
        if (!op_editor_size_range(&minW_, &minH_, &maxW_, &maxH_, &aspectW_, &aspectH_)) {
            minW_ = maxW_ = width;
            minH_ = maxH_ = height;
        }
        return kResultTrue;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) SMTG_OVERRIDE {
        QUERY_INTERFACE(iid, obj, IPlugViewContentScaleSupport::iid, IPlugViewContentScaleSupport)
        return EditorView::queryInterface(iid, obj);
    }
    uint32 PLUGIN_API addRef() SMTG_OVERRIDE { return EditorView::addRef(); }
    uint32 PLUGIN_API release() SMTG_OVERRIDE { return EditorView::release(); }
};

IPlugView* PLUGIN_API Controller::createView(FIDString name) {
    if (!name || std::strcmp(name, ViewType::kEditor) != 0) return nullptr;
    return new WinEditorView(this);
}
}
#endif
