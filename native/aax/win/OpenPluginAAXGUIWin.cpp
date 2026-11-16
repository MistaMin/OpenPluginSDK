// SPDX-License-Identifier: MIT
// The AAX view on Windows, built in place of aax/OpenPluginAAXGUI.cpp when the
// shell hosts the editor (OP_EDITOR): the plugin's op_editor_* editor as a child
// window of the HWND the host gives (AAX_eViewContainer_Type_HWND).
//
// Sizes: the editor works in points and its window is points x scale pixels, where
// the scale is the DPI of the host's window over 96 (op_editor_scale). AAX sizes
// on Windows are pixels, so GetViewSize and SetViewSize carry pixels. The host
// has no call that tells a view its scale changed; TimerWakeup notices it (the
// window moved to another display) and asks the host for the new pixel size.
#include "OpenPluginAAX.h"
#include "AAX_CEffectGUI.h"
#include "AAX_IEffectParameters.h"
#include "AAX_IParameter.h"
#include "AAX_IViewContainer.h"
#include "op_editor.h"
#include <cmath>
#include <string>
#include <vector>
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

extern "C" {
// Windows part of the editor ABI (crates/openplugin-targets/src/shell.rs): pixels per
// point in use, the host window's DPI over 96 once the view is attached. 0 for no editor.
double op_editor_scale(void* editor);
}

namespace openplugin_aax {
namespace {
uint32_t toPixels(uint32_t points, double scale) {
    const double pixels = std::round(static_cast<double>(points) * scale);
    return pixels < 1.0 ? 1u : static_cast<uint32_t>(pixels);
}
}

class Gui final : public AAX_CEffectGUI {
public:
    static AAX_IEffectGUI* AAX_CALLBACK create() { return new Gui; }
    ~Gui() override { close(); }

    // The host's timer, on the main thread. The scale follows the display the
    // window is on; when it moved, the view's pixel size did too.
    AAX_Result TimerWakeup() override {
        if (editor_) {
            const double now = scale();
            if (now != scale_) {
                scale_ = now;
                if (AAX_IViewContainer* container = GetViewContainer()) {
                    AAX_Point point{static_cast<float>(toPixels(height_, scale_)), static_cast<float>(toPixels(width_, scale_))};
                    container->SetViewSize(point);
                }
            }
        }
        return AAX_CEffectGUI::TimerWakeup();
    }

protected:
    void CreateViewContents() override {
        OpShellInfo info{};
        if (!op_shell_info(&info)) return;
        ids_.resize(info.param_count);
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            if (op_shell_param(i, &desc)) ids_[i] = desc.id;
        }
    }
    void CreateViewContainer() override {
        close();
        if (GetViewContainerType() != AAX_eViewContainer_Type_HWND) return;
        void* parent = GetViewContainerPtr();
        if (!parent || !IsWindow(static_cast<HWND>(parent))) return;
        OpEditorHost host{this, beginEdit, performEdit, endEdit};
        editor_ = op_editor_create(&host);
        if (!editor_) return;
        // The data model owns the engine for as long as this view exists.
        void* engine = nullptr;
        uint32_t written = 0;
        if (auto* parameters = GetEffectParameters()) {
            if (parameters->GetCustomData(kEngineData, sizeof(engine), &engine, &written) != AAX_SUCCESS || written != sizeof(engine))
                engine = nullptr;
        }
        op_editor_bind(editor_, engine);
        // The AAX host fixes the view at GetViewSize and never resizes it by
        // itself; the editor can only ask, through the view container.
        op_editor_get_size(editor_, &width_, &height_);
        op_editor_set_request_resize(editor_, this, requestResize);
        if (!op_editor_attach(editor_, parent)) {
            close();
            return;
        }
        // Attached, the editor's scale is the one of the host's window.
        scale_ = scale();
    }
    void DeleteViewContainer() override { close(); }
    AAX_Result GetViewSize(AAX_Point* size) const override {
        if (!size) return AAX_ERROR_NULL_OBJECT;
        uint32_t width = width_;
        uint32_t height = height_;
        double pixelsPerPoint = scale_;
        if (!editor_) {
            // No view yet: the size the editor opens at, at the system's scale.
            op_editor_size(&width, &height);
            const UINT dpi = GetDpiForSystem();
            pixelsPerPoint = dpi ? static_cast<double>(dpi) / 96.0 : 1.0;
        }
        size->horz = static_cast<float>(toPixels(width, pixelsPerPoint));
        size->vert = static_cast<float>(toPixels(height, pixelsPerPoint));
        return AAX_SUCCESS;
    }
    AAX_Result ParameterUpdated(AAX_CParamID id) override {
        if (!editor_ || !id) return AAX_SUCCESS;
        for (uint32_t i = 0; i < ids_.size(); ++i) {
            if (ids_[i] != id) continue;
            float value = 0.f;
            if (AAX_IParameter* parameter = find(i); parameter && parameter->GetValueAsFloat(&value))
                op_editor_set_plain(editor_, i, value);
            break;
        }
        return AAX_SUCCESS;
    }

private:
    AAX_IParameter* find(uint32_t index) {
        AAX_IParameter* parameter = nullptr;
        auto* parameters = GetEffectParameters();
        if (!parameters || index >= ids_.size()) return nullptr;
        return parameters->GetParameter(ids_[index].c_str(), &parameter) == AAX_SUCCESS ? parameter : nullptr;
    }
    double scale() const {
        const double value = editor_ ? op_editor_scale(editor_) : 0.0;
        return value > 0.0 ? value : 1.0;
    }
    static void beginEdit(void* context, uint32_t index) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->Touch();
    }
    static void performEdit(void* context, uint32_t index, double plain) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->SetValueWithFloat(static_cast<float>(plain));
    }
    static void endEdit(void* context, uint32_t index) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->Release();
    }
    // The editor asks for a size in points: the host decides through SetViewSize
    // (pixels), then the editor takes it. The size GetViewSize reports changes
    // first, so a host that asks for it inside SetViewSize gets the new one.
    static uint32_t requestResize(void* context, uint32_t width, uint32_t height) {
        auto* self = static_cast<Gui*>(context);
        AAX_IViewContainer* container = self->GetViewContainer();
        if (!self->editor_ || !container) return 0;
        const uint32_t oldWidth = self->width_;
        const uint32_t oldHeight = self->height_;
        self->width_ = width;
        self->height_ = height;
        AAX_Point point{static_cast<float>(toPixels(height, self->scale_)), static_cast<float>(toPixels(width, self->scale_))};
        if (container->SetViewSize(point) != AAX_SUCCESS) {
            self->width_ = oldWidth;
            self->height_ = oldHeight;
            return 0;
        }
        op_editor_resize(self->editor_, width, height);
        return 1;
    }
    void close() {
        if (editor_) {
            op_editor_set_request_resize(editor_, nullptr, nullptr);
            op_editor_destroy(editor_);
            editor_ = nullptr;
        }
    }
    void* editor_ = nullptr;
    uint32_t width_ = 0;   // points
    uint32_t height_ = 0;  // points
    double scale_ = 1.0;   // pixels per point
    std::vector<std::string> ids_;
};
AAX_IEffectGUI* AAX_CALLBACK createGUI() { return Gui::create(); }
}
