// SPDX-License-Identifier: MIT
// Development manufacturer and product IDs come from the plugin id hash.
// Replace them with registered Avid IDs before a signed Pro Tools release.
//
// One component per stem pair, from op_shell_layout* (OpenPluginAAXProcess.cpp,
// buildConfigs): mono, stereo, 5.1, 7.1, 7.1.4 and so on, each with its own
// plug-in ID. The plugin category follows op_shell_kind().
#include "OpenPluginAAX.h"
#include "AAX_ICollection.h"
#include "AAX_IComponentDescriptor.h"
#include "AAX_IEffectDescriptor.h"
#include "AAX_IPropertyMap.h"
#include "AAX_Errors.h"
#include <cstring>
#include <string>
namespace openplugin_aax {
namespace {
constexpr uint32_t kAllMidiChannels = 0xFFFF;
AAX_Result describe(AAX_IEffectDescriptor& effect, const Config& config, uint32_t notePorts, const OpShellInfo& info) {
    auto* component = effect.NewComponentDescriptor();
    if (!component) return AAX_ERROR_NULL_OBJECT;
    AAX_Result error = AAX_SUCCESS;
    const auto check = [&](AAX_Result result) {
        if (error == AAX_SUCCESS && result != AAX_SUCCESS) error = result;
    };
    check(component->AddAudioIn(inputField));
    check(component->AddAudioOut(outputField));
    check(component->AddAudioBufferLength(framesField));
    check(component->AddDataInPort(controlsField, static_cast<uint32_t>(packetBytes(info.param_count))));
    check(component->AddPrivateData(stateField, sizeof(ProcessState*)));
    // MIDI in and out only when the plugin takes or emits notes. The transport node is not a MIDI
    // connection: the host routes nothing to it, and AAX_ITransport is reached through it.
    const std::string in = std::string(info.name) + " In";
    const std::string out = std::string(info.name) + " Out";
    if (notePorts & OP_NOTE_PORT_INPUT) check(component->AddMIDINode(midiInField, AAX_eMIDINodeType_LocalInput, in.c_str(), kAllMidiChannels));
    if (notePorts & OP_NOTE_PORT_OUTPUT) check(component->AddMIDINode(midiOutField, AAX_eMIDINodeType_LocalOutput, out.c_str(), kAllMidiChannels));
    check(component->AddMIDINode(transportField, AAX_eMIDINodeType_Transport, "Transport", kAllMidiChannels));
    // The side-chain field gives the index of the extra (mono) input channel; 0 when none is connected.
    if (config.sidechain) check(component->AddSideChainIn(sideChainField));
    auto* properties = component->NewPropertyMap();
    if (!properties) return AAX_ERROR_NULL_OBJECT;
    check(properties->AddProperty(AAX_eProperty_ManufacturerID, static_cast<int32_t>(info.manufacturer_id)));
    check(properties->AddProperty(AAX_eProperty_ProductID, static_cast<int32_t>(info.product_id)));
    check(properties->AddProperty(AAX_eProperty_PlugInID_Native, static_cast<int32_t>(config.pluginId)));
    check(properties->AddProperty(AAX_eProperty_InputStemFormat, config.input));
    check(properties->AddProperty(AAX_eProperty_OutputStemFormat, config.output));
    if (config.sidechain) check(properties->AddProperty(AAX_eProperty_SupportsSideChainInput, true));
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
    // The library must be at least as new as this shell (ABI 2); a newer one is fine.
    if (!openplugin_aax::abiOk() || !op_shell_info(&info) || info.param_count == 0 || info.param_count > openplugin_aax::kMaxParams)
        return AAX_ERROR_INVALID_ARGUMENT;
    const auto configs = openplugin_aax::buildConfigs(info);
    if (configs.empty()) return AAX_ERROR_INVALID_ARGUMENT;
    const uint32_t kind = op_shell_kind();
    const uint32_t notePorts = op_shell_note_ports();
    auto* effect = collection->NewDescriptor();
    if (!effect) return AAX_ERROR_NULL_OBJECT;
    AAX_Result error = AAX_SUCCESS;
    const auto check = [&](AAX_Result result) {
        if (error == AAX_SUCCESS && result != AAX_SUCCESS) error = result;
    };
    check(effect->AddName(info.name));
    // The category comes from the kind: an instrument is a software generator, a note effect a MIDI effect.
    // Only an audio effect uses the plugin's category string.
    if (kind == OP_KIND_INSTRUMENT) {
        check(effect->AddCategory(AAX_ePlugInCategory_SWGenerators));
    } else if (kind == OP_KIND_NOTE_EFFECT) {
        check(effect->AddCategory(AAX_EPlugInCategory_MIDIEffect));
    } else {
        const bool eq = std::strstr(info.category, "EQ") != nullptr;
        const bool dynamics = std::strstr(info.category, "Dynamics") != nullptr;
        if (eq) check(effect->AddCategory(AAX_ePlugInCategory_EQ));
        if (dynamics) check(effect->AddCategory(AAX_ePlugInCategory_Dynamics));
        if (!eq && !dynamics) check(effect->AddCategory(AAX_EPlugInCategory_Effect));
    }
    for (const auto& config : configs) check(openplugin_aax::describe(*effect, config, notePorts, info));
    // Effect-level properties: the plugin reads AAX_ITransport (through the transport node), and a plugin
    // fed by MIDI is not offered as multi-mono (its MIDI would reach only the first instance).
    auto* properties = effect->NewPropertyMap();
    if (!properties) return AAX_ERROR_NULL_OBJECT;
    check(properties->AddProperty(AAX_eProperty_UsesTransport, true));
    if (notePorts & OP_NOTE_PORT_INPUT) check(properties->AddProperty(AAX_eProperty_Constraint_MultiMonoSupport, false));
    check(effect->SetProperties(properties));
    check(effect->AddProcPtr(reinterpret_cast<void*>(openplugin_aax::createParameters), kAAX_ProcPtrID_Create_EffectParameters));
    check(effect->AddProcPtr(reinterpret_cast<void*>(openplugin_aax::createGUI), kAAX_ProcPtrID_Create_EffectGUI));
#ifdef OP_AAX_PAGE_TABLE
    check(effect->AddResourceInfo(AAX_eResourceType_PageTable, OP_AAX_PAGE_TABLE));
#endif
    check(collection->AddEffect(info.id, effect));
    check(collection->SetManufacturerName(info.vendor));
    check(collection->AddPackageName(info.name));
    check(collection->SetPackageVersion(1));
    return error;
}
