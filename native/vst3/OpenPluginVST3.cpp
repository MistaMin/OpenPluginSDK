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
#include "pluginterfaces/base/ibstream.h"
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

// The host bypass parameter. Its ID is far outside 0..n-1 so the plugin's own
// parameter IDs are the ABI indices, unshifted.
constexpr ParamID kBypassId = 0x42595053;
// Length of the switch between processed and bypassed audio.
constexpr double kBypassFadeSeconds = 0.010;

#if OP_HAS_EDITOR
constexpr const char* kEngineMessage = "openplugin.engine";
#endif

// Component state. The head is unchanged since version 1: int32 version (1),
// int32 parameter count, one double per parameter, then an int32 bypass flag
// (absent in states written before bypass). After the flag a state may carry
// the plugin's whole state from op_shell_save_state as one trailer:
//   uint32 kStateTrailerTag, int32 trailer version (1), uint32 length, bytes.
// The trailer is last, so a shell that predates it reads the head and stops.
// A state without a trailer is a version 1 state and loads as before. A
// trailer with another tag or version, a length of 0 or above kMaxStateBlob,
// fewer bytes than its length, or bytes after it, is refused.
constexpr uint32 kStateTrailerTag = 0x4F505342;  // 'OPSB'
constexpr int32 kStateTrailerVersion = 1;
constexpr uint32 kMaxStateBlob = 1u << 30;

// Saves the whole state of `instance`: query the size, then copy, and repeat
// while the plugin says the state grew in between (OP_STATE_TOO_SMALL).
bool save_blob(void* instance, std::vector<uint8_t>& out) {
    uint32_t needed = 0;
    if (!instance || op_shell_save_state(instance, nullptr, 0, &needed) != OP_STATE_TOO_SMALL) return false;
    for (int attempt = 0; attempt < 64; ++attempt) {
        if (needed == 0 || needed > kMaxStateBlob) return false;
        out.assign(needed, 0);
        const uint32_t result = op_shell_save_state(instance, out.data(), needed, &needed);
        if (result == OP_STATE_OK) {
            if (needed == 0 || needed > out.size()) return false;
            out.resize(needed);
            return true;
        }
        if (result != OP_STATE_TOO_SMALL) return false;
    }
    return false;
}

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
            keepChunk();
            destroy();
            return AudioEffect::setActive(active);
        }
        if (processSetup.maxSamplesPerBlock < 1) return kResultFalse;
        void* next = op_shell_create(processSetup.sampleRate, static_cast<uint32_t>(processSetup.maxSamplesPerBlock), channels_);
        if (!next) return kResultFalse;
        keepChunk();
        destroy();
        engine_ = next;
        op_shell_reset(engine_);
        // The plugin's state set while inactive, or kept from the engine that
        // just went away, goes into the new engine.
        if (!pendingBlob_.empty() && op_shell_load_state(engine_, pendingBlob_.data(), static_cast<uint32_t>(pendingBlob_.size())))
            pushValues(engine_);
        pendingBlob_.clear();
        prepareBypass();
        announce(engine_);
        return AudioEffect::setActive(active);
    }
    tresult PLUGIN_API setProcessing(TBool active) SMTG_OVERRIDE {
        if (active && engine_) {
            op_shell_reset(engine_);
            clearBypassLine();
            // A fresh start has nothing to fade from.
            mix_ = bypass_.load() ? 1.0f : 0.0f;
        }
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
    // Loads the head and, if present, the plugin-state trailer (see above). It
    // changes nothing unless everything is accepted: op_shell_load_state
    // validates before it applies, and the parameter values and bypass change
    // only after that.
    tresult PLUGIN_API setState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        int32 version = 0;
        int32 count = 0;
        if (!reader.readInt32(version) || version != 1 || !reader.readInt32(count) || count != static_cast<int32>(count_)) return kResultFalse;
        std::vector<double> loaded(count_);
        for (uint32_t i = 0; i < count_; ++i) {
            if (!reader.readDouble(loaded[i]) || !std::isfinite(loaded[i]) || loaded[i] < 0.0 || loaded[i] > 1.0) return kResultFalse;
        }
        // Bypass follows the parameters. A state written before bypass existed
        // ends here, which means not bypassed.
        int32 bypass = 0;
        bool hasTrailer = false;
        std::vector<uint8_t> blob;
        if (reader.readInt32(bypass)) {
            if (!readTrailer(stream, hasTrailer, blob)) return kResultFalse;
        } else {
            bypass = 0;
        }
        if (hasTrailer) {
            if (engine_) {
                if (!op_shell_load_state(engine_, blob.data(), static_cast<uint32_t>(blob.size()))) return kResultFalse;
            } else {
                // No engine to load into: check the blob on a scratch instance
                // and keep it for the next setActive.
                if (!checkBlob(blob)) return kResultFalse;
                pendingBlob_ = std::move(blob);
            }
        }
        for (uint32_t i = 0; i < count_; ++i) values_[i].store(loaded[i]);
        bypass_.store(bypass != 0);
        if (hasTrailer && engine_) pushValues(engine_);
        return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream || count_ == 0) return kResultFalse;
        // The plugin's state first, so a failure writes nothing. The engine
        // gets the values held here before it is saved, so head and blob agree.
        std::vector<uint8_t> blob;
        if (engine_) {
            pushValues(engine_);
            if (!save_blob(engine_, blob)) return kResultFalse;
        } else {
            blob = pendingBlob_;
        }
        IBStreamer writer(stream, kLittleEndian);
        if (!writer.writeInt32(1) || !writer.writeInt32(static_cast<int32>(count_))) return kResultFalse;
        for (uint32_t i = 0; i < count_; ++i) {
            if (!writer.writeDouble(values_[i].load())) return kResultFalse;
        }
        if (!writer.writeInt32(bypass_.load() ? 1 : 0)) return kResultFalse;
        if (blob.empty()) return kResultOk;
        if (!writer.writeInt32(static_cast<int32>(kStateTrailerTag)) || !writer.writeInt32(kStateTrailerVersion) || !writer.writeInt32(static_cast<int32>(blob.size()))) return kResultFalse;
        int32 written = 0;
        if (stream->write(blob.data(), static_cast<int32>(blob.size()), &written) != kResultOk || written != static_cast<int32>(blob.size())) return kResultFalse;
        return kResultOk;
    }
    tresult PLUGIN_API process(ProcessData& data) SMTG_OVERRIDE {
        const int32 numSamples = std::max<int32>(data.numSamples, 0);
        const uint32_t pending = collectChanges(data.inputParameterChanges, numSamples);
        uint32_t next = 0;
        if (data.numSamples == 0) {
            applyDue(next, pending, 0, true);
            return kResultOk;
        }
        if (!engine_ || data.symbolicSampleSize != kSample32 || data.numSamples < 0 || data.numInputs != 1 || data.numOutputs != 1 || !data.inputs || !data.outputs)
            return kResultFalse;
        auto& input = data.inputs[0];
        auto& output = data.outputs[0];
        int32 channels = input.numChannels;
        if ((channels != 1 && channels != 2) || output.numChannels != channels || !input.channelBuffers32 || !output.channelBuffers32)
            return kResultFalse;
        // Start from the values held at the end of the previous block.
        for (uint32_t i = 0; i < count_; ++i) {
            if (!op_shell_set_normalized(engine_, i, static_cast<float>(values_[i].load()))) return kResultFalse;
        }
        const int32 limit = processSetup.maxSamplesPerBlock > 0 ? processSetup.maxSamplesPerBlock : data.numSamples;
        int32 offset = 0;
        while (offset < data.numSamples) {
            if (!applyDue(next, pending, offset, false)) return kResultFalse;
            // The segment runs to the next change point, the end of the block, or
            // the prepared block size, whichever comes first.
            int32 end = data.numSamples;
            if (next < pending) end = std::min(end, changes_[next].offset);
            const uint32_t frames = static_cast<uint32_t>(std::min(limit, end - offset));
            const float target = bypass_.load() ? 1.0f : 0.0f;
            const float* inLeft = input.channelBuffers32[0] + offset;
            const float* inRight = channels > 1 ? input.channelBuffers32[1] + offset : nullptr;
            float* outLeft = output.channelBuffers32[0] + offset;
            float* outRight = channels > 1 ? output.channelBuffers32[1] + offset : nullptr;
            // The delayed dry signal is taken before the engine runs, because the
            // host may have given the same buffer for input and output.
            captureDry(inLeft, inRight, channels, frames);
            if (!op_shell_process(engine_, inLeft, inRight, outLeft, outRight, frames)) return kResultFalse;
            applyBypass(outLeft, outRight, channels, frames, target);
            offset += static_cast<int32>(frames);
        }
        // Points at the very end of the block hold for the next one.
        if (!applyDue(next, pending, data.numSamples, true)) return kResultFalse;
        output.silenceFlags = 0;
        return kResultOk;
    }

private:
    // Reads the trailer that follows the bypass flag. `present` stays false when
    // the stream ends right there; a partial, foreign, empty, oversized or
    // extended trailer is an error.
    static bool readTrailer(IBStream* stream, bool& present, std::vector<uint8_t>& blob) {
        present = false;
        IBStreamer reader(stream, kLittleEndian);
        uint32_t tagBytes = 0;
        int32 got = 0;
        if (stream->read(&tagBytes, 4, &got) != kResultOk || got == 0) return true;
        if (got != 4) return false;
#if BYTEORDER == kBigEndian
        tagBytes = SWAP_32(tagBytes);
#endif
        int32 version = 0;
        int32 length = 0;
        if (tagBytes != kStateTrailerTag || !reader.readInt32(version) || version != kStateTrailerVersion || !reader.readInt32(length)) return false;
        if (length < 1 || static_cast<uint32_t>(length) > kMaxStateBlob) return false;
        // Read in pieces so a corrupt length cannot make a huge allocation
        // before the stream has run out.
        constexpr int32 kPiece = 1 << 16;
        blob.clear();
        int32 remaining = length;
        while (remaining > 0) {
            const int32 want = std::min(remaining, kPiece);
            const size_t at = blob.size();
            blob.resize(at + static_cast<size_t>(want));
            int32 read = 0;
            if (stream->read(blob.data() + at, want, &read) != kResultOk || read != want) return false;
            remaining -= want;
        }
        uint8_t extra = 0;
        int32 more = 0;
        if (stream->read(&extra, 1, &more) == kResultOk && more != 0) return false;
        present = true;
        return true;
    }
    // Checks a blob without an engine: a scratch instance loads it, then goes.
    bool checkBlob(const std::vector<uint8_t>& blob) const {
        const double rate = processSetup.sampleRate >= 8000.0 ? processSetup.sampleRate : 48000.0;
        const uint32_t block = processSetup.maxSamplesPerBlock >= 1 ? static_cast<uint32_t>(processSetup.maxSamplesPerBlock) : 512u;
        void* scratch = op_shell_create(rate, block, channels_);
        if (!scratch) return false;
        const bool ok = op_shell_load_state(scratch, blob.data(), static_cast<uint32_t>(blob.size())) != 0;
        op_shell_destroy(scratch);
        return ok;
    }
    // Hands the engine the parameter values held here.
    void pushValues(void* instance) {
        for (uint32_t i = 0; i < count_; ++i) op_shell_set_normalized(instance, i, static_cast<float>(values_[i].load()));
    }
    // Before the engine goes away, keep the plugin's state for the next one.
    void keepChunk() {
        if (engine_ && !save_blob(engine_, pendingBlob_)) pendingBlob_.clear();
    }
    std::vector<uint8_t> pendingBlob_;
    struct Change {
        int32 offset;
        uint32_t order;
        ParamID id;
        double value;
    };
    // Room for this many points per block. A block never allocates; if it
    // carries more, collectChanges thins each queue. One point per parameter
    // always fits.
    static constexpr uint32_t kMinChangeCapacity = 4096;
    bool relevant(ParamID id) const { return id < count_ || id == kBypassId; }
    // Gathers the points of every queue into changes_, ordered by (offset,
    // arrival). Returns how many it kept. Offsets are clamped into the block.
    // If the queues carry more points than changes_ holds, each queue keeps an
    // even share of its own points (first, last and evenly spaced between), so
    // the final value of every parameter is always kept and the ramp shape is
    // approximated; a queue beyond the capacity is ignored. Hosts send one queue
    // per parameter, so that last case does not occur in practice.
    uint32_t collectChanges(IParameterChanges* queues, int32 numSamples) {
        if (!queues || changes_.empty()) return 0;
        const uint32_t capacity = static_cast<uint32_t>(changes_.size());
        const int32 queueCount = queues->getParameterCount();
        uint64_t total = 0;
        uint32_t used = 0;
        for (int32 q = 0; q < queueCount; ++q) {
            IParamValueQueue* queue = queues->getParameterData(q);
            if (!queue || !relevant(queue->getParameterId())) continue;
            const int32 points = queue->getPointCount();
            if (points < 1) continue;
            total += static_cast<uint64_t>(points);
            ++used;
        }
        if (used == 0) return 0;
        const bool thin = total > capacity;
        const uint32_t share = thin ? std::max<uint32_t>(capacity / used, 1) : 0;
        uint32_t count = 0;
        uint32_t order = 0;
        for (int32 q = 0; q < queueCount && count < capacity; ++q) {
            IParamValueQueue* queue = queues->getParameterData(q);
            if (!queue) continue;
            const ParamID id = queue->getParameterId();
            if (!relevant(id)) continue;
            const int32 points = queue->getPointCount();
            if (points < 1) continue;
            const uint32_t keep = thin ? std::min<uint32_t>(share, static_cast<uint32_t>(points)) : static_cast<uint32_t>(points);
            for (uint32_t k = 0; k < keep && count < capacity; ++k) {
                int32 index = static_cast<int32>(k);
                if (keep < static_cast<uint32_t>(points))
                    index = keep == 1 ? points - 1 : static_cast<int32>(static_cast<uint64_t>(k) * static_cast<uint64_t>(points - 1) / (keep - 1));
                int32 at = 0;
                ParamValue value = 0.0;
                if (queue->getPoint(index, at, value) != kResultTrue || !std::isfinite(value)) continue;
                changes_[count++] = Change{std::clamp(at, 0, numSamples), order++, id, value};
            }
        }
        auto before = [](const Change& a, const Change& b) { return a.offset != b.offset ? a.offset < b.offset : a.order < b.order; };
        if (!std::is_sorted(changes_.begin(), changes_.begin() + count, before))
            std::sort(changes_.begin(), changes_.begin() + count, before);
        return count;
    }
    // Applies every change whose offset is at or before `offset` (all of them
    // when `all`), in order. The plugin sees each value on its next process.
    bool applyDue(uint32_t& next, uint32_t pending, int32 offset, bool all) {
        while (next < pending && (all || changes_[next].offset <= offset)) {
            const Change& change = changes_[next++];
            if (change.id == kBypassId) {
                bypass_.store(change.value >= 0.5);
                continue;
            }
            const double clamped = std::clamp(change.value, 0.0, 1.0);
            values_[change.id].store(clamped);
            if (engine_ && !op_shell_set_normalized(engine_, change.id, static_cast<float>(clamped))) return false;
        }
        return true;
    }
    // Pushes the input through a delay of the reported latency into dry_, so a
    // bypassed signal stays aligned with what the host compensates for. The
    // line always runs, so switching never starts from stale audio.
    void captureDry(const float* left, const float* right, int32 channels, uint32_t frames) {
        const float* in[2] = {left, right};
        for (int32 c = 0; c < channels; ++c) {
            float* dry = dry_[c].data();
            if (delay_ == 0) {
                std::memcpy(dry, in[c], frames * sizeof(float));
                continue;
            }
            float* line = line_[c].data();
            size_t pos = linePos_;
            for (uint32_t i = 0; i < frames; ++i) {
                dry[i] = line[pos];
                line[pos] = in[c][i];
                if (++pos == delay_) pos = 0;
            }
        }
        if (delay_ != 0) linePos_ = (linePos_ + frames) % delay_;
    }
    // Crossfades the engine output towards the dry signal (or back). Fully
    // bypassed output is the dry signal exactly.
    void applyBypass(float* left, float* right, int32 channels, uint32_t frames, float target) {
        float* out[2] = {left, right};
        if (mix_ == target) {
            if (target == 0.0f) return;
            for (int32 c = 0; c < channels; ++c) std::memcpy(out[c], dry_[c].data(), frames * sizeof(float));
            return;
        }
        float mix = mix_;
        for (uint32_t i = 0; i < frames; ++i) {
            if (mix < target) mix = std::min(target, mix + mixStep_);
            else if (mix > target) mix = std::max(target, mix - mixStep_);
            for (int32 c = 0; c < channels; ++c) out[c][i] = mix >= 1.0f ? dry_[c][i] : out[c][i] + (dry_[c][i] - out[c][i]) * mix;
        }
        mix_ = mix;
    }
    // Allocates everything the bypass needs; process never allocates.
    void prepareBypass() {
        delay_ = engine_ ? op_shell_latency(engine_) : 0;
        const size_t block = static_cast<size_t>(std::max(processSetup.maxSamplesPerBlock, 1));
        changes_.assign(std::max<size_t>(kMinChangeCapacity, static_cast<size_t>(count_) + 1), Change{0, 0, 0, 0.0});
        for (int c = 0; c < 2; ++c) {
            dry_[c].assign(block, 0.0f);
            line_[c].assign(delay_, 0.0f);
        }
        linePos_ = 0;
        mixStep_ = static_cast<float>(1.0 / std::max(1.0, kBypassFadeSeconds * processSetup.sampleRate));
        mix_ = bypass_.load() ? 1.0f : 0.0f;
    }
    void clearBypassLine() {
        for (auto& line : line_) std::fill(line.begin(), line.end(), 0.0f);
        linePos_ = 0;
    }
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
    std::atomic<bool> bypass_{false};
    // Bypass state, touched only by the audio thread after setActive.
    std::vector<Change> changes_;
    std::vector<float> dry_[2];
    std::vector<float> line_[2];
    size_t delay_ = 0;
    size_t linePos_ = 0;
    float mix_ = 0.0f;
    float mixStep_ = 1.0f;
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
        count_ = info.param_count;
        parameters.addParameter(STR16("Bypass"), nullptr, 1, 0.0, ParameterInfo::kCanAutomate | ParameterInfo::kIsBypass, kBypassId);
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream* stream) SMTG_OVERRIDE {
        if (!stream) return kResultFalse;
        IBStreamer reader(stream, kLittleEndian);
        int32 version = 0;
        int32 count = 0;
        if (!reader.readInt32(version) || version != 1 || !reader.readInt32(count) || count != static_cast<int32>(count_)) return kResultFalse;
        for (int32 i = 0; i < count; ++i) {
            double value = 0.0;
            if (!reader.readDouble(value) || !std::isfinite(value)) return kResultFalse;
            setParamNormalized(static_cast<ParamID>(i), value);
        }
        // The plugin-state trailer after the flag belongs to the processor.
        int32 bypass = 0;
        if (!reader.readInt32(bypass)) bypass = 0;
        setParamNormalized(kBypassId, bypass != 0 ? 1.0 : 0.0);
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
        if (editor_ && result == kResultOk && tag < count_) op_editor_set_plain(editor_, tag, static_cast<float>(op_shell_to_plain(tag, value)));
        return result;
    }
    // Called by the view.
    void* openEditor(void* parent) {
        closeEditor();
        OpEditorHost host{this, hostBegin, hostPerform, hostEnd};
        editor_ = op_editor_create(&host);
        if (!editor_) return nullptr;
        for (uint32_t i = 0; i < count_; ++i) {
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

private:
    uint32_t count_ = 0;
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
