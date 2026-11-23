// SPDX-License-Identifier: MIT
// The VST3 shell on Linux: the shell of native/vst3/OpenPluginVST3.cpp, built for a
// Linux host, the way native/vst3/win/OpenPluginVST3Win.cpp builds it for Windows.
// This file sets up what differs, includes that one, and adds the Linux view.
//
//   - Class ids: as on macOS. The VST3 SDK is not COM compatible on Linux, so a
//     TUID is the 16 bytes of the FUID string, as op_shell_info gives them.
//   - The window type: kPlatformTypeX11EmbedWindowID. The parent is the host's X
//     window id, which the plugin's X11 view (openplugin-dri/src/x11) takes as its
//     parent (op_editor_x11.h).
//   - Sizes: a Linux host speaks pixels, the editor points. The shell's size calls
//     go through native/vst3/win/editor_pixels.h, which converts at the view's
//     scale (the display's Xft.dpi over 96; the host does not set it).
//   - The event loop: the view's X connection is run by the host's loop. The view
//     below registers a timer and the connection's descriptor with the host's
//     Linux::IRunLoop (from its IPlugFrame) and pumps the view on both. A host
//     without a run loop cannot drive the view, so attaching fails there.
//   - The entry points come from the VST3 SDK's linuxmain.cpp (ModuleEntry,
//     ModuleExit); native/vst3/linux/vst3.cmake adds it.
#include "op_shell.h"
#include "pluginterfaces/gui/iplugview.h"

// iplugview.h is included above, so this renames only the shell's use of the name.
#define kPlatformTypeNSView kPlatformTypeX11EmbedWindowID

#if OP_HAS_EDITOR
// op_editor.h is included by editor_pixels.h, so these rename only the shell's calls.
#include "op_editor_x11.h"
#include "../win/editor_pixels.h"
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
// The shell's view, run by the host's loop.
class LinuxEditorView final : public EditorView, public Linux::ITimerHandler, public Linux::IEventHandler {
public:
    explicit LinuxEditorView(Controller* controller) : EditorView(controller) {}
    ~LinuxEditorView() override { leaveLoop(); }

    tresult PLUGIN_API attached(void* parent, FIDString type) SMTG_OVERRIDE {
        // The loop first: without one the view would never draw.
        Linux::IRunLoop* loop = nullptr;
        if (!plugFrame || plugFrame->queryInterface(Linux::IRunLoop::iid, reinterpret_cast<void**>(&loop)) != kResultTrue || !loop)
            return kResultFalse;
        const tresult result = EditorView::attached(parent, type);
        if (result != kResultTrue) {
            loop->release();
            return result;
        }
        loop_ = loop;
        loop_->registerTimer(this, OP_EDITOR_TICK_MS);
        watchDescriptor();
        return result;
    }
    tresult PLUGIN_API removed() SMTG_OVERRIDE {
        leaveLoop();
        return EditorView::removed();
    }

    // Linux::ITimerHandler: the view's tick.
    void PLUGIN_API onTimer() SMTG_OVERRIDE { pump(); }
    // Linux::IEventHandler: the X connection has something to read.
    void PLUGIN_API onFDIsSet(Linux::FileDescriptor) SMTG_OVERRIDE { pump(); }

    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) SMTG_OVERRIDE {
        QUERY_INTERFACE(iid, obj, Linux::ITimerHandler::iid, Linux::ITimerHandler)
        QUERY_INTERFACE(iid, obj, Linux::IEventHandler::iid, Linux::IEventHandler)
        return EditorView::queryInterface(iid, obj);
    }
    uint32 PLUGIN_API addRef() SMTG_OVERRIDE { return EditorView::addRef(); }
    uint32 PLUGIN_API release() SMTG_OVERRIDE { return EditorView::release(); }

private:
    void pump() {
        void* editor = ownEditor();
        if (!editor) return;
        op_editor_pump(editor);
        // A lost connection stays readable for good: stop watching it.
        if (fd_ >= 0 && op_editor_fd(editor) < 0) unwatchDescriptor();
    }
    void watchDescriptor() {
        void* editor = ownEditor();
        const int32_t fd = editor ? op_editor_fd(editor) : -1;
        if (fd < 0 || !loop_) return;
        if (loop_->registerEventHandler(this, fd) == kResultTrue) fd_ = fd;
    }
    void unwatchDescriptor() {
        if (fd_ >= 0 && loop_) loop_->unregisterEventHandler(this);
        fd_ = -1;
    }
    void leaveLoop() {
        if (!loop_) return;
        unwatchDescriptor();
        loop_->unregisterTimer(this);
        loop_->release();
        loop_ = nullptr;
    }

    Linux::IRunLoop* loop_ = nullptr;
    int32_t fd_ = -1;
};

IPlugView* PLUGIN_API Controller::createView(FIDString name) {
    if (!name || std::strcmp(name, ViewType::kEditor) != 0) return nullptr;
    return new LinuxEditorView(this);
}
}
#endif
