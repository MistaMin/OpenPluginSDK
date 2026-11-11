// SPDX-License-Identifier: MIT
// Development manufacturer and product IDs come from the plugin id hash.
// Replace them with registered Avid IDs before a signed Pro Tools release.
#include "OpenPluginAAX.h"
#include "AAX_ICollection.h"
#include "AAX_IComponentDescriptor.h"
#include "AAX_IEffectDescriptor.h"
#include "AAX_IPropertyMap.h"
#include "AAX_Errors.h"
#include <cstring>
namespace openplugin_aax {
namespace {
AAX_Result describe(AAX_IEffectDescriptor& effect, AAX_EStemFormat stem, int32_t pluginId, const OpShellInfo& info) {
    auto* component = effect.NewComponentDescriptor();
    if (!component) return AAX_ERROR_NULL_OBJECT;
    AAX_Result error = AAX_SUCCESS;
    const auto check = [&](AAX_Result result) {
        if (error == AAX_SUCCESS && result != AAX_SUCCESS) error = result;
    };
    check(component->AddAudioIn(inputField));
    check(component->AddAudioOut(outputField));
    check(component->AddAudioBufferLength(framesField));
    check(component->AddDataInPort(controlsField, sizeof(Packet)));
    check(component->AddPrivateData(stateField, sizeof(ProcessState*)));
    auto* properties = component->NewPropertyMap();
    if (!properties) return AAX_ERROR_NULL_OBJECT;
    check(properties->AddProperty(AAX_eProperty_ManufacturerID, static_cast<int32_t>(info.manufacturer_id)));
    check(properties->AddProperty(AAX_eProperty_ProductID, static_cast<int32_t>(info.product_id)));
    check(properties->AddProperty(AAX_eProperty_PlugInID_Native, pluginId));
    check(properties->AddProperty(AAX_eProperty_InputStemFormat, stem));
    check(properties->AddProperty(AAX_eProperty_OutputStemFormat, stem));
    check(properties->AddProperty(AAX_eProperty_CanBypass, true));
    check(properties->AddProperty(AAX_eProperty_UsesClientGUI, false));
    check(properties->AddProperty(AAX_eProperty_SampleRate,
        AAX_eSampleRateMask_44100 | AAX_eSampleRateMask_48000 | AAX_eSampleRateMask_88200 |
        AAX_eSampleRateMask_96000 | AAX_eSampleRateMask_192000));
    check(component->AddProcessProc_Native(process, properties));
    check(effect.AddComponent(component));
    return error;
}
}
}
AAX_Result GetEffectDescriptions(AAX_ICollection* collection) {
    if (!collection) return AAX_ERROR_NULL_OBJECT;
    OpShellInfo info{};
    if (!op_shell_info(&info) || info.abi != OP_SHELL_ABI || info.param_count == 0 || info.param_count > openplugin_aax::kMaxParams)
        return AAX_ERROR_INVALID_ARGUMENT;
    auto* effect = collection->NewDescriptor();
    if (!effect) return AAX_ERROR_NULL_OBJECT;
    AAX_Result error = AAX_SUCCESS;
    const auto check = [&](AAX_Result result) {
        if (error == AAX_SUCCESS && result != AAX_SUCCESS) error = result;
    };
    check(effect->AddName(info.name));
    if (std::strstr(info.category, "EQ")) check(effect->AddCategory(AAX_ePlugInCategory_EQ));
    if (std::strstr(info.category, "Dynamics")) check(effect->AddCategory(AAX_ePlugInCategory_Dynamics));
    if (std::strstr(info.category, "Instrument")) check(effect->AddCategory(AAX_ePlugInCategory_SWGenerators));
    if (error == AAX_SUCCESS && !std::strstr(info.category, "EQ") && !std::strstr(info.category, "Dynamics") && !std::strstr(info.category, "Instrument"))
        check(effect->AddCategory(AAX_EPlugInCategory_Effect));
    check(openplugin_aax::describe(*effect, AAX_eStemFormat_Mono, static_cast<int32_t>(info.plugin_id_mono), info));
    check(openplugin_aax::describe(*effect, AAX_eStemFormat_Stereo, static_cast<int32_t>(info.plugin_id_stereo), info));
    check(effect->AddProcPtr(reinterpret_cast<void*>(openplugin_aax::createParameters), kAAX_ProcPtrID_Create_EffectParameters));
    check(effect->AddProcPtr(reinterpret_cast<void*>(openplugin_aax::createGUI), kAAX_ProcPtrID_Create_EffectGUI));
    check(collection->AddEffect(info.id, effect));
    check(collection->SetManufacturerName(info.vendor));
    check(collection->AddPackageName(info.name));
    check(collection->SetPackageVersion(1));
    return error;
}
