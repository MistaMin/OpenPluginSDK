// SPDX-License-Identifier: MIT
// VST3EditorProof on Windows: loads a VST3 module built with -DOP_EDITOR=ON as a
// host does and checks the editor the shell hosts through op_editor_*. The Windows
// counterpart of native/vst3/editor_host.mm, with the same checks:
//
//   - the view supports HWND (and not NSView), has a size, and attaches one child
//     window of exactly that size in pixels;
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
//   - the host's scale (IPlugViewContentScaleSupport): given before the view is
//     attached, the view opens at points x scale pixels; given while it is open, the
//     window takes the new size at once, the frame is asked for it, and a click at
//     the scaled position still finds the control; a scale of 0 is refused;
//   - removed() takes the child window out, and tearing down with the processor
//     first, while a second view is open, does not crash.
//
//   VST3EditorProof <module.vst3 folder> [x y param]   (x, y in editor points from the top left)
//
// The process is per-monitor DPI aware, so on a scaled display the view is larger
// than the editor's points and every size here is in pixels, as a host sees them.
// Clicks are window messages sent to the editor's child window, at pixels. It needs
// a desktop to make windows on (a logged-in session), like the macOS proof.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/gui/iplugviewcontentscalesupport.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstchannelcontextinfo.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivstcontextmenu.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstmessage.h"
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>

// windows.h names these; the proof uses them as plain words.
#undef small

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
    int32 getItemCount() override { return 0; }
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

// The host's frame: records the request and grants it, as hosts do by calling onSize.
struct Frame final : IPlugFrame {
    IPlugView* view = nullptr;
    int requests = 0;
    int32 width = 0, height = 0;
    bool grant = true;
    tresult PLUGIN_API resizeView(IPlugView* v, ViewRect* newSize) override {
        ++requests;
        width = newSize->getWidth();
        height = newSize->getHeight();
        if (!grant) return kResultFalse;
        return v->onSize(newSize);
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void** obj) override {
        if (FUnknownPrivate::iidEqual(iid, IPlugFrame::iid) || FUnknownPrivate::iidEqual(iid, FUnknown::iid)) {
            *obj = this;
            return kResultOk;
        }
        *obj = nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
};


#if defined(_M_ARM64EC)
static const wchar_t* const kArchFolder = L"arm64ec-win";
#elif defined(_M_ARM64)
static const wchar_t* const kArchFolder = L"arm64-win";
#else
static const wchar_t* const kArchFolder = L"x86_64-win";
#endif

// `<folder>\Contents\<arch>-win\<name of the folder>`, the module of a VST3 folder.
static HMODULE loadModule(const char* folder) {
    wchar_t wide[4096];
    if (!MultiByteToWideChar(CP_ACP, 0, folder, -1, wide, 4096)) return nullptr;
    std::wstring path(wide);
    for (wchar_t& c : path) {
        if (c == L'/') c = L'\\';
    }
    while (!path.empty() && path.back() == L'\\') path.pop_back();
    const size_t slash = path.find_last_of(L'\\');
    const std::wstring name = slash == std::wstring::npos ? path : path.substr(slash + 1);
    const std::wstring file = path + L"\\Contents\\" + kArchFolder + L"\\" + name;
    wchar_t full[4096];
    const DWORD length = GetFullPathNameW(file.c_str(), 4096, full, nullptr);
    if (length == 0 || length >= 4096) return nullptr;
    return LoadLibraryExW(full, nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
}

// A top-level window whose client area is `width` x `height` pixels: the host's place for the view.
static HWND makeParent(int width, int height) {
    static bool registered = false;
    const wchar_t* const name = L"OpenPluginVST3EditorProof";
    if (!registered) {
        WNDCLASSW wc{};
        wc.lpfnWndProc = DefWindowProcW;
        wc.hInstance = GetModuleHandleW(nullptr);
        wc.lpszClassName = name;
        if (!RegisterClassW(&wc)) return nullptr;
        registered = true;
    }
    // A popup has no frame, so the window is its client area.
    HWND window = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, name, L"VST3 editor proof", WS_POPUP | WS_CLIPCHILDREN, 40, 40, width, height,
                                  nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
    if (window) ShowWindow(window, SW_SHOWNOACTIVATE);
    return window;
}

static int childCount(HWND parent) {
    int count = 0;
    for (HWND child = GetWindow(parent, GW_CHILD); child; child = GetWindow(child, GW_HWNDNEXT)) ++count;
    return count;
}
static bool clientIs(HWND window, int width, int height) {
    RECT rc{};
    return GetClientRect(window, &rc) && rc.right - rc.left == width && rc.bottom - rc.top == height;
}

// Dispatches this thread's messages (the editor's timer and paints) for `seconds`.
static void spin(double seconds) {
    const ULONGLONG until = GetTickCount64() + static_cast<ULONGLONG>(seconds * 1000.0 + 0.5);
    for (;;) {
        MSG message;
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        const ULONGLONG now = GetTickCount64();
        if (now >= until) return;
        MsgWaitForMultipleObjectsEx(0, nullptr, static_cast<DWORD>(until - now), QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

static LPARAM at(double x, double y) { return MAKELPARAM(static_cast<int>(std::lround(x)), static_cast<int>(std::lround(y))); }
// A left click at a point in pixels from the top left; count 2 is the second click of a double-click.
static void click(HWND editor, double x, double y, int count) {
    SendMessageW(editor, count == 2 ? WM_LBUTTONDBLCLK : WM_LBUTTONDOWN, MK_LBUTTON, at(x, y));
    SendMessageW(editor, WM_LBUTTONUP, 0, at(x, y));
}
static void rightClick(HWND editor, double x, double y) {
    SendMessageW(editor, WM_RBUTTONDOWN, MK_RBUTTON, at(x, y));
    SendMessageW(editor, WM_RBUTTONUP, 0, at(x, y));
}

int main(int argc, char** argv) {
    if (argc != 2 && argc != 5) {
        std::printf("usage: VST3EditorProof <module.vst3 folder> [x y param]\n");
        return 2;
    }
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    HMODULE module = loadModule(argv[1]);
    CHECK(module);
    auto entry = reinterpret_cast<bool (*)()>(GetProcAddress(module, "InitDll"));
    auto getFactory = reinterpret_cast<IPluginFactory* (*)()>(GetProcAddress(module, "GetPluginFactory"));
    CHECK(entry && getFactory && entry());
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
    CHECK(view->isPlatformTypeSupported(kPlatformTypeHWND) == kResultTrue);
    CHECK(view->isPlatformTypeSupported(kPlatformTypeNSView) != kResultTrue);
    ViewRect rect{};
    CHECK(view->getSize(&rect) == kResultOk && rect.getWidth() > 0 && rect.getHeight() > 0);
    HWND parent = makeParent(rect.getWidth(), rect.getHeight());
    CHECK(parent);
    CHECK(view->attached(parent, kPlatformTypeHWND) == kResultOk);
    CHECK(childCount(parent) == 1);
    HWND editor = GetWindow(parent, GW_CHILD);
    // The size with the editor open is the one that counts: it is at the window's own scale.
    CHECK(view->getSize(&rect) == kResultOk && rect.getWidth() > 0 && rect.getHeight() > 0);
    CHECK(clientIs(editor, rect.getWidth(), rect.getHeight()));
    // Pixels per editor point in this window.
    const double scale = GetDpiForWindow(parent) / 96.0;
    // A point lands within this many pixels of where it was asked for.
    const int slack = static_cast<int>(std::ceil(scale));
    std::printf("editor %dx%d pixels, %.2f pixels per point\n", rect.getWidth(), rect.getHeight(), scale);
    spin(0.2);

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
        rightClick(editor, mx, my);
        spin(0.05);
        CHECK(handler.menus == 1 && handler.menuParam == mparam && handler.menuView == view);
        CHECK(handler.menu.popups == 1);
        CHECK(std::abs(handler.menu.x - static_cast<UCoord>(std::lround(mx))) <= slack && std::abs(handler.menu.y - static_cast<UCoord>(std::lround(my))) <= slack);
        // Not an edit, and the bare plate has no menu.
        rightClick(editor, 5 * scale, 5 * scale);
        spin(0.05);
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
        click(editor, mx, my, 1);
        spin(0.1);
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
    // The click point scales with the view: examples/editor keeps its shape from the origin.
    const double factor = double(big.getWidth()) / rect.getWidth();
    ViewRect to = big;
    CHECK(view->onSize(&to) == kResultOk);
    ViewRect now{};
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == big.getWidth() && now.getHeight() == big.getHeight());
    spin(0.05);
    CHECK(clientIs(editor, big.getWidth(), big.getHeight()));
    std::printf("resized to %dx%d\n", now.getWidth(), now.getHeight());
    // The old position of the control hits nothing at this size...
    const int before = handler.begins;
    click(editor, mx, my, 1);
    spin(0.05);
    CHECK(handler.begins == before);
    // ...and the scaled one hits it.
    click(editor, mx * factor, my * factor, 1);
    spin(0.05);
    std::printf("scaled click: %d begin / %d end, parameter %u\n", handler.begins, handler.ends, handler.last);
    CHECK(handler.begins == before + 1 && handler.ends == handler.begins && handler.last == mparam);
    // Smaller than the range: the view stays at the smallest size.
    ViewRect tiny(0, 0, 10, 10);
    CHECK(view->onSize(&tiny) == kResultOk);
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == small.getWidth() && now.getHeight() == small.getHeight());
    // The editor asks for a size: a double-click on the bare plate.
    ViewRect open(0, 0, rect.getWidth(), rect.getHeight());
    CHECK(view->onSize(&open) == kResultOk);
    CHECK(view->setFrame(&plugFrame) == kResultOk);
    click(editor, 5 * scale, 5 * scale, 2);
    spin(0.05);
    CHECK(plugFrame.requests == 1);
    CHECK(view->getSize(&now) == kResultOk && now.getWidth() == plugFrame.width && now.getHeight() == plugFrame.height);
    CHECK(plugFrame.width > rect.getWidth());
    CHECK(clientIs(editor, plugFrame.width, plugFrame.height));
    std::printf("editor asked for %dx%d, view is %dx%d\n", plugFrame.width, plugFrame.height, now.getWidth(), now.getHeight());
    // A frame that refuses leaves the size alone.
    plugFrame.grant = false;
    click(editor, 5 * scale, 5 * scale, 2);
    spin(0.05);
    CHECK(plugFrame.requests == 2);
    ViewRect after{};
    CHECK(view->getSize(&after) == kResultOk && after.getWidth() == now.getWidth() && after.getHeight() == now.getHeight());
    CHECK(view->setFrame(nullptr) == kResultOk);

    CHECK(view->removed() == kResultOk);
    CHECK(childCount(parent) == 0);
    CHECK(!IsWindow(editor));
    view->release();

    // Two views on one controller: the second replaces the first's editor.
    // Removing and releasing the first must leave the second's editor alone.
    {
        HWND secondParent = makeParent(rect.getWidth(), rect.getHeight());
        CHECK(secondParent);
        IPlugView* first = controller->createView(ViewType::kEditor);
        CHECK(first && first->attached(parent, kPlatformTypeHWND) == kResultOk);
        IPlugView* second = controller->createView(ViewType::kEditor);
        CHECK(second && second->attached(secondParent, kPlatformTypeHWND) == kResultOk);
        CHECK(childCount(secondParent) == 1);
        CHECK(first->removed() == kResultOk);
        first->release();
        spin(0.05);
        CHECK(childCount(secondParent) == 1);
        ViewRect size{};
        CHECK(second->getSize(&size) == kResultOk && size.getWidth() > 0);
        const int begins = handler.begins;
        click(GetWindow(secondParent, GW_CHILD), mx * size.getWidth() / rect.getWidth(), my * size.getHeight() / rect.getHeight(), 1);
        spin(0.05);
        std::printf("second view after the first closed: %d begin\n", handler.begins - begins);
        CHECK(handler.begins == begins + 1);
        CHECK(second->removed() == kResultOk);
        CHECK(childCount(secondParent) == 0);
        second->release();
        DestroyWindow(secondParent);
    }
    // The host's scale. 2.0 before the view is attached, then 1.5 while it is open.
    {
        const int pointsW = static_cast<int>(std::lround(rect.getWidth() / scale));
        const int pointsH = static_cast<int>(std::lround(rect.getHeight() / scale));
        const double pointX = mx / scale, pointY = my / scale;
        IPlugView* scaled = controller->createView(ViewType::kEditor);
        CHECK(scaled);
        FUnknownPtr<IPlugViewContentScaleSupport> support(scaled);
        CHECK(support);
        CHECK(support->setContentScaleFactor(0.0f) != kResultTrue);
        CHECK(support->setContentScaleFactor(2.0f) == kResultTrue);
        ViewRect size{};
        CHECK(scaled->getSize(&size) == kResultOk && size.getWidth() == pointsW * 2 && size.getHeight() == pointsH * 2);
        HWND scaledParent = makeParent(size.getWidth(), size.getHeight());
        CHECK(scaledParent);
        CHECK(scaled->attached(scaledParent, kPlatformTypeHWND) == kResultOk);
        HWND child = GetWindow(scaledParent, GW_CHILD);
        CHECK(child && clientIs(child, pointsW * 2, pointsH * 2));
        CHECK(scaled->getSize(&size) == kResultOk && size.getWidth() == pointsW * 2 && size.getHeight() == pointsH * 2);
        spin(0.05);
        int begins = handler.begins;
        click(child, pointX * 2.0, pointY * 2.0, 1);
        spin(0.05);
        CHECK(handler.begins == begins + 1 && handler.last == mparam);
        std::printf("scale 2.0 before attach: %dx%d points open as %dx%d pixels\n", pointsW, pointsH, size.getWidth(), size.getHeight());
        // The scale changes with the view open: the frame hears the new size.
        Frame scaleFrame;
        CHECK(scaled->setFrame(&scaleFrame) == kResultOk);
        CHECK(support->setContentScaleFactor(1.5f) == kResultTrue);
        spin(0.05);
        const int w15 = static_cast<int>(std::lround(pointsW * 1.5)), h15 = static_cast<int>(std::lround(pointsH * 1.5));
        CHECK(scaleFrame.requests == 1 && scaleFrame.width == w15 && scaleFrame.height == h15);
        CHECK(scaled->getSize(&size) == kResultOk && size.getWidth() == w15 && size.getHeight() == h15);
        CHECK(clientIs(child, w15, h15));
        begins = handler.begins;
        click(child, pointX * 1.5, pointY * 1.5, 1);
        spin(0.05);
        CHECK(handler.begins == begins + 1 && handler.last == mparam);
        // The same scale again changes nothing and asks nothing.
        CHECK(support->setContentScaleFactor(1.5f) == kResultTrue);
        CHECK(scaleFrame.requests == 1);
        std::printf("scale 1.5 while open: the frame was asked for %dx%d, the window is %dx%d\n", scaleFrame.width, scaleFrame.height, size.getWidth(), size.getHeight());
        CHECK(scaled->setFrame(nullptr) == kResultOk);
        CHECK(scaled->removed() == kResultOk);
        scaled->release();
        DestroyWindow(scaledParent);
    }
    // Open once more, then tear down with the processor going first.
    view = controller->createView(ViewType::kEditor);
    CHECK(view && view->attached(parent, kPlatformTypeHWND) == kResultOk);
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
    view->release();
    CHECK(controller->terminate() == kResultOk);
    controller->release();
    DestroyWindow(parent);
    std::printf("VST3 editor proof passed\n");
    return 0;
}
