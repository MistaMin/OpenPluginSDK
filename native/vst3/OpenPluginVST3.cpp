// SPDX-License-Identifier: MIT
// Thin VST3 adapter. The plugin is the Rust op_shell_* ABI. This file does not
// contain DSP. With OP_HAS_EDITOR the controller hosts the plugin's op_editor_* view;
// without it a host shows its generic controls.
#include "op_shell.h"
#if OP_HAS_EDITOR
#include "op_editor.h"
#include "public.sdk/source/common/pluginview.h"
#include "pluginterfaces/vst/ivstmessage.h"
#endif
#include "public.sdk/source/main/pluginfactory.h"
#include "public.sdk/source/vst/vstaudioeffect.h"
#include "public.sdk/source/vst/vsteditcontroller.h"
#include "public.sdk/source/vst/vstparameters.h"
#include "pluginterfaces/base/ustring.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "base/source/fstreamer.h"
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <memory>
#include <vector>

using namespace Steinberg;
using namespace Steinberg::Vst;

namespace openplugin_vst {
bool load_info(OpShellInfo& info) {
    std::memset(&info, 0, sizeof(info));
    return op_shell_info(&info) && info.abi == OP_SHELL_ABI && info.param_count > 0;
}

FUID fuid_from(const uint8_t bytes[16]) {
    TUID cid{};
    std::memcpy(cid, bytes, 16);
    return FUID::fromTUID(cid);
}

class ShellParameter final : public Parameter {
public:
    ShellParameter(const TChar* title, const TChar* units, ParamID tag, ParamValue normalized, int32 stepCount, int32 flags)
        : Parameter(title, tag, units, normalized, stepCount, flags), index_(tag) {}
    ParamValue toPlain(ParamValue normalized) const SMTG_OVERRIDE { return op_shell_to_plain(index_, normalized); }
    ParamValue toNormalized(ParamValue plain) const SMTG_OVERRIDE { return op_shell_to_normalized(index_, plain); }
    OBJ_METHODS(ShellParameter, Parameter)
private:
    ParamID index_;
};

#if OP_HAS_EDITOR
constexpr const char* kEngineMessage = "openplugin.engine";
#endif

class Processor final : public AudioEffect {
public:
    Processor() {
        OpShellInfo info{};
        if (!load_info(info)) return;
        setControllerClass(fuid_from(info.controller_cid));
        count_ = info.param_count;
        values_ = std::make_unique<std::atomic<double>[]>(count_);
        for (uint32_t i = 0; i < count_; ++i) {
            OpParamDesc desc{};
            if (op_shell_param(i, &desc)) values_[i].store(desc.default_normalized);
            else values_[i].store(0.0);
        }
    }
    ~Processor() override { destroy(); }
    static FUnknown* create(void*) { return static_cast<IAudioProcessor*>(new Processor); }

    tresult PLUGIN_API initialize(FUnknown* context) SMTG_OVERRIDE {
        if (count_ == 0) return kResultFalse;
        tresult result = AudioEffect::initialize(context);
        if (result != kResultOk) return result;
        addAudioInput(STR16("Input"), SpeakerArr::kStereo);
        addAudioOutput(STR16("Output"), SpeakerArr::kStereo);
        return kResultOk;
    }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement* inputs, int32 inputCount, SpeakerArrangement* outputs, int32 outputCount) SMTG_OVERRIDE {
        if (!inputs || !outputs || inputCount != 1 || outputCount != 1 || inputs[0] != outputs[0]) return kResultFalse;
        if (inputs[0] != SpeakerArr::kMono && inputs[0] != SpeakerArr::kStereo) return kResultFalse;
        channels_ = inputs[0] == SpeakerArr::kMono ? 1u : 2u;
        return AudioEffect::setBusArrangements(inputs, inputCount, outputs, outputCount);
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) SMTG_OVERRIDE { return size == kSample32 ? kResultTrue : kResultFalse; }
    tresult PLUGIN_API setupProcessing(ProcessSetup& setup) SMTG_OVERRIDE {
        if (setup.symbolicSampleSize != kSample32 || setup.maxSamplesPerBlock < 1 || setup.maxSamplesPerBlock > 65536) return kInvalidArgument;
        if (!std::isfinite(setup.sampleRate) || setup.sampleRate < 8000.0 || setup.sampleRate > 768000.0) return kInvalidArgument;
        return AudioEffect::setupProcessing(setup);
    }
    tresult PLUGIN_API setActive(TBool active) SMTG_OVERRIDE {
        if (!active) {
            announce(nullptr);
            destroy();
            return AudioEffect::setActive(active);
        }
        if (processSetup.maxSamplesPerBlock < 1) return kResultFalse;
        void* next = op_shell_create(processSetup.sampleRate, static_cast<uint32_t>(processSetup.maxSamplesPerBlock), channels_);
        if (!next) return kResultFalse;
        destroy();
        engine_ = next;
        op_shell_reset(engine_);
        announce(engine_);
        return AudioEffect::setActive(active);
    }
    tresult PLUGIN_API setProcessing(TBool active) SMTG_OVERRIDE {
        if (active && engine_) op_shell_reset(engine_);
        return kResultOk;
    }
    uint32 PLUGIN_API getLatencySamples() SMTG_OVERRIDE { return engine_ ? op_shell_latency(engine_) : 0; }
#if OP_HAS_TAIL
    uint32 PLUGIN_API getTailSamples() SMTG_OVERRIDE {
        if (!engine_) return kNoTail;
        const double samples = op_shell_tail_seconds(engine_) * processSetup.sampleRate;
        if (!(samples < 4.0e9)) return kInfiniteTail;
        return samples > 0.0 ? static_cast<uint32>(samples) : kNoTail;
    }
#endif
    tresult PLUGIN_API setState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        int32 version = 0;
        int32 count = 0;
        if (!reader.readInt32(version) || version != 1 || !reader.readInt32(count) || count != static_cast<int32>(count_)) return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) {
            double value = 0.0;
            if (!reader.readDouble(value) || !std::isfinite(value) || value < 0.0 || value > 1.0) return kResultFalse;
            values_[i].store(value);
        }
        return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        IBStreamer writer(stream, kLittleEndian);
        if (!writer.writeInt32(1) || !writer.writeInt32(static_cast<int32>(count_))) return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) {
            if (!writer.writeDouble(values_[i].load())) return kResultFalse;
        }
        return kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& data) SMTG_OVERRIDE {
        if (data.inputParameterChanges) {
            int32 queues = data.inputParameterChanges->getParameterCount();
            for (int32 i = 0; i < queues; ++i) {
                auto* queue = data.inputParameterChanges->getParameterData(i);
                if (!queue || queue->getPointCount() < 1) continue;
                ParamID id = queue->getParameterId();
                if (id >= count_) continue;
                int32 offset = 0;
                ParamValue value = 0.0;
                if (queue->getPoint(queue->getPointCount() - 1, offset, value) == kResultTrue && std::isfinite(value))
                    values_[id].store(std::clamp(value, 0.0, 1.0));
            }
        }
        if (data.numSamples == 0) return kResultOk;
        if (!engine_ || data.symbolicSampleSize != kSample32 || data.numSamples < 0 || data.numInputs != 1 || data.numOutputs != 1 || !data.inputs || !data.outputs)
            return kResultFalse;
        auto& input = data.inputs[0];
        auto& output = data.outputs[0];
        int32 channels = input.numChannels;
        if ((channels != 1 && channels != 2) || output.numChannels != channels || !input.channelBuffers32 || !output.channelBuffers32)
            return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) {
            if (!op_shell_set_normalized(engine_, i, static_cast<float>(values_[i].load()))) return kResultFalse;
        }
        const int32 limit = processSetup.maxSamplesPerBlock > 0 ? processSetup.maxSamplesPerBlock : data.numSamples;
        for (int32 offset = 0; offset < data.numSamples;) {
            uint32_t frames = static_cast<uint32_t>(std::min(limit, data.numSamples - offset));
            const float* inLeft = input.channelBuffers32[0] + offset;
            const float* inRight = channels > 1 ? input.channelBuffers32[1] + offset : nullptr;
            float* outLeft = output.channelBuffers32[0] + offset;
            float* outRight = channels > 1 ? output.channelBuffers32[1] + offset : nullptr;
            if (!op_shell_process(engine_, inLeft, inRight, outLeft, outRight, frames)) return kResultFalse;
            offset += static_cast<int32>(frames);
        }
        output.silenceFlags = 0;
        return kResultOk;
    }

private:
    // Tells the controller which engine an open editor should show. The pointer
    // only means something in this process, and the editor checks it is live.
    void announce(void* engine) {
#if OP_HAS_EDITOR
        if (IPtr<IMessage> message = owned(allocateMessage())) {
            message->setMessageID(kEngineMessage);
            message->getAttributes()->setInt("engine", static_cast<int64>(reinterpret_cast<intptr_t>(engine)));
            sendMessage(message);
        }
#else
        (void)engine;
#endif
    }
    void destroy() {
        if (engine_) {
            op_shell_destroy(engine_);
            engine_ = nullptr;
        }
    }
    void* engine_ = nullptr;
    uint32_t count_ = 0;
    uint32_t channels_ = 2;
    std::unique_ptr<std::atomic<double>[]> values_;
};

class Controller final : public EditController {
public:
    static FUnknown* create(void*) { return static_cast<IEditController*>(new Controller); }
    tresult PLUGIN_API initialize(FUnknown* context) SMTG_OVERRIDE {
        tresult result = EditController::initialize(context);
        if (result != kResultOk) return result;
        OpShellInfo info{};
        if (!load_info(info)) return kResultFalse;
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            if (!op_shell_param(i, &desc)) return kResultFalse;
            String128 title{};
            String128 unit{};
            UString(title, 128).fromAscii(desc.name);
            UString(unit, 128).fromAscii(desc.unit);
            int32 steps = desc.steps > 1 ? static_cast<int32>(desc.steps - 1) : 0;
            int32 flags = (desc.flags & OP_PARAM_AUTOMATABLE) ? ParameterInfo::kCanAutomate : 0;
            parameters.addParameter(new ShellParameter(title, unit, i, desc.default_normalized, steps, flags));
        }
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        int32 version = 0;
        int32 count = 0;
        if (!reader.readInt32(version) || version != 1 || !reader.readInt32(count) || count != parameters.getParameterCount()) return kResultFalse;
        for (int32 i = 0; i < count; ++i) {
            double value = 0.0;
            if (!reader.readDouble(value) || !std::isfinite(value)) return kResultFalse;
            setParamNormalized(static_cast<ParamID>(i), value);
        }
        return kResultOk;
    }
#if OP_HAS_EDITOR
    IPlugView* PLUGIN_API createView(FIDString name) SMTG_OVERRIDE;
    tresult PLUGIN_API notify(IMessage* message) SMTG_OVERRIDE {
        if (message && message->getMessageID() && std::strcmp(message->getMessageID(), kEngineMessage) == 0) {
            int64 value = 0;
            if (message->getAttributes()->getInt("engine", value) == kResultOk) {
                engine_ = reinterpret_cast<void*>(static_cast<intptr_t>(value));
                if (editor_) op_editor_bind(editor_, engine_);
            }
            return kResultOk;
        }
        return EditController::notify(message);
    }
    tresult PLUGIN_API setParamNormalized(ParamID tag, ParamValue value) SMTG_OVERRIDE {
        const tresult result = EditController::setParamNormalized(tag, value);
        if (editor_ && result == kResultOk) op_editor_set_plain(editor_, tag, static_cast<float>(op_shell_to_plain(tag, value)));
        return result;
    }
    // Called by the view.
    void* openEditor(void* parent) {
        closeEditor();
        OpEditorHost host{this, hostBegin, hostPerform, hostEnd};
        editor_ = op_editor_create(&host);
        if (!editor_) return nullptr;
        for (int32 i = 0; i < parameters.getParameterCount(); ++i) {
            const ParamID tag = static_cast<ParamID>(i);
            op_editor_set_plain(editor_, tag, static_cast<float>(op_shell_to_plain(tag, getParamNormalized(tag))));
        }
        op_editor_bind(editor_, engine_);
        if (!op_editor_attach(editor_, parent)) closeEditor();
        return editor_;
    }
    void closeEditor() {
        if (editor_) {
            op_editor_destroy(editor_);
            editor_ = nullptr;
        }
    }

private:
    static void hostBegin(void* context, uint32_t index) { static_cast<Controller*>(context)->EditController::beginEdit(index); }
    static void hostPerform(void* context, uint32_t index, double plain) {
        auto* self = static_cast<Controller*>(context);
        const ParamValue normalized = op_shell_to_normalized(index, plain);
        self->EditController::setParamNormalized(index, normalized);
        self->EditController::performEdit(index, normalized);
    }
    static void hostEnd(void* context, uint32_t index) { static_cast<Controller*>(context)->EditController::endEdit(index); }
    void* editor_ = nullptr;
    void* engine_ = nullptr;
#else
    IPlugView* PLUGIN_API createView(FIDString) SMTG_OVERRIDE { return nullptr; }
#endif
};

#if OP_HAS_EDITOR
// The NSView host. The controller owns the editor, so a view that outlives its
// window still closes cleanly.
class EditorView final : public CPluginView {
public:
    explicit EditorView(Controller* controller) : controller_(controller) {
        uint32_t width = 0;
        uint32_t height = 0;
        op_editor_size(&width, &height);
        rect = ViewRect(0, 0, static_cast<int32>(width), static_cast<int32>(height));
    }
    ~EditorView() override {
        if (open_ && controller_) controller_->closeEditor();
    }
    tresult PLUGIN_API isPlatformTypeSupported(FIDString type) SMTG_OVERRIDE {
        return type && std::strcmp(type, kPlatformTypeNSView) == 0 ? kResultTrue : kResultFalse;
    }
    tresult PLUGIN_API attached(void* parent, FIDString type) SMTG_OVERRIDE {
        if (!parent || isPlatformTypeSupported(type) != kResultTrue || !controller_) return kResultFalse;
        if (!controller_->openEditor(parent)) return kResultFalse;
        open_ = true;
        return CPluginView::attached(parent, type);
    }
    tresult PLUGIN_API removed() SMTG_OVERRIDE {
        if (open_ && controller_) controller_->closeEditor();
        open_ = false;
        return CPluginView::removed();
    }
    tresult PLUGIN_API onSize(ViewRect*) SMTG_OVERRIDE { return kResultOk; }
    tresult PLUGIN_API checkSizeConstraint(ViewRect* size) SMTG_OVERRIDE {
        if (!size) return kInvalidArgument;
        size->right = size->left + rect.getWidth();
        size->bottom = size->top + rect.getHeight();
        return kResultOk;
    }

private:
    IPtr<Controller> controller_;
    bool open_ = false;
};

IPlugView* PLUGIN_API Controller::createView(FIDString name) {
    if (!name || std::strcmp(name, ViewType::kEditor) != 0) return nullptr;
    return new EditorView(this);
}
#endif
}

bool InitModule() { return true; }
bool DeinitModule() { return true; }

extern "C" SMTG_EXPORT_SYMBOL IPluginFactory* GetPluginFactory() {
    if (!gPluginFactory) {
        OpShellInfo info{};
        if (!openplugin_vst::load_info(info)) return nullptr;
        PFactoryInfo factoryInfo(info.vendor, "", "", PFactoryInfo::kNoFlags);
        gPluginFactory = new CPluginFactory(factoryInfo);
        TUID processor{};
        TUID controller{};
        std::memcpy(processor, info.processor_cid, 16);
        std::memcpy(controller, info.controller_cid, 16);
        PClassInfo2 processorClass(processor, PClassInfo::kManyInstances, kVstAudioEffectClass, info.name, kDistributable, info.category, info.vendor, info.version, kVstVersionString);
        PClassInfo2 controllerClass(controller, PClassInfo::kManyInstances, kVstComponentControllerClass, info.name, 0, "", info.vendor, info.version, kVstVersionString);
        gPluginFactory->registerClass(&processorClass, openplugin_vst::Processor::create);
        gPluginFactory->registerClass(&controllerClass, openplugin_vst::Controller::create);
    } else {
        gPluginFactory->addRef();
    }
    return gPluginFactory;
}
