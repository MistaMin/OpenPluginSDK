// SPDX-License-Identifier: MIT
// VST3EditorProof on Linux: loads a VST3 module built with -DOP_EDITOR=ON as a host
// does and checks the editor the shell hosts through op_editor_*. The Linux
// counterpart of native/vst3/editor_host.mm, with the same checks and the ones a
// Linux host adds:
//
//   - the view supports X11EmbedWindowID (and not NSView or HWND), has a size, and
//     attaches one child window of exactly that size in pixels, on screen;
//   - the view is driven from the host's run loop: attached() fails when the frame
//     lends no Linux::IRunLoop; with one the view registers one timer and one
//     descriptor, the window shows the editor's picture (so the pump runs), and
//     removed() takes both registrations back;
//   - the processor can be deactivated and activated again with the editor open;
//   - optionally, a click at a point produces paired begin/perform/end edits on one
//     parameter, and the controller then holds the value it reported;
//   - the view resizes as a host resizes it: canResize, checkSizeConstraint (clamped
//     to the editor's range and shape), onSize, getSize, the child window's size,
//     and a click at the scaled position of the Warm toggle edits the parameter
//     while the old position hits nothing;
//   - an editor-initiated size (double-click on the bare plate) reaches
//     IPlugFrame::resizeView, and the view takes the size the frame grants;
//   - a right click on a control asks the host's IComponentHandler3 for its context
//     menu on that parameter and pops it up at the click; a click on the bare plate
//     asks nothing; the controller takes IInfoListener::setChannelContextInfos with
//     the editor open;
//   - the view does not offer IPlugViewContentScaleSupport: its scale is the
//     display's (Xft.dpi);
//   - removed() takes the child window out, and tearing down with the processor
//     first, while a second view is open, does not crash.
//
//   VST3EditorProof <module.vst3 folder> [x y param]   (x, y in editor points from the top left)
//
// Every size here is in pixels, as a host sees them: on a display that asks for a
// scale (Xft.dpi) the view is larger than the editor's points. Clicks are X events
// sent to the editor's window, at pixels. It needs an X display (DISPLAY): a
// desktop session, Xwayland, or Xvfb.
#include "CoreFoundation/CoreFoundation.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstchannelcontextinfo.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstcontextmenu.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <vector>
#include <xcb/xcb.h>

using namespace Steinberg;
using namespace Steinberg::Vst;

#define CHECK(cond)                                                    \
    do {                                                               \
        if (!(cond)) {                                                 \
            std::printf("VST3 editor proof failed line %d: %s\n", __LINE__, #cond); \
            return 1;                                                  \
        }                                                              \
    } while (0)

// The host's parameter menu: records the popup.
struct Menu final : IContextMenu {
    int popups = 0;
    UCoord x = -1, y = -1;
    int32 PLUGIN_API getItemCount() override { return 0; }
    tresult PLUGIN_API getItem(int32, Item&, IContextMenuTarget**) override { return kResultFalse; }
    tresult PLUGIN_API addItem(const Item&, IContextMenuTarget*) override { return kResultOk; }
    tresult PLUGIN_API removeItem(const Item&, IContextMenuTarget*) override { return kResultOk; }
    tresult PLUGIN_API popup(UCoord px, UCoord py) override {
        ++popups;
        x = px;
        y = py;
        return kResultOk;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IContextMenu::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = this;
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

// The host side of parameter editing: records what the controller reports.
struct Handler final : IComponentHandler, IComponentHandler3 {
    int begins = 0, performs = 0, ends = 0;
    // IComponentHandler3: which parameter and view a context menu was asked for.
    int menus = 0;
    ParamID menuParam = 0xffffffff;
    IPlugView* menuView = nullptr;
    Menu menu;
    IContextMenu* PLUGIN_API createContextMenu(IPlugView* view, const ParamID* id) override {
        ++menus;
        menuView = view;
        menuParam = id ? *id : 0xffffffff;
        return &menu;
    }
    ParamID last = 0xffffffff;
    ParamValue value = -1.0;
    tresult PLUGIN_API beginEdit(ParamID) override { ++begins; return kResultOk; }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue v) override {
        ++performs;
        last = id;
        value = v;
        return kResultOk;
    }
    tresult PLUGIN_API endEdit(ParamID) override { ++ends; return kResultOk; }
    tresult PLUGIN_API restartComponent(int32) override { return kResultOk; }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IComponentHandler::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = static_cast<IComponentHandler*>(this);
            return kResultOk;
        }
        if (FUnknownPrivate::iidEqual(iid, IComponentHandler3::iid)) {
            *obj = static_cast<IComponentHandler3*>(this);
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

// The channel context a host sends: a name and a colour.
struct Attributes final : IAttributeList {
    std::map<std::string, int64> ints;
    std::map<std::string, std::u16string> strings;
    tresult PLUGIN_API setInt(AttrID id, int64 v) override { ints[id] = v; return kResultOk; }
    tresult PLUGIN_API getInt(AttrID id, int64& v) override {
        auto it = ints.find(id);
        if (it == ints.end()) return kResultFalse;
        v = it->second;
        return kResultOk;
    }
    tresult PLUGIN_API setFloat(AttrID, double) override { return kResultFalse; }
    tresult PLUGIN_API getFloat(AttrID, double&) override { return kResultFalse; }
    tresult PLUGIN_API setString(AttrID id, const TChar* s) override { strings[id] = reinterpret_cast<const char16_t*>(s); return kResultOk; }
    tresult PLUGIN_API getString(AttrID id, TChar* s, uint32 bytes) override {
        auto it = strings.find(id);
        if (it == strings.end() || bytes < (it->second.size() + 1) * sizeof(TChar)) return kResultFalse;
        std::memcpy(s, it->second.c_str(), (it->second.size() + 1) * sizeof(TChar));
        return kResultOk;
    }
    tresult PLUGIN_API setBinary(AttrID, const void*, uint32) override { return kResultFalse; }
    tresult PLUGIN_API getBinary(AttrID, const void*&, uint32&) override { return kResultFalse; }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IAttributeList::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = this;
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

// The host's frame: records a size request and grants it, as hosts do by calling
// onSize. With `lend` it is also the host's run loop: it counts what a view
// registers and passes each call to the loop the proof runs (cf_compat.cpp).
struct Frame final : IPlugFrame, Linux::IRunLoop {
    int requests = 0;
    int32 width = 0, height = 0;
    bool grant = true;
    bool lend = true;
    int timers = 0;
    int descriptors = 0;
    tresult PLUGIN_API resizeView(IPlugView* v, ViewRect* newSize) override {
        ++requests;
        width = newSize->getWidth();
        height = newSize->getHeight();
        if (!grant) return kResultFalse;
        return v->onSize(newSize);
    }
    tresult PLUGIN_API registerEventHandler(Linux::IEventHandler* handler, Linux::FileDescriptor fd) override {
        const tresult result = op_linux_run_loop()->registerEventHandler(handler, fd);
        if (result == kResultOk) ++descriptors;
        return result;
    }
    tresult PLUGIN_API unregisterEventHandler(Linux::IEventHandler* handler) override {
        const tresult result = op_linux_run_loop()->unregisterEventHandler(handler);
        if (result == kResultOk) --descriptors;
        return result;
    }
    tresult PLUGIN_API registerTimer(Linux::ITimerHandler* handler, Linux::TimerInterval milliseconds) override {
        const tresult result = op_linux_run_loop()->registerTimer(handler, milliseconds);
        if (result == kResultOk) ++timers;
        return result;
    }
    tresult PLUGIN_API unregisterTimer(Linux::ITimerHandler* handler) override {
        const tresult result = op_linux_run_loop()->unregisterTimer(handler);
        if (result == kResultOk) --timers;
        return result;
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IPlugFrame::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = static_cast<IPlugFrame*>(this);
            return kResultOk;
        }
        if (lend && FUnknownPrivate::iidEqual(iid, Linux::IRunLoop::iid)) {
            *obj = static_cast<Linux::IRunLoop*>(this);
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};

// The host's side of the display: its own connection, as a host has.
struct Display {
    xcb_connection_t* conn = nullptr;
    xcb_screen_t* screen = nullptr;
    uint32_t clock = 100000;
    int parents = 0;

    bool open() {
        int number = 0;
        conn = xcb_connect(nullptr, &number);
        if (!conn || xcb_connection_has_error(conn)) return false;
        xcb_screen_iterator_t it = xcb_setup_roots_iterator(xcb_get_setup(conn));
        for (int i = 0; i < number && it.rem; ++i) xcb_screen_next(&it);
        screen = it.data;
        // Hyprland (through Xwayland) remembers a window first made on the root that
        // it did not see destroyed, and later moves a window that reuses the id. The
        // first ids of a connection are the likely ones: they stay unused, and every
        // parent has a place of its own (as in examples/editor/tests/x11.rs).
        for (int i = 0; i < 16; ++i) xcb_generate_id(conn);
        return screen != nullptr;
    }
    // Returns once the server has handled everything sent so far.
    void sync() { std::free(xcb_get_input_focus_reply(conn, xcb_get_input_focus(conn), nullptr)); }

    // Pixels per point as the editor's view reads it: Xft.dpi over 96, from 1 to 3.
    double scale() {
        double result = 1.0;
        xcb_get_property_reply_t* reply =
            xcb_get_property_reply(conn, xcb_get_property(conn, 0, screen->root, XCB_ATOM_RESOURCE_MANAGER, XCB_ATOM_STRING, 0, 1 << 18), nullptr);
        if (!reply) return result;
        const std::string text(static_cast<const char*>(xcb_get_property_value(reply)), static_cast<size_t>(xcb_get_property_value_length(reply)));
        std::free(reply);
        size_t at = 0;
        while (at < text.size()) {
            size_t end = text.find('\n', at);
            if (end == std::string::npos) end = text.size();
            const std::string line = text.substr(at, end - at);
            const size_t start = line.find_first_not_of(" \t");
            if (start != std::string::npos && line.compare(start, 8, "Xft.dpi:") == 0) {
                const double dpi = std::atof(line.c_str() + start + 8);
                if (dpi > 0.0) result = std::min(3.0, std::max(1.0, dpi / 96.0));
            }
            at = end + 1;
        }
        return result;
    }

    // A window of `width` x `height` pixels that no window manager frames: the
    // host's place for the view.
    xcb_window_t makeParent(int width, int height) {
        const int16_t x = static_cast<int16_t>(40 + 24 * parents);
        const int16_t y = static_cast<int16_t>(40 + 24 * parents);
        ++parents;
        const xcb_window_t window = xcb_generate_id(conn);
        const uint32_t values[] = {screen->black_pixel, 1};
        xcb_void_cookie_t made = xcb_create_window_checked(conn, XCB_COPY_FROM_PARENT, window, screen->root, x, y, static_cast<uint16_t>(width),
                                                          static_cast<uint16_t>(height), 0, XCB_WINDOW_CLASS_INPUT_OUTPUT, screen->root_visual,
                                                          XCB_CW_BACK_PIXEL | XCB_CW_OVERRIDE_REDIRECT, values);
        if (xcb_generic_error_t* error = xcb_request_check(conn, made)) {
            std::free(error);
            return 0;
        }
        xcb_map_window(conn, window);
        sync();
        return window;
    }
    void destroy(xcb_window_t window) {
        xcb_destroy_window(conn, window);
        sync();
    }
    std::vector<xcb_window_t> children(xcb_window_t window) {
        std::vector<xcb_window_t> result;
        if (xcb_query_tree_reply_t* reply = xcb_query_tree_reply(conn, xcb_query_tree(conn, window), nullptr)) {
            const xcb_window_t* list = xcb_query_tree_children(reply);
            result.assign(list, list + xcb_query_tree_children_length(reply));
            std::free(reply);
        }
        return result;
    }
    bool exists(xcb_window_t window) {
        xcb_get_geometry_reply_t* reply = xcb_get_geometry_reply(conn, xcb_get_geometry(conn, window), nullptr);
        std::free(reply);
        return reply != nullptr;
    }
    bool sizeIs(xcb_window_t window, int width, int height) {
        xcb_get_geometry_reply_t* reply = xcb_get_geometry_reply(conn, xcb_get_geometry(conn, window), nullptr);
        const bool same = reply && reply->width == width && reply->height == height;
        std::free(reply);
        return same;
    }
    bool mapped(xcb_window_t window) {
        xcb_get_window_attributes_reply_t* reply = xcb_get_window_attributes_reply(conn, xcb_get_window_attributes(conn, window), nullptr);
        const bool yes = reply && reply->map_state == XCB_MAP_STATE_VIEWABLE;
        std::free(reply);
        return yes;
    }
    // Whether the window shows a picture: more than one colour on screen.
    bool painted(xcb_window_t window) {
        xcb_get_geometry_reply_t* g = xcb_get_geometry_reply(conn, xcb_get_geometry(conn, window), nullptr);
        if (!g) return false;
        const uint16_t w = g->width, h = g->height;
        std::free(g);
        xcb_get_image_reply_t* reply = xcb_get_image_reply(conn, xcb_get_image(conn, XCB_IMAGE_FORMAT_Z_PIXMAP, window, 0, 0, w, h, ~0u), nullptr);
        if (!reply) return false;
        const uint8_t* data = xcb_get_image_data(reply);
        const size_t length = static_cast<size_t>(xcb_get_image_data_length(reply));
        const size_t step = w && h ? length / (static_cast<size_t>(w) * h) : 0;
        bool differs = false;
        for (size_t at = step; step && at + step <= length && !differs; at += step) differs = std::memcmp(data, data + at, step) != 0;
        std::free(reply);
        return differs;
    }

    void button(xcb_window_t window, bool press, uint8_t which, double x, double y, uint16_t state, uint32_t time) {
        xcb_button_press_event_t event{};
        event.response_type = press ? XCB_BUTTON_PRESS : XCB_BUTTON_RELEASE;
        event.detail = which;
        event.time = time;
        event.root = screen->root;
        event.event = window;
        event.root_x = event.event_x = static_cast<int16_t>(std::lround(x));
        event.root_y = event.event_y = static_cast<int16_t>(std::lround(y));
        event.state = state;
        event.same_screen = 1;
        xcb_send_event(conn, 0, window, press ? XCB_EVENT_MASK_BUTTON_PRESS : XCB_EVENT_MASK_BUTTON_RELEASE, reinterpret_cast<const char*>(&event));
    }
    // A left click at a point in pixels from the top left; count 2 is a double click.
    // Each call is a second after the last, so two calls are two single clicks.
    void click(xcb_window_t window, double x, double y, int count) {
        clock += 1000;
        for (int i = 0; i < count; ++i) {
            const uint32_t time = clock + static_cast<uint32_t>(i) * 50;
            button(window, true, 1, x, y, 0, time);
            button(window, false, 1, x, y, XCB_BUTTON_MASK_1, time + 1);
        }
        sync();
    }
    void rightClick(xcb_window_t window, double x, double y) {
        clock += 1000;
        button(window, true, 3, x, y, 0, clock);
        button(window, false, 3, x, y, XCB_BUTTON_MASK_3, clock + 1);
        sync();
    }
};

// Runs the host's run loop (the view's timer and descriptor) for `seconds`.
static void spin(double seconds) {
    const auto until = std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
    while (std::chrono::steady_clock::now() < until) CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
}
// Runs it until `done`, for five seconds at most. Whether `done` came true.
template <class Done>
static bool spinUntil(Done done) {
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (!done()) {
        if (std::chrono::steady_clock::now() > deadline) return false;
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, false);
    }
    return true;
}

static void* handle(xcb_window_t window) { return reinterpret_cast<void*>(static_cast<uintptr_t>(window)); }

int main(int argc, char** argv) {
    if (argc != 2 && argc != 5) {
        std::printf("usage: VST3EditorProof <module.vst3 folder> [x y param]\n");
        return 2;
    }
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    Display display;
    if (!display.open()) {
        std::printf("VST3 editor proof: no X display (DISPLAY is %s)\n", std::getenv("DISPLAY") ? std::getenv("DISPLAY") : "not set");
        return 1;
    }
    CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8*>(argv[1]), static_cast<CFIndex>(std::strlen(argv[1])), true);
    CHECK(url);
    CFBundleRef bundle = CFBundleCreate(nullptr, url);
    CFRelease(url);
    CHECK(bundle && CFBundleLoadExecutable(bundle));
    auto entry = reinterpret_cast<bool (*)(CFBundleRef)>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
    CHECK(entry && getFactory && entry(bundle));
    IPluginFactory* factory = getFactory();
    CHECK(factory);
    PClassInfo effect{};
    bool found = false;
    for (int32 i = 0; i < factory->countClasses(); ++i) {
        PClassInfo info{};
        if (factory->getClassInfo(i, &info) == kResultOk && std::strcmp(info.category, kVstAudioEffectClass) == 0) {
            effect = info;
            found = true;
        }
    }
    CHECK(found);
    IComponent* component = nullptr;
    CHECK(factory->createInstance(effect.cid, IComponent::iid, reinterpret_cast<void**>(&component)) == kResultOk && component);
    CHECK(component->initialize(nullptr) == kResultOk);
    IAudioProcessor* processor = nullptr;
    CHECK(component->queryInterface(IAudioProcessor::iid, reinterpret_cast<void**>(&processor)) == kResultOk);
    TUID controllerId{};
    CHECK(component->getControllerClassId(controllerId) == kResultOk);
    IEditController* controller = nullptr;
    CHECK(factory->createInstance(controllerId, IEditController::iid, reinterpret_cast<void**>(&controller)) == kResultOk && controller);
    CHECK(controller->initialize(nullptr) == kResultOk);
    Handler handler;
    Frame plugFrame;
    CHECK(controller->setComponentHandler(&handler) == kResultOk);

    // Connect the two halves, as a host does; the editor finds its engine through this.
    IConnectionPoint* fromComponent = nullptr;
    IConnectionPoint* fromController = nullptr;
    CHECK(component->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromComponent)) == kResultOk);
    CHECK(controller->queryInterface(IConnectionPoint::iid, reinterpret_cast<void**>(&fromController)) == kResultOk);
    CHECK(fromComponent->connect(fromController) == kResultOk && fromController->connect(fromComponent) == kResultOk);

    SpeakerArrangement stereo = SpeakerArr::kStereo;
    CHECK(processor->setBusArrangements(&stereo, 1, &stereo, 1) == kResultOk);
    ProcessSetup setup{kRealtime, kSample32, 512, 48000.0};
    CHECK(processor->setupProcessing(setup) == kResultOk);
    CHECK(component->setActive(true) == kResultOk);
    std::printf("tail %u samples\n", processor->getTailSamples());

    IPlugView* view = controller->createView(ViewType::kEditor);
    CHECK(view);
    CHECK(view->isPlatformTypeSupported(kPlatformTypeX11EmbedWindowID) == kResultTrue);
    CHECK(view->isPlatformTypeSupported(kPlatformTypeNSView) != kResultTrue);
    CHECK(view->isPlatformTypeSupported(kPlatformTypeHWND) != kResultTrue);
    // The scale is the display's; a host is not asked for one.
    CHECK(!FUnknownPtr<IPlugViewContentScaleSupport>(view));
    ViewRect rect{};
    CHECK(view->getSize(&rect) == kResultOk && rect.getWidth() > 0 && rect.getHeight() > 0);
    const xcb_window_t parent = display.makeParent(rect.getWidth(), rect.getHeight());
    CHECK(parent);

    // No frame, then a frame that lends no run loop: nothing could drive the view,
    // so it does not open.
    CHECK(view->attached(handle(parent), kPlatformTypeX11EmbedWindowID) != kResultOk);
    plugFrame.lend = false;
    CHECK(view->setFrame(&plugFrame) == kResultOk);
    CHECK(view->attached(handle(parent), kPlatformTypeX11EmbedWindowID) != kResultOk);
    display.sync();
    CHECK(display.children(parent).empty());
    CHECK(view->setFrame(nullptr) == kResultOk);
    plugFrame.lend = true;
    CHECK(view->setFrame(&plugFrame) == kResultOk);
    std::printf("no run loop: the view does not attach\n");

    CHECK(view->attached(handle(parent), kPlatformTypeX11EmbedWindowID) == kResultOk);
    display.sync();
    CHECK(display.children(parent).size() == 1);
    const xcb_window_t editor = display.children(parent)[0];
    // The size with the editor open is the one that counts: it is at the view's own scale.
    CHECK(view->getSize(&rect) == kResultOk && rect.getWidth() > 0 && rect.getHeight() > 0);
    CHECK(display.sizeIs(editor, rect.getWidth(), rect.getHeight()));
    CHECK(display.mapped(editor));
    CHECK(plugFrame.timers == 1 && plugFrame.descriptors == 1);
    // Pixels per editor point on this display.
    const double scale = display.scale();
    // A point lands within this many pixels of where it was asked for.
    const int slack = static_cast<int>(std::ceil(scale));
    std::printf("editor %dx%d pixels, %.2f pixels per point\n", rect.getWidth(), rect.getHeight(), scale);
    // The run loop drives the view: the editor's picture reaches the window.
    CHECK(spinUntil([&] { return display.painted(editor); }));
    std::printf("the host's run loop drives the view (1 timer, 1 descriptor): the window shows the editor\n");

    // The engine is destroyed and made again with the editor open.
    CHECK(component->setActive(false) == kResultOk);
    spin(0.1);
    CHECK(component->setActive(true) == kResultOk);
    spin(0.1);

    // The host's parameter menu. The control is the one the click and resize checks use.
    const double mx = (argc == 5 ? std::atof(argv[2]) : 135.0) * scale;
    const double my = (argc == 5 ? std::atof(argv[3]) : 68.0) * scale;
    const ParamID mparam = argc == 5 ? static_cast<ParamID>(std::atoi(argv[4])) : 1;
    {
        const int begins = handler.begins;
        display.rightClick(editor, mx, my);
        CHECK(spinUntil([&] { return handler.menus == 1; }));
        CHECK(handler.menuParam == mparam && handler.menuView == view);
        CHECK(handler.menu.popups == 1);
        CHECK(std::abs(handler.menu.x - static_cast<UCoord>(std::lround(mx))) <= slack && std::abs(handler.menu.y - static_cast<UCoord>(std::lround(my))) <= slack);
        // Not an edit, and the bare plate has no menu.
        display.rightClick(editor, 5 * scale, 5 * scale);
        spin(0.2);
        CHECK(handler.menus == 1 && handler.begins == begins);
        std::printf("host menu: a right click opens the host's menu for parameter %u at the click\n", handler.menuParam);
        // The host tells the track while the editor is open.
        Attributes info;
        info.strings[ChannelContext::kChannelNameKey] = u"Lead ♪";
        info.ints[ChannelContext::kChannelColorKey] = 0xFF336699;
        FUnknownPtr<ChannelContext::IInfoListener> listener(controller);
        CHECK(listener && listener->setChannelContextInfos(&info) == kResultOk);
        CHECK(listener->setChannelContextInfos(nullptr) == kInvalidArgument);
        spin(0.05);
    }

    if (argc == 5) {
        const int begins = handler.begins;
        display.click(editor, mx, my, 1);
        CHECK(spinUntil([&] { return handler.ends > 0 && handler.ends == handler.begins && handler.begins > begins; }));
        std::printf("click: %d begin / %d perform / %d end, parameter %u = %.4f\n", handler.begins, handler.performs, handler.ends, handler.last, handler.value);
        CHECK(handler.begins > 0 && handler.begins == handler.ends && handler.performs >= handler.begins);
        CHECK(handler.last == mparam);
        CHECK(controller->getParamNormalized(mparam) == handler.value);
    }

    // Resizing, as a host does. The range comes from the view itself.
    CHECK(view->canResize() == kResultTrue);
    ViewRect big(0, 0, 100000, 100000);
    CHECK(view->checkSizeConstraint(&big) == kResultOk);
    ViewRect small(0, 0, 1, 1);
    CHECK(view->checkSizeConstraint(&small) == kResultOk);
    std::printf("range %dx%d to %dx%d\n", small.getWidth(), small.getHeight(), big.getWidth(), big.getHeight());
    CHECK(big.getWidth() > rect.getWidth() && small.getWidth() < rect.getWidth());
    // A size off the shape is brought to the shape.
    ViewRect odd(0, 0, big.getWidth(), small.getHeight() + (big.getHeight() - small.getHeight()) / 2);
    CHECK(view->checkSizeConstraint(&odd) == kResultOk);
    CHECK(std::abs(odd.getWidth() * big.getHeight() - odd.getHeight() * big.getWidth()) <= big.getWidth() * (slack + 1));
    // A host that gives its own width and leaves the height (Ardour: the view's
    // window is in a socket as wide as the host's toolbar). The view takes the
    // width and the height of its shape, and asks the frame for that size once.
    {
        const int32 wideWidth = std::min(rect.getWidth() * 3 / 2, big.getWidth());
        const auto shaped = [&](const ViewRect& r) {
            return r.getWidth() == wideWidth && std::abs(r.getWidth() * big.getHeight() - r.getHeight() * big.getWidth()) <= big.getWidth() * (slack + 1);
        };
        ViewRect got{};
        // A frame that cannot give the height is asked once, not on every turn.
        plugFrame.grant = false;
        int count = plugFrame.requests;
        ViewRect wide(0, 0, wideWidth, rect.getHeight());
        CHECK(view->onSize(&wide) == kResultOk);
        CHECK(view->getSize(&got) == kResultOk && shaped(got));
        CHECK(spinUntil([&] { return plugFrame.requests == count + 1; }));
        CHECK(plugFrame.width == got.getWidth() && plugFrame.height == got.getHeight());
        spin(0.2);
        CHECK(plugFrame.requests == count + 1);
        plugFrame.grant = true;
        ViewRect back(0, 0, rect.getWidth(), rect.getHeight());
        CHECK(view->onSize(&back) == kResultOk);
        spin(0.1);
        CHECK(plugFrame.requests == count + 1);
        // A frame that gives it: asked once, and the window has the shape.
        count = plugFrame.requests;
        wide = ViewRect(0, 0, wideWidth, rect.getHeight());
        CHECK(view->onSize(&wide) == kResultOk);
        CHECK(spinUntil([&] { return plugFrame.requests == count + 1; }));
        CHECK(view->getSize(&got) == kResultOk && shaped(got));
        CHECK(plugFrame.width == got.getWidth() && plugFrame.height == got.getHeight());
        CHECK(spinUntil([&] { return display.sizeIs(editor, got.getWidth(), got.getHeight()); }));
        spin(0.2);
        CHECK(plugFrame.requests == count + 1);
        std::printf("host gave %dx%d, view took %dx%d and asked the frame once\n", wideWidth, rect.getHeight(), got.getWidth(), got.getHeight());
        back = ViewRect(0, 0, rect.getWidth(), rect.getHeight());
        CHECK(view->onSize(&back) == kResultOk);
        CHECK(spinUntil([&] { return display.sizeIs(editor, rect.getWidth(), rect.getHeight()); }));
    }
    // The click point scales with the view: examples/editor keeps its shape from the origin.
    const double factor = double(big.getWidth()) / rect.getWidth();
    const int asked = plugFrame.requests;
    ViewRect to = big;
    CHECK(view->onSize(&to) == kResultOk);
    ViewRect now{};
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == big.getWidth() && now.getHeight() == big.getHeight());
    CHECK(spinUntil([&] { return display.sizeIs(editor, big.getWidth(), big.getHeight()); }));
    std::printf("resized to %dx%d\n", now.getWidth(), now.getHeight());
    // The old position of the control hits nothing at this size...
    const int before = handler.begins;
    display.click(editor, mx, my, 1);
    spin(0.2);
    CHECK(handler.begins == before);
    // ...and the scaled one hits it.
    display.click(editor, mx * factor, my * factor, 1);
    CHECK(spinUntil([&] { return handler.begins == before + 1 && handler.ends == handler.begins; }));
    std::printf("scaled click: %d begin / %d end, parameter %u\n", handler.begins, handler.ends, handler.last);
    CHECK(handler.last == mparam);
    // Smaller than the range: the view stays at the smallest size.
    ViewRect tiny(0, 0, 10, 10);
    CHECK(view->onSize(&tiny) == kResultOk);
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == small.getWidth() && now.getHeight() == small.getHeight());
    ViewRect open(0, 0, rect.getWidth(), rect.getHeight());
    CHECK(view->onSize(&open) == kResultOk);
    CHECK(spinUntil([&] { return display.sizeIs(editor, rect.getWidth(), rect.getHeight()); }));
    spin(0.1);
    // A size the host set is not asked back of it.
    CHECK(plugFrame.requests == asked);
    // The editor asks for a size: a double-click on the bare plate.
    display.click(editor, 5 * scale, 5 * scale, 2);
    CHECK(spinUntil([&] { return plugFrame.requests == asked + 1; }));
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == plugFrame.width && now.getHeight() == plugFrame.height);
    CHECK(plugFrame.width > rect.getWidth());
    CHECK(spinUntil([&] { return display.sizeIs(editor, plugFrame.width, plugFrame.height); }));
    spin(0.1);
    CHECK(plugFrame.requests == asked + 1);
    std::printf("editor asked for %dx%d, view is %dx%d\n", plugFrame.width, plugFrame.height, now.getWidth(), now.getHeight());
    // A frame that refuses leaves the size alone.
    plugFrame.grant = false;
    display.click(editor, 5 * scale, 5 * scale, 2);
    CHECK(spinUntil([&] { return plugFrame.requests == asked + 2; }));
    spin(0.1);
    CHECK(plugFrame.requests == asked + 2);
    ViewRect after{};
    CHECK(view->getSize(&after) == kResultOk && after.getWidth() == now.getWidth() && after.getHeight() == now.getHeight());
    CHECK(display.sizeIs(editor, now.getWidth(), now.getHeight()));
    plugFrame.grant = true;

    CHECK(view->removed() == kResultOk);
    display.sync();
    CHECK(display.children(parent).empty());
    CHECK(!display.exists(editor));
    CHECK(plugFrame.timers == 0 && plugFrame.descriptors == 0);
    CHECK(view->setFrame(nullptr) == kResultOk);
    view->release();

    // Two views on one controller: the second replaces the first's editor.
    // Removing and releasing the first must leave the second's editor alone.
    {
        const xcb_window_t secondParent = display.makeParent(rect.getWidth(), rect.getHeight());
        CHECK(secondParent);
        Frame firstFrame, secondFrame;
        IPlugView* first = controller->createView(ViewType::kEditor);
        CHECK(first && first->setFrame(&firstFrame) == kResultOk);
        CHECK(first->attached(handle(parent), kPlatformTypeX11EmbedWindowID) == kResultOk);
        IPlugView* second = controller->createView(ViewType::kEditor);
        CHECK(second && second->setFrame(&secondFrame) == kResultOk);
        CHECK(second->attached(handle(secondParent), kPlatformTypeX11EmbedWindowID) == kResultOk);
        display.sync();
        CHECK(display.children(secondParent).size() == 1);
        // The first view has no editor left to drive, and stops asking to be called.
        CHECK(spinUntil([&] { return firstFrame.timers == 0 && firstFrame.descriptors == 0; }));
        CHECK(first->removed() == kResultOk);
        CHECK(first->setFrame(nullptr) == kResultOk);
        first->release();
        spin(0.05);
        CHECK(display.children(secondParent).size() == 1);
        CHECK(secondFrame.timers == 1 && secondFrame.descriptors == 1);
        ViewRect size{};
        CHECK(second->getSize(&size) == kResultOk && size.getWidth() > 0);
        const int begins = handler.begins;
        display.click(display.children(secondParent)[0], mx * size.getWidth() / rect.getWidth(), my * size.getHeight() / rect.getHeight(), 1);
        CHECK(spinUntil([&] { return handler.begins == begins + 1; }));
        std::printf("second view after the first closed: %d begin\n", handler.begins - begins);
        CHECK(second->removed() == kResultOk);
        display.sync();
        CHECK(display.children(secondParent).empty());
        CHECK(secondFrame.timers == 0 && secondFrame.descriptors == 0);
        CHECK(second->setFrame(nullptr) == kResultOk);
        second->release();
        display.destroy(secondParent);
    }
    // Open once more, then tear down with the processor going first.
    view = controller->createView(ViewType::kEditor);
    CHECK(view && view->setFrame(&plugFrame) == kResultOk);
    CHECK(view->attached(handle(parent), kPlatformTypeX11EmbedWindowID) == kResultOk);
    CHECK(component->setActive(false) == kResultOk);
    fromComponent->disconnect(fromController);
    fromController->disconnect(fromComponent);
    fromComponent->release();
    fromController->release();
    processor->release();
    CHECK(component->terminate() == kResultOk);
    component->release();
    spin(0.2);
    CHECK(view->removed() == kResultOk);
    CHECK(plugFrame.timers == 0 && plugFrame.descriptors == 0);
    CHECK(view->setFrame(nullptr) == kResultOk);
    view->release();
    CHECK(controller->terminate() == kResultOk);
    controller->release();
    factory->release();
    display.destroy(parent);
    xcb_disconnect(display.conn);
    std::printf("VST3 editor proof passed\n");
    return 0;
}
