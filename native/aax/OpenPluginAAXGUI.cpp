// SPDX-License-Identifier: MIT
// Empty view. The audio shell does not embed the GoodLookin editor.
#include "OpenPluginAAX.h"
#include "AAX_CEffectGUI.h"
namespace openplugin_aax {
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
AAX_IEffectGUI* AAX_CALLBACK createGUI() { return Gui::create(); }
}
