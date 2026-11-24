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
//   - A host that embeds through a socket (Ardour's GtkSocket) gives the view the size
//     of its own area whatever the view answers: an editor with a fixed shape takes
//     the width and the height that goes with it, and the view asks the frame for that
//     size once (onSize, followSize below).
//   - Timers. The shell's own timer (the poll for a new latency or parameter info) is
//     the SDK's Timer, which on Linux runs on the run loop the host lends through
//     IPluginFactory3::setHostContext. A host that lends none gets no timer: a change
//     is then not announced until the host asks.
//   - The entry points come from the VST3 SDK's linuxmain.cpp (ModuleEntry,
//     ModuleExit); native/vst3/linux/vst3.cmake adds it.
#include "op_shell.h"
#include "pluginterfaces/gui/iplugview.h"

// iplugview.h is included above, so this renames only the shell's use of the name.
#define kPlatformTypeNSView kPlatformTypeX11EmbedWindowID

#if OP_HAS_EDITOR
// op_editor.h is included by editor_pixels.h, so these rename only the shell's calls.
// editor_pixels.h first: it turns on the size range and menu calls of op_editor.h.
#include "../win/editor_pixels.h"
#include "op_editor_x11.h"
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
// The shell's view, driven from the host's run loop.
class LinuxEditorView final : public EditorView, public Linux::ITimerHandler, public Linux::IEventHandler {
public:
    explicit LinuxEditorView(Controller* controller) : EditorView(controller) {}
    ~LinuxEditorView() override { stopDriving(); }

    tresult PLUGIN_API attached(void* parent, FIDString type) SMTG_OVERRIDE {
        // The run loop first: without one the editor would open and never draw.
        FUnknownPtr<Linux::IRunLoop> loop(plugFrame);
        if (!loop) return kResultFalse;
        stopDriving();
        const tresult result = EditorView::attached(parent, type);
        if (result != kResultOk) return result;
        void* editor = ownEditor();
        if (!editor || !startDriving(loop, editor)) {
            EditorView::removed();
            return kResultFalse;
        }
        // The host reads the size next (getSize): that is the size it knows.
        hostW_ = hostH_ = askedW_ = askedH_ = 0;
        op_editor_get_size(editor, &hostW_, &hostH_);
        return kResultOk;
    }
    // A host that embeds through a socket (Ardour's GtkSocket) makes the view's
    // window the size of its own area whatever the view answers, and gives that
    // size here. An editor with a fixed shape takes the width it is given and the
    // height that goes with it; followSize then asks the frame for that height.
    // A size that already has the shape comes out as it went in.
    tresult PLUGIN_API onSize(ViewRect* newSize) SMTG_OVERRIDE {
        if (!newSize) return kInvalidArgument;
        hostW_ = static_cast<uint32_t>(newSize->getWidth() > 0 ? newSize->getWidth() : 1);
        hostH_ = static_cast<uint32_t>(newSize->getHeight() > 0 ? newSize->getHeight() : 1);
        ViewRect shaped = *newSize;
        if (aspectW_ > 0 && aspectH_ > 0) {
            const long height = std::lround(static_cast<double>(hostW_) * aspectH_ / aspectW_);
            shaped.bottom = shaped.top + static_cast<int32>(height < 1 ? 1 : height);
        }
        return EditorView::onSize(&shaped);
    }
    tresult PLUGIN_API removed() SMTG_OVERRIDE {
        // The host stops calling before the view closes its connection.
        stopDriving();
        return EditorView::removed();
    }
    // A host may take its frame away, or give another, while the view is open: the
    // view is driven from the run loop of the frame it has.
    tresult PLUGIN_API setFrame(IPlugFrame* frame) SMTG_OVERRIDE {
        if (frame == plugFrame) return kResultOk;
        const bool driven = loop_ != nullptr;
        stopDriving();
        const tresult result = EditorView::setFrame(frame);
        if (driven) {
            FUnknownPtr<Linux::IRunLoop> loop(plugFrame);
            if (void* editor = ownEditor(); loop && editor) startDriving(loop, editor);
        }
        return result;
    }

    void PLUGIN_API onTimer() SMTG_OVERRIDE { pump(); }
    void PLUGIN_API onFDIsSet(Linux::FileDescriptor) SMTG_OVERRIDE { pump(); }

    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) SMTG_OVERRIDE {
        QUERY_INTERFACE(iid, obj, Linux::ITimerHandler::iid, Linux::ITimerHandler)
        QUERY_INTERFACE(iid, obj, Linux::IEventHandler::iid, Linux::IEventHandler)
        return EditorView::queryInterface(iid, obj);
    }
    uint32 PLUGIN_API addRef() SMTG_OVERRIDE { return EditorView::addRef(); }
    uint32 PLUGIN_API release() SMTG_OVERRIDE { return EditorView::release(); }

private:
    bool startDriving(Linux::IRunLoop* loop, void* editor) {
        if (loop->registerTimer(this, OP_EDITOR_TICK_MS) != kResultOk) return false;
        loop_ = loop;
        // The descriptor makes input prompt; the timer alone still runs the view.
        const int32_t fd = op_editor_fd(editor);
        watching_ = fd >= 0 && loop->registerEventHandler(this, fd) == kResultOk;
        return true;
    }
    void stopDriving() {
        if (!loop_) return;
        if (watching_) loop_->unregisterEventHandler(this);
        loop_->unregisterTimer(this);
        watching_ = false;
        loop_ = nullptr;
    }
    void pump() {
        void* editor = ownEditor();
        if (!editor) {
            // Another view took the editor over: its connection is closed, and
            // nothing is left to watch.
            stopDriving();
            return;
        }
        // The editor may ask for a size inside the pump (requestResize), and the
        // host may release the view from inside a call it makes then.
        IPtr<LinuxEditorView> keep(this);
        op_editor_pump(editor);
        followSize();
    }
    // An X11 child window does not resize its parent. When the editor's size is
    // not the one the host knows (it changed size on its own, its window was given
    // a size directly, or it took another size than the host gave), the frame is
    // asked for it: once for each size, so a host that cannot give it is not
    // asked again on every turn.
    void followSize() {
        void* editor = ownEditor();
        uint32_t width = 0;
        uint32_t height = 0;
        if (!editor || !op_editor_get_size(editor, &width, &height)) return;
        if (width == hostW_ && height == hostH_) {
            askedW_ = askedH_ = 0;
            return;
        }
        if (width == askedW_ && height == askedH_) return;
        askedW_ = width;
        askedH_ = height;
        ViewRect asked(rect.left, rect.top, rect.left + static_cast<int32>(width), rect.top + static_cast<int32>(height));
        rect = asked;
        // The host may call onSize from inside this, which sets what it knows.
        if (plugFrame && plugFrame->resizeView(this, &asked) == kResultTrue && ownEditor() == editor) {
            if (op_editor_get_size(editor, &width, &height)) {
                hostW_ = width;
                hostH_ = height;
            }
        }
    }
    IPtr<Linux::IRunLoop> loop_;
    bool watching_ = false;
    // The size the host has (from getSize after attached, onSize, or a resizeView
    // it accepted) and the last one the frame was asked for, in pixels.
    uint32_t hostW_ = 0, hostH_ = 0, askedW_ = 0, askedH_ = 0;
};

IPlugView* PLUGIN_API Controller::createView(FIDString name) {
    if (!name || std::strcmp(name, ViewType::kEditor) != 0) return nullptr;
    return new LinuxEditorView(this);
}
}
#endif
