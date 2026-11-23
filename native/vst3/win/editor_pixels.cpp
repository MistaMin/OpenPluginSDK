// SPDX-License-Identifier: MIT
// See editor_pixels.h. Main thread only, like every op_editor_* call.
// The Linux VST3 shell (vst3/linux) uses this file too: there the system's
// scale is the display's (op_editor_display_scale, op_editor_x11.h).
#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#else
#include "op_editor_x11.h"
#endif
#include "editor_pixels.h"
#include <cmath>
#include <cstddef>
#include <initializer_list>
#include <vector>

namespace openplugin_vst_win {
namespace {
// What the shell asked to hear from one editor. The editor calls the functions
// below with the Link as its context; they convert and call the shell.
struct Link {
    void* editor = nullptr;
    void* resizeContext = nullptr;
    OpEditorRequestResize resize = nullptr;
    void* menuContext = nullptr;
    OpEditorContextMenu menu = nullptr;
};
// One per open editor: a host has a handful at most.
std::vector<Link*>& links() {
    static std::vector<Link*> all;
    return all;
}
Link* linkOf(void* editor, bool make) {
    for (Link* link : links()) {
        if (link->editor == editor) return link;
    }
    if (!make) return nullptr;
    Link* link = new Link;
    link->editor = editor;
    links().push_back(link);
    return link;
}

double gHostScale = 0.0;
// The scale to expect before an editor has a window: the host's, else the system's.
double systemScale() {
    if (gHostScale > 0.0) return gHostScale;
#ifdef _WIN32
    const UINT dpi = GetDpiForSystem();
    return dpi ? static_cast<double>(dpi) / 96.0 : 1.0;
#else
    // Asked once: it opens a connection to the X server.
    static const double display = op_editor_display_scale();
    return display > 0.0 ? display : 1.0;
#endif
}
double scaleOf(void* editor) {
    const double scale = editor ? op_editor_scale(editor) : 0.0;
    return scale > 0.0 ? scale : systemScale();
}
// The rounding of the view itself (openplugin-dri win32.rs to_pixels, to_points).
uint32_t toPixels(uint32_t points, double scale) {
    const long value = std::lround(static_cast<double>(points) * scale);
    return value < 1 ? 1u : static_cast<uint32_t>(value);
}
uint32_t toPoints(uint32_t pixels, double scale) {
    const long value = std::lround(static_cast<double>(pixels) / scale);
    return value < 1 ? 1u : static_cast<uint32_t>(value);
}

uint32_t requestResize(void* context, uint32_t width, uint32_t height) {
    auto* link = static_cast<Link*>(context);
    if (!link->resize) return 0;
    const double scale = scaleOf(link->editor);
    return link->resize(link->resizeContext, toPixels(width, scale), toPixels(height, scale));
}
uint32_t showMenu(void* context, uint32_t index, int32_t x, int32_t y) {
    auto* link = static_cast<Link*>(context);
    if (!link->menu) return 0;
    const double scale = scaleOf(link->editor);
    return link->menu(link->menuContext, index, static_cast<int32_t>(std::lround(x * scale)), static_cast<int32_t>(std::lround(y * scale)));
}
}

uint32_t editor_size(uint32_t* width, uint32_t* height) {
    if (!width || !height) return 0;
    const uint32_t ok = op_editor_size(width, height);
    const double scale = systemScale();
    *width = toPixels(*width, scale);
    *height = toPixels(*height, scale);
    return ok;
}

uint32_t editor_size_range(uint32_t* min_width, uint32_t* min_height, uint32_t* max_width, uint32_t* max_height,
                           uint32_t* aspect_width, uint32_t* aspect_height) {
    if (!op_editor_size_range(min_width, min_height, max_width, max_height, aspect_width, aspect_height)) return 0;
    const double scale = systemScale();
    for (uint32_t* value : {min_width, min_height, max_width, max_height}) {
        if (value) *value = toPixels(*value, scale);
    }
    return 1;
}

uint32_t editor_get_size(void* editor, uint32_t* width, uint32_t* height) {
    if (!width || !height || !op_editor_get_size(editor, width, height)) return 0;
    const double scale = scaleOf(editor);
    *width = toPixels(*width, scale);
    *height = toPixels(*height, scale);
    return 1;
}

uint32_t editor_adjust_size(void* editor, uint32_t* width, uint32_t* height) {
    if (!editor || !width || !height) return 0;
    const double scale = scaleOf(editor);
    uint32_t w = toPoints(*width, scale);
    uint32_t h = toPoints(*height, scale);
    if (!op_editor_adjust_size(editor, &w, &h)) return 0;
    *width = toPixels(w, scale);
    *height = toPixels(h, scale);
    return 1;
}

uint32_t editor_resize(void* editor, uint32_t width, uint32_t height) { return editor ? op_editor_set_pixel_size(editor, width, height) : 0; }

void editor_set_request_resize(void* editor, void* context, OpEditorRequestResize request) {
    if (!editor) return;
    Link* link = linkOf(editor, true);
    link->resizeContext = context;
    link->resize = request;
    op_editor_set_request_resize(editor, link, request ? requestResize : nullptr);
}

void editor_set_context_menu(void* editor, void* context, OpEditorContextMenu show) {
    if (!editor) return;
    Link* link = linkOf(editor, true);
    link->menuContext = context;
    link->menu = show;
    op_editor_set_context_menu(editor, link, show ? showMenu : nullptr);
}

void editor_set_host_scale(double scale) { gHostScale = scale > 0.0 ? scale : 0.0; }

uint32_t editor_attach(void* editor, void* parent) {
    if (editor && gHostScale > 0.0) op_editor_set_scale(editor, gHostScale);
    return op_editor_attach(editor, parent);
}

void editor_destroy(void* editor) {
    // The editor first: it may still call out while it closes.
    op_editor_destroy(editor);
    auto& all = links();
    for (size_t i = 0; i < all.size(); ++i) {
        if (all[i]->editor == editor) {
            delete all[i];
            all.erase(all.begin() + static_cast<std::ptrdiff_t>(i));
            break;
        }
    }
}
}
