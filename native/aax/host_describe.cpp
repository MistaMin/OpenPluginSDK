// SPDX-License-Identifier: MIT
// Proves what the AAX shell describes for ABI 2: one component per stem pair with
// the right fields (MIDI input and output nodes only when the plugin has note
// ports, the transport node, the side-chain field when a layout has a
// sidechain), the stem formats and plug-in IDs, the category from the plugin
// kind, and the library version check. It runs GetEffectDescriptions against a
// recording AAX_ICollection over the stand-in ABI in proof_stub.cpp. It also
// delivers the host's offline notifications to the shell's data model. Not a
// Pro Tools test and not Avid's validator (scripts/aax_validate.sh).
//
//   ctest --test-dir build-shell -R aax_describe_proof --output-on-failure
#include "OpenPluginAAX.h"
#include "proof_stub.h"
#include "AAX_ICollection.h"
#include "AAX_IComponentDescriptor.h"
#include "AAX_IEffectDescriptor.h"
#include "AAX_CEffectParameters.h"
#include "AAX_IPropertyMap.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <string>
#include <vector>

static unsigned long g_checks = 0;
static void require(bool ok, const char* message) {
    ++g_checks;
    if (!ok) {
        std::fprintf(stderr, "AAX describe proof failed: %s\n", message);
        std::exit(1);
    }
}

// The proof has no editor; the shell's createGUI is not part of this binary.
namespace openplugin_aax {
AAX_IEffectGUI* AAX_CALLBACK createGUI() { return nullptr; }
}

struct Props final : AAX_IPropertyMap {
    std::map<int, int> values;
    AAX_CBoolean GetProperty(AAX_EProperty p, AAX_CPropertyValue* out) const override {
        const auto it = values.find(p);
        if (it == values.end()) return false;
        *out = it->second;
        return true;
    }
    AAX_CBoolean GetPointerProperty(AAX_EProperty, const void**) const override { return false; }
    AAX_Result AddProperty(AAX_EProperty p, AAX_CPropertyValue v) override { values[p] = v; return AAX_SUCCESS; }
    AAX_Result AddPointerProperty(AAX_EProperty, const void*) override { return AAX_SUCCESS; }
    AAX_Result AddPointerProperty(AAX_EProperty, const char*) override { return AAX_SUCCESS; }
    AAX_Result RemoveProperty(AAX_EProperty p) override { values.erase(p); return AAX_SUCCESS; }
    AAX_Result AddPropertyWithIDArray(AAX_EProperty, const AAX_SPlugInIdentifierTriad*, uint32_t) override { return AAX_SUCCESS; }
    AAX_CBoolean GetPropertyWithIDArray(AAX_EProperty, const AAX_SPlugInIdentifierTriad**, uint32_t*) const override { return false; }
    IACFUnknown* GetIUnknown() override { return nullptr; }
    bool has(int p) const { return values.count(p) != 0; }
    int get(int p) const { return values.at(p); }
};

struct MidiNode {
    AAX_CFieldIndex field;
    AAX_EMIDINodeType type;
    std::string name;
    uint32_t mask;
};
struct Component final : AAX_IComponentDescriptor {
    std::vector<AAX_CFieldIndex> audioIn, audioOut, length, sideChain;
    std::vector<std::pair<AAX_CFieldIndex, uint32_t>> dataIn, priv;
    std::vector<MidiNode> midi;
    std::vector<std::unique_ptr<Props>> maps;
    Props* processProps = nullptr;
    bool hasProc = false;
    AAX_Result Clear() override { return AAX_SUCCESS; }
    AAX_Result AddAudioIn(AAX_CFieldIndex f) override { audioIn.push_back(f); return AAX_SUCCESS; }
    AAX_Result AddAudioOut(AAX_CFieldIndex f) override { audioOut.push_back(f); return AAX_SUCCESS; }
    AAX_Result AddAudioBufferLength(AAX_CFieldIndex f) override { length.push_back(f); return AAX_SUCCESS; }
    AAX_Result AddSampleRate(AAX_CFieldIndex) override { return AAX_SUCCESS; }
    AAX_Result AddClock(AAX_CFieldIndex) override { return AAX_SUCCESS; }
    AAX_Result AddSideChainIn(AAX_CFieldIndex f) override { sideChain.push_back(f); return AAX_SUCCESS; }
    AAX_Result AddDataInPort(AAX_CFieldIndex f, uint32_t size, AAX_EDataInPortType) override { dataIn.push_back({f, size}); return AAX_SUCCESS; }
    AAX_Result AddAuxOutputStem(AAX_CFieldIndex, int32_t, const char[]) override { return AAX_SUCCESS; }
    AAX_Result AddPrivateData(AAX_CFieldIndex f, int32_t size, uint32_t) override { priv.push_back({f, static_cast<uint32_t>(size)}); return AAX_SUCCESS; }
    AAX_Result AddTemporaryData(AAX_CFieldIndex, uint32_t) override { return AAX_SUCCESS; }
    AAX_Result AddDmaInstance(AAX_CFieldIndex, AAX_IDma::EMode) override { return AAX_SUCCESS; }
    AAX_Result AddMeters(AAX_CFieldIndex, const AAX_CTypeID*, const uint32_t) override { return AAX_SUCCESS; }
    AAX_Result AddMIDINode(AAX_CFieldIndex f, AAX_EMIDINodeType t, const char name[], uint32_t mask) override {
        midi.push_back({f, t, name, mask});
        return AAX_SUCCESS;
    }
    AAX_Result AddReservedField(AAX_CFieldIndex, uint32_t) override { return AAX_SUCCESS; }
    AAX_IPropertyMap* NewPropertyMap() const override {
        auto* self = const_cast<Component*>(this);
        self->maps.push_back(std::make_unique<Props>());
        return self->maps.back().get();
    }
    AAX_IPropertyMap* DuplicatePropertyMap(AAX_IPropertyMap*) const override { return nullptr; }
    AAX_Result AddProcessProc_Native(AAX_CProcessProc, AAX_IPropertyMap* p, AAX_CInstanceInitProc, AAX_CBackgroundProc, AAX_CSelector*) override {
        hasProc = true;
        processProps = static_cast<Props*>(p);
        return AAX_SUCCESS;
    }
    AAX_Result AddProcessProc_TI(const char[], const char[], AAX_IPropertyMap*, const char[], const char[], AAX_CSelector*) override { return AAX_SUCCESS; }
    AAX_Result AddProcessProc(AAX_IPropertyMap*, AAX_CSelector*, int32_t) override { return AAX_SUCCESS; }
    const MidiNode* node(AAX_EMIDINodeType t) const {
        for (const auto& n : midi)
            if (n.type == t) return &n;
        return nullptr;
    }
};
struct Effect final : AAX_IEffectDescriptor {
    std::vector<std::unique_ptr<Component>> components, pending;
    std::vector<std::string> names;
    uint32_t categories = 0;
    std::vector<std::unique_ptr<Props>> maps;
    Props* properties = nullptr;
    bool createParameters = false, createGUI = false;
    AAX_IComponentDescriptor* NewComponentDescriptor() override {
        pending.push_back(std::make_unique<Component>());
        return pending.back().get();
    }
    AAX_Result AddComponent(AAX_IComponentDescriptor* c) override {
        for (auto it = pending.begin(); it != pending.end(); ++it)
            if (it->get() == c) {
                components.push_back(std::move(*it));
                pending.erase(it);
                return AAX_SUCCESS;
            }
        return AAX_ERROR_NULL_OBJECT;
    }
    AAX_Result AddName(const char* n) override { names.push_back(n); return AAX_SUCCESS; }
    AAX_Result AddCategory(uint32_t c) override { categories |= c; return AAX_SUCCESS; }
    AAX_Result AddCategoryBypassParameter(uint32_t, AAX_CParamID) override { return AAX_SUCCESS; }
    AAX_Result AddProcPtr(void*, AAX_CProcPtrID id) override {
        createParameters = createParameters || id == kAAX_ProcPtrID_Create_EffectParameters;
        createGUI = createGUI || id == kAAX_ProcPtrID_Create_EffectGUI;
        return AAX_SUCCESS;
    }
    AAX_IPropertyMap* NewPropertyMap() override {
        maps.push_back(std::make_unique<Props>());
        return maps.back().get();
    }
    AAX_Result SetProperties(AAX_IPropertyMap* p) override { properties = static_cast<Props*>(p); return AAX_SUCCESS; }
    AAX_Result AddResourceInfo(AAX_EResourceType, const char*) override { return AAX_SUCCESS; }
    AAX_Result AddMeterDescription(AAX_CTypeID, const char*, AAX_IPropertyMap*) override { return AAX_SUCCESS; }
    AAX_Result AddControlMIDINode(AAX_CTypeID, AAX_EMIDINodeType, const char[], uint32_t) override { return AAX_SUCCESS; }
};
struct Collection final : AAX_ICollection {
    std::vector<std::unique_ptr<Effect>> effects;
    std::vector<std::string> ids;
    std::vector<Effect*> added;
    std::string vendor;
    AAX_IEffectDescriptor* NewDescriptor() override {
        effects.push_back(std::make_unique<Effect>());
        return effects.back().get();
    }
    AAX_Result AddEffect(const char* id, AAX_IEffectDescriptor* e) override {
        ids.push_back(id);
        added.push_back(static_cast<Effect*>(e));
        return AAX_SUCCESS;
    }
    AAX_Result SetManufacturerName(const char* n) override { vendor = n; return AAX_SUCCESS; }
    AAX_Result AddPackageName(const char*) override { return AAX_SUCCESS; }
    AAX_Result SetPackageVersion(uint32_t) override { return AAX_SUCCESS; }
    AAX_IPropertyMap* NewPropertyMap() override { return nullptr; }
    AAX_Result SetProperties(AAX_IPropertyMap*) override { return AAX_SUCCESS; }
    AAX_Result GetHostVersion(uint32_t*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_IDescriptionHost* DescriptionHost() override { return nullptr; }
    const AAX_IDescriptionHost* DescriptionHost() const override { return nullptr; }
    IACFDefinition* HostDefinition() const override { return nullptr; }
};

AAX_Result GetEffectDescriptions(AAX_ICollection* collection);

static OpBus bus(uint32_t kind, std::vector<uint8_t> roles) {
    OpBus b{};
    b.kind = kind;
    b.channels = static_cast<uint32_t>(roles.size());
    std::memset(b.roles, OP_ROLE_NONE, sizeof b.roles);
    for (size_t i = 0; i < roles.size(); ++i) b.roles[i] = roles[i];
    return b;
}
static OpLayout layout(OpBus in, OpBus out, OpBus side = OpBus{}) {
    OpLayout l{};
    l.input = in;
    l.output = out;
    l.sidechain = side;
    std::strcpy(l.name, "proof");
    return l;
}
static void configure(uint32_t kind, uint32_t ports, std::vector<OpLayout> layouts) {
    op_stub_configure(kind, ports, static_cast<uint32_t>(layouts.size()), layouts.data());
}
static const Component* find(const Effect& e, AAX_EStemFormat in, AAX_EStemFormat out) {
    for (const auto& c : e.components)
        if (c->processProps && c->processProps->get(AAX_eProperty_InputStemFormat) == in && c->processProps->get(AAX_eProperty_OutputStemFormat) == out) return c.get();
    return nullptr;
}
// Every component has the fields the callback reads.
static void basics(const Effect& e, const OpShellInfo& info) {
    require(e.createParameters && e.createGUI && e.names.size() == 1 && e.names[0] == info.name, "data model, GUI and name");
    for (const auto& c : e.components) {
        require(c->hasProc && c->audioIn.size() == 1 && c->audioIn[0] == openplugin_aax::inputField, "audio in field");
        require(c->audioOut.size() == 1 && c->audioOut[0] == openplugin_aax::outputField, "audio out field");
        require(c->length.size() == 1 && c->length[0] == openplugin_aax::framesField, "buffer length field");
        require(c->dataIn.size() == 1 && c->dataIn[0].first == openplugin_aax::controlsField &&
                c->dataIn[0].second == openplugin_aax::packetBytes(info.param_count), "controls port sized by the parameter count");
        require(c->priv.size() == 1 && c->priv[0].first == openplugin_aax::stateField, "private state pointer");
        const MidiNode* transport = c->node(AAX_eMIDINodeType_Transport);
        require(transport && transport->field == openplugin_aax::transportField, "every component reads the transport");
        require(c->node(AAX_eMIDINodeType_Global) == nullptr, "no global MIDI node");
        require(c->processProps->get(AAX_eProperty_CanBypass) == 1 && c->processProps->get(AAX_eProperty_ManufacturerID) == static_cast<int>(info.manufacturer_id), "properties");
    }
    require(e.properties && e.properties->has(AAX_eProperty_UsesTransport) && e.properties->get(AAX_eProperty_UsesTransport) == 1, "the effect says it uses the transport");
}

static void describeOne(const char* what, AAX_Result want, Collection& c) {
    const AAX_Result got = GetEffectDescriptions(&c);
    if (got != want) {
        std::fprintf(stderr, "AAX describe proof failed: %s (result %d, wanted %d)\n", what, static_cast<int>(got), static_cast<int>(want));
        std::exit(1);
    }
    ++g_checks;
}

int main() {
    using namespace openplugin_aax;
    OpShellInfo info{};
    require(op_shell_info(&info), "info");
    const OpLayout stereo = layout(bus(OP_BUS_STEREO, {0, 1}), bus(OP_BUS_STEREO, {0, 1}));
    const OpBus mono = bus(OP_BUS_MONO, {OP_ROLE_NONE});
    const OpBus six = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5});

    {   // An effect that declares stereo only (the default): stereo, and the ABI 1 mono as before.
        configure(OP_KIND_EFFECT, 0, {stereo});
        Collection c;
        describeOne("default effect", AAX_SUCCESS, c);
        require(c.added.size() == 1 && c.ids[0] == info.id, "one effect with the plugin id");
        const Effect& e = *c.added[0];
        basics(e, info);
        require(e.components.size() == 2, "stereo and mono");
        require(e.categories == AAX_EPlugInCategory_Effect, "an effect with an unknown category is a plain effect");
        const Component* st = find(e, AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo);
        const Component* mo = find(e, AAX_eStemFormat_Mono, AAX_eStemFormat_Mono);
        require(st && mo, "both stems");
        require(st->processProps->get(AAX_eProperty_PlugInID_Native) == static_cast<int>(info.plugin_id_stereo) && mo->processProps->get(AAX_eProperty_PlugInID_Native) == static_cast<int>(info.plugin_id_mono), "the stereo and mono IDs");
        for (const auto& comp : e.components) {
            require(comp->node(AAX_eMIDINodeType_LocalInput) == nullptr && comp->node(AAX_eMIDINodeType_LocalOutput) == nullptr, "no MIDI nodes without note ports");
            require(comp->sideChain.empty(), "no sidechain");
        }
        require(!e.properties->has(AAX_eProperty_Constraint_MultiMonoSupport), "an audio effect stays multi-mono capable");
    }
    {   // Categories from the plugin's category string, for effects only.
        op_stub_set_category("Fx|EQ");
        configure(OP_KIND_EFFECT, 0, {stereo});
        Collection c;
        describeOne("eq", AAX_SUCCESS, c);
        require(c.added[0]->categories == AAX_ePlugInCategory_EQ, "EQ");
        op_stub_set_category("Dynamics|EQ");
        Collection d;
        describeOne("dynamics", AAX_SUCCESS, d);
        require(d.added[0]->categories == (AAX_ePlugInCategory_EQ | AAX_ePlugInCategory_Dynamics), "both");
        op_stub_set_category("Fx");
    }
    {   // An instrument: notes in and out, stereo in/out stems, software generator category.
        op_stub_set_category("Instrument|Synth");
        configure(OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, {layout(OpBus{}, bus(OP_BUS_STEREO, {0, 1}))});
        Collection c;
        describeOne("instrument", AAX_SUCCESS, c);
        const Effect& e = *c.added[0];
        basics(e, info);
        require(e.categories == AAX_ePlugInCategory_SWGenerators, "software generator");
        require(e.components.size() == 1 && find(e, AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo), "stereo only");
        const Component& comp = *e.components[0];
        const MidiNode* in = comp.node(AAX_eMIDINodeType_LocalInput);
        const MidiNode* out = comp.node(AAX_eMIDINodeType_LocalOutput);
        require(in && in->field == midiInField && in->mask == 0xFFFF, "local MIDI input, all channels");
        require(out && out->field == midiOutField && out->mask == 0xFFFF, "local MIDI output");
        require(in->name != out->name, "the nodes have different names");
        require(e.properties->has(AAX_eProperty_Constraint_MultiMonoSupport) && e.properties->get(AAX_eProperty_Constraint_MultiMonoSupport) == 0, "a MIDI instrument is not multi-mono");
        // Only an input port: no output node.
        configure(OP_KIND_INSTRUMENT, OP_NOTE_PORT_INPUT, {layout(OpBus{}, bus(OP_BUS_STEREO, {0, 1}))});
        Collection d;
        describeOne("instrument, notes in", AAX_SUCCESS, d);
        require(d.added[0]->components[0]->node(AAX_eMIDINodeType_LocalInput) && !d.added[0]->components[0]->node(AAX_eMIDINodeType_LocalOutput), "input node only");
        op_stub_set_category("Fx");
    }
    {   // A note effect: MIDI effect category, audio passes through, so mono and stereo stems.
        configure(OP_KIND_NOTE_EFFECT, OP_NOTE_PORT_INPUT | OP_NOTE_PORT_OUTPUT, {layout(OpBus{}, OpBus{})});
        Collection c;
        describeOne("note effect", AAX_SUCCESS, c);
        const Effect& e = *c.added[0];
        basics(e, info);
        require(e.categories == AAX_EPlugInCategory_MIDIEffect, "MIDI effect");
        require(e.components.size() == 2 && find(e, AAX_eStemFormat_Mono, AAX_eStemFormat_Mono) && find(e, AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo), "mono and stereo");
        require(e.components[0]->node(AAX_eMIDINodeType_LocalInput) && e.components[0]->node(AAX_eMIDINodeType_LocalOutput), "MIDI in and out");
    }
    {   // Stereo with a sidechain and 5.1, 7.1, 7.1.4.
        const OpBus b71 = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 9, 10, 4, 5});
        const OpBus b714 = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 9, 10, 4, 5, 12, 14, 15, 17});
        configure(OP_KIND_EFFECT, 0, {layout(stereo.input, stereo.output, mono), layout(six, six), layout(b71, b71), layout(b714, b714)});
        Collection c;
        describeOne("surround", AAX_SUCCESS, c);
        const Effect& e = *c.added[0];
        basics(e, info);
        require(e.components.size() == 5, "stereo, 5.1, 7.1, 7.1.4 and the ABI 1 mono");
        const Component* st = find(e, AAX_eStemFormat_Stereo, AAX_eStemFormat_Stereo);
        require(st && st->sideChain.size() == 1 && st->sideChain[0] == sideChainField && st->processProps->has(AAX_eProperty_SupportsSideChainInput), "the stereo component has the side-chain field");
        const Component* f1 = find(e, AAX_eStemFormat_5_1, AAX_eStemFormat_5_1);
        const Component* s71 = find(e, AAX_eStemFormat_7_1_DTS, AAX_eStemFormat_7_1_DTS);
        const Component* s714 = find(e, AAX_eStemFormat_7_1_4, AAX_eStemFormat_7_1_4);
        require(f1 && s71 && s714, "5.1, 7.1 and 7.1.4 stems");
        require(f1->sideChain.empty() && s714->sideChain.empty(), "no sidechain where the layout has none");
        std::map<int, int> ids;
        for (const auto& comp : e.components) ids[comp->processProps->get(AAX_eProperty_PlugInID_Native)]++;
        require(ids.size() == 5, "five different plug-in IDs");
        require(find(e, AAX_eStemFormat_Mono, AAX_eStemFormat_Mono)->sideChain.empty(), "the ABI 1 mono has none");
        // The IDs are the same on a second scan, as the host caches them.
        Collection d;
        describeOne("surround again", AAX_SUCCESS, d);
        for (size_t i = 0; i < e.components.size(); ++i)
            require(d.added[0]->components[i]->processProps->get(AAX_eProperty_PlugInID_Native) == e.components[i]->processProps->get(AAX_eProperty_PlugInID_Native), "stable IDs");
    }
    {   // The Atmos beds: 5.1.2, 7.1.2 and 9.1.6 are described, a bus of unmapped roles is not.
        const OpBus b512 = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5, 18, 19});
        const OpBus b712 = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 9, 10, 4, 5, 18, 19});
        const OpBus b916 = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 9, 10, 4, 5, 20, 21, 12, 14, 18, 19, 15, 17});
        const OpBus odd = bus(OP_BUS_SURROUND, {0, 1, 2, 3, 4, 5, 6, 7});
        configure(OP_KIND_EFFECT, 0, {layout(b512, b512), layout(b712, b712), layout(b916, b916), layout(odd, odd)});
        Collection c;
        describeOne("atmos", AAX_SUCCESS, c);
        const Effect& e = *c.added[0];
        require(find(e, AAX_eStemFormat_5_1_2, AAX_eStemFormat_5_1_2) && find(e, AAX_eStemFormat_7_1_2, AAX_eStemFormat_7_1_2) &&
                find(e, AAX_eStemFormat_9_1_6, AAX_eStemFormat_9_1_6), "5.1.2, 7.1.2 and 9.1.6 stems");
        require(e.components.size() == 3, "the bus with unmapped roles is not offered");
    }
    {   // A plugin none of whose layouts has an AAX stem is not described.
        configure(OP_KIND_EFFECT, 0, {layout(bus(OP_BUS_DISCRETE, {OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE}), bus(OP_BUS_DISCRETE, {OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE, OP_ROLE_NONE}))});
        Collection c;
        describeOne("no stems", AAX_ERROR_INVALID_ARGUMENT, c);
        require(c.added.empty(), "nothing added");
    }
    {   // The library must be at least as new as the shell.
        configure(OP_KIND_EFFECT, 0, {stereo});
        op_stub_set_abi(OP_SHELL_ABI - 1);
        Collection old;
        describeOne("older library", AAX_ERROR_INVALID_ARGUMENT, old);
        require(old.added.empty(), "an older library describes nothing");
        op_stub_set_abi(OP_SHELL_ABI + 1);
        Collection newer;
        describeOne("newer library", AAX_SUCCESS, newer);
        require(newer.added.size() == 1, "a newer library is described");
        op_stub_set_abi(OP_SHELL_ABI);
    }
    {   // Offline: the host's bounce notifications reach the engine through the data model.
        configure(OP_KIND_EFFECT, 0, {stereo});
        std::unique_ptr<AAX_CEffectParameters> parameters(createParameters());
        require(proofInit(parameters.get(), 48000.0, 2), "data model without a controller");
        void* engine = proofEngine(parameters.get());
        require(engine != nullptr, "engine");
        StubSeen seen;
        op_stub_seen(engine, &seen);
        require(seen.offline == 0, "not offline");
        require(proofNotify(parameters.get(), AAX_eNotificationEvent_EnteringOfflineMode), "entering");
        op_stub_seen(engine, &seen);
        require(seen.offline == 1, "offline reaches the plugin");
        require(proofNotify(parameters.get(), AAX_eNotificationEvent_ExitingOfflineMode), "exiting");
        op_stub_seen(engine, &seen);
        require(seen.offline == 0, "and back");
        require(proofNotify(parameters.get(), AAX_eNotificationEvent_SessionBeingOpened), "an unrelated notification is accepted");
        // Told before the engine exists: the engine starts offline.
        std::unique_ptr<AAX_CEffectParameters> early(createParameters());
        require(proofNotify(early.get(), AAX_eNotificationEvent_EnteringOfflineMode) && proofEngine(early.get()) == nullptr, "notification before init");
        require(proofInit(early.get(), 48000.0, 2), "init");
        op_stub_seen(proofEngine(early.get()), &seen);
        require(seen.offline == 1, "an engine built while the host is offline starts offline");
    }
    std::printf("AAX describe proof: %lu checks. Components per stem pair, MIDI input and output nodes only with note ports, transport node and UsesTransport, side-chain field, stems 5.1 / 7.1 / 7.1.4, category from the kind, IDs distinct and stable, library version check >=, offline notifications. Not Pro Tools or Avid's validator: PASS\n", g_checks);
    return 0;
}
