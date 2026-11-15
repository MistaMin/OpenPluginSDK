// SPDX-License-Identifier: MIT
// With OP_HAS_EDITOR the view is the plugin's op_editor_* editor. Without it
// the view is an empty rectangle.
#include "OpenPluginAAX.h"
#include "AAX_CEffectGUI.h"
#if OP_HAS_EDITOR
#include "AAX_IEffectParameters.h"
#include "AAX_IParameter.h"
#include "AAX_IViewContainer.h"
#include "op_editor.h"
#include <string>
#include <vector>
#endif
namespace openplugin_aax {
#if OP_HAS_EDITOR
class Gui final : public AAX_CEffectGUI {
public:
    static AAX_IEffectGUI* AAX_CALLBACK create() { return new Gui; }
    ~Gui() override { close(); }

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
        if (GetViewContainerType() != AAX_eViewContainer_Type_NSView) return;
        void* parent = GetViewContainerPtr();
        if (!parent) return;
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
        if (!op_editor_attach(editor_, parent)) close();
    }
    void DeleteViewContainer() override { close(); }
    AAX_Result GetViewSize(AAX_Point* size) const override {
        if (!size) return AAX_ERROR_NULL_OBJECT;
        uint32_t width = width_;
        uint32_t height = height_;
        if (!editor_) op_editor_size(&width, &height);
        size->horz = static_cast<float>(width);
        size->vert = static_cast<float>(height);
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
    static void beginEdit(void* context, uint32_t index) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->Touch();
    }
    static void performEdit(void* context, uint32_t index, double plain) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->SetValueWithFloat(static_cast<float>(plain));
    }
    static void endEdit(void* context, uint32_t index) {
        if (AAX_IParameter* parameter = static_cast<Gui*>(context)->find(index)) parameter->Release();
    }
    // The editor asks for a size: the host decides through SetViewSize, then the
    // editor takes it. The size GetViewSize reports changes first, so a host that
    // asks for it inside SetViewSize gets the new one.
    static uint32_t requestResize(void* context, uint32_t width, uint32_t height) {
        auto* self = static_cast<Gui*>(context);
        AAX_IViewContainer* container = self->GetViewContainer();
        if (!self->editor_ || !container) return 0;
        const uint32_t oldWidth = self->width_;
        const uint32_t oldHeight = self->height_;
        self->width_ = width;
        self->height_ = height;
        AAX_Point point{static_cast<float>(height), static_cast<float>(width)};
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
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    std::vector<std::string> ids_;
};
#else
class Gui final : public AAX_CEffectGUI {
public:
    static AAX_IEffectGUI* AAX_CALLBACK create() { return new Gui; }
protected:
    void CreateViewContents() override {}
    void CreateViewContainer() override {}
    void DeleteViewContainer() override {}
    AAX_Result GetViewSize(AAX_Point* size) const override {
        if (!size) return AAX_ERROR_NULL_OBJECT;
        size->horz = 100.f;
        size->vert = 40.f;
        return AAX_SUCCESS;
    }
};
#endif
AAX_IEffectGUI* AAX_CALLBACK createGUI() { return Gui::create(); }
}
