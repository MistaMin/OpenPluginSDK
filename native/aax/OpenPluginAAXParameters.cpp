// SPDX-License-Identifier: MIT
#include "OpenPluginAAX.h"
#include "OpenPluginAAXState.h"
#include "AAX_CBinaryDisplayDelegate.h"
#include "AAX_CBinaryTaperDelegate.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CLinearTaperDelegate.h"
#include "AAX_CLogTaperDelegate.h"
#include "AAX_CString.h"
#include "AAX_IDisplayDelegate.h"
#include <algorithm>
#include <cmath>
#include "AAX_CParameter.h"
#include "AAX_Errors.h"
#include <atomic>
#include <cstring>
#include <memory>
#include <string>
#include <utility>
#include <vector>
namespace openplugin_aax {
namespace {
PacketBuffer defaults() {
    OpShellInfo info{};
    if (!op_shell_info(&info) || info.param_count > kMaxParams) return PacketBuffer(0);
    PacketBuffer packet(info.param_count);
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        if (op_shell_param(i, &desc)) packet.plain()[i] = static_cast<float>(desc.default_plain);
    }
    return packet;
}
}
// The plugin's own parameter text (shell ABI 3), not the SDK's number display. Main
// thread only: both ABI calls may allocate, and AAX asks for parameter text through
// the data model's main-thread entry points, never from the audio callback.
std::string shellText(uint32_t index, double plain) {
    char small[128];
    const size_t length = op_shell_param_format(index, plain, small, sizeof small);
    if (length < sizeof small) return std::string(small, length);
    std::string big(length + 1, '\0');
    const size_t again = op_shell_param_format(index, plain, &big[0], big.size());
    big.resize(std::min(again, big.size() - 1));
    return big;
}
// The ABI has no short form of a parameter's text, so a short request is answered
// from the full text, in this order, taking the first that fits `maxChars` bytes:
// the whole text; the text without spaces ("6.0 dB" -> "6.0dB"); the number alone
// when the text starts with one ("6.0"); the text cut on a UTF-8 boundary. Bytes,
// not characters, are counted, so the result never exceeds what the host asked for
// however it counts. A negative limit is refused.
bool fitText(const std::string& text, int32_t maxChars, std::string* out) {
    if (!out || maxChars < 0) return false;
    const size_t limit = static_cast<size_t>(maxChars);
    *out = text;
    if (text.size() <= limit) return true;
    std::string packed;
    for (char c : text) if (c != ' ') packed += c;
    if (packed.size() <= limit) {
        *out = packed;
        return true;
    }
    const size_t space = text.find(' ');
    if (space != std::string::npos && space > 0 && space <= limit) {
        const char first = text[0];
        if ((first >= '0' && first <= '9') || first == '-' || first == '+' || first == '.') {
            *out = text.substr(0, space);
            return true;
        }
    }
    size_t cut = limit;
    while (cut > 0 && (static_cast<unsigned char>(text[cut]) & 0xC0) == 0x80) --cut;  // never inside a sequence
    while (cut > 0 && text[cut - 1] == ' ') --cut;
    *out = text.substr(0, cut);
    return true;
}
namespace {
// Display delegate of every shell parameter. The real value is the plain value, so the
// plugin's text for it is exactly what its own display shows (choice and toggle labels
// included). The taper stays the SDK's linear or log one.
class ShellDisplay final : public AAX_IDisplayDelegate<float> {
    uint32_t index_;
public:
    explicit ShellDisplay(uint32_t index) : index_(index) {}
    ShellDisplay* Clone() const override { return new ShellDisplay(*this); }
    bool ValueToString(float value, AAX_CString* out) const override {
        if (!out) return false;
        *out = AAX_CString(shellText(index_, value));
        return true;
    }
    bool ValueToString(float value, int32_t maxNumChars, AAX_CString* out) const override {
        std::string fitted;
        if (!out || !fitText(shellText(index_, value), maxNumChars, &fitted)) return false;
        *out = AAX_CString(fitted);
        return true;
    }
    bool StringToValue(const AAX_CString& text, float* value) const override {
        double plain = 0.0;
        if (!value || !op_shell_param_parse(index_, text.Get(), &plain) || !std::isfinite(plain)) return false;
        *value = static_cast<float>(plain);
        return true;
    }
};
}
class Parameters final : public AAX_CEffectParameters {
    mutable ProcessState state_;
    std::vector<std::string> ids_;
    // The controls packet is posted by this class, not by AAX_CPacketDispatcher,
    // because its size follows the parameter count and a dispatcher packet has a
    // compile-time size. Parameter updates only set the flag; the host's
    // GenerateCoefficients call posts, as AAX requires.
    mutable PacketBuffer posted_;
    bool dirty_ = true;
    std::atomic<bool> offline_{false};
public:
    static AAX_CEffectParameters* AAX_CALLBACK create() { return new Parameters; }
    AAX_Result EffectInit() override {
        if (!Controller()) return AAX_ERROR_NOT_INITIALIZED;
        AAX_CSampleRate rate = 0;
        AAX_EStemFormat input = AAX_eStemFormat_None;
        AAX_EStemFormat output = AAX_eStemFormat_None;
        AAX_Result result = Controller()->GetSampleRate(&rate);
        if (result != AAX_SUCCESS) return result;
        result = Controller()->GetInputStemFormat(&input);
        if (result != AAX_SUCCESS) return result;
        result = Controller()->GetOutputStemFormat(&output);
        if (result != AAX_SUCCESS) return result;
        result = buildStems(rate, input, output);
        if (result != AAX_SUCCESS) return result;
        dirty_ = true;
        return Controller()->SetSignalLatency(static_cast<int32_t>(op_shell_latency(state_.engine)));
    }
    // Everything EffectInit does that does not need an AAX controller.
    AAX_Result build(double rate, uint32_t channels) {
        const AAX_EStemFormat stem = channels == 1 ? AAX_eStemFormat_Mono : channels == 2 ? AAX_eStemFormat_Stereo : AAX_eStemFormat_None;
        return buildStems(rate, stem, stem);
    }
    AAX_Result buildStems(double rate, AAX_EStemFormat input, AAX_EStemFormat output) {
        OpShellInfo info{};
        if (!abiOk() || !op_shell_info(&info) || info.param_count == 0 || info.param_count > kMaxParams)
            return AAX_ERROR_INVALID_ARGUMENT;
        AAX_Result result = AAX_SUCCESS;
        if (!state_.prepareStems(rate, input, output)) return AAX_ERROR_INVALID_ARGUMENT;
        op_shell_set_offline(state_.engine, offline_ ? 1u : 0u);
        ids_.assign(info.param_count, std::string());
        posted_ = PacketBuffer(info.param_count);
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            if (!op_shell_param(i, &desc) || desc.id[0] == '\0') return AAX_ERROR_INVALID_PARAMETER_ID;
            ids_[i] = desc.id;
            for (uint32_t earlier = 0; earlier < i; ++earlier) {
                if (ids_[earlier] == ids_[i]) return AAX_ERROR_INVALID_PARAMETER_ID;
            }
            const float initial = static_cast<float>(desc.default_plain);
            std::unique_ptr<AAX_IParameter> parameter;
            if (desc.flags & OP_PARAM_LOG) {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLogTaperDelegate<float, 0>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i), true);
                parameter->SetType(AAX_eParameterType_Continuous);
            } else if ((desc.flags & OP_PARAM_DISCRETE) && desc.steps > 1) {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i), true);
                parameter->SetNumberOfSteps(static_cast<uint32_t>(desc.steps));
                parameter->SetType(AAX_eParameterType_Discrete);
            } else {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i), true);
                // The step count stays at the AAX default for a continuous parameter.
                // The AAX Validator rejects 10001 ("Number of parameter steps exceeds limit").
                parameter->SetType(AAX_eParameterType_Continuous);
            }
            mParameterManager.AddParameter(parameter.release());
        }
        auto bypass = std::make_unique<AAX_CParameter<bool>>(
            cDefaultMasterBypassID, AAX_CString("Bypass"), false,
            AAX_CBinaryTaperDelegate<bool>(), AAX_CBinaryDisplayDelegate<bool>("Off", "On"), true);
        bypass->SetNumberOfSteps(2);
        bypass->SetType(AAX_eParameterType_Discrete);
        mParameterManager.AddParameter(bypass.release());
        return result;
    }
    // AAX_CParameter ignores the length in GetValueString(max, ...); this honours it.
    AAX_Result GetParameterValueString(AAX_CParamID id, AAX_IString* out, int32_t maxLength) const override {
        const AAX_IParameter* parameter = mParameterManager.GetParameterByID(id);
        if (!parameter) return AAX_ERROR_INVALID_PARAMETER_ID;
        if (!out) return AAX_ERROR_NULL_ARGUMENT;
        AAX_CString text;
        if (!parameter->GetStringFromNormalizedValue(parameter->GetNormalizedValue(), maxLength, text)) return AAX_ERROR_INVALID_STRING_CONVERSION;
        *out = text;
        return AAX_SUCCESS;
    }
    // State chunk: one chunk of ours replaces the SDK default (which keeps doubles
    // by id and accepts anything). See OpenPluginAAXState.h for the layout.
    AAX_Result GetNumberOfChunks(int32_t* count) const override {
        if (!count) return AAX_ERROR_NULL_ARGUMENT;
        *count = 1;
        return AAX_SUCCESS;
    }
    AAX_Result GetChunkIDFromIndex(int32_t index, AAX_CTypeID* id) const override {
        if (!id) return AAX_ERROR_NULL_ARGUMENT;
        if (index != 0) {
            *id = 0;
            return AAX_ERROR_INVALID_CHUNK_INDEX;
        }
        *id = state::kChunkId;
        return AAX_SUCCESS;
    }
    // The chunk carries the engine's own state blob, whose length the plugin decides. The
    // host sizes its buffer from this answer, so GetChunk may never write more than the
    // last answer: it refuses a state that grew since (a plugin's state can change between
    // the two calls) rather than overrun the host's buffer.
    AAX_Result GetChunkSize(AAX_CTypeID id, uint32_t* size) const override {
        if (!size) return AAX_ERROR_NULL_ARGUMENT;
        *size = 0;
        if (id != state::kChunkId) return AAX_ERROR_INVALID_CHUNK_ID;
        if (ids_.empty() || !state_.engine) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        uint32_t needed = 0;
        if (op_shell_save_state(state_.engine, nullptr, 0, &needed) != OP_STATE_TOO_SMALL || needed == 0) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        const size_t bytes = state::encodedSize(needed);
        if (bytes == 0 || bytes > INT32_MAX) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        reserved_ = static_cast<uint32_t>(bytes);
        *size = reserved_;
        return AAX_SUCCESS;
    }
    AAX_Result GetChunk(AAX_CTypeID id, AAX_SPlugInChunk* chunk) const override {
        if (!chunk) return AAX_ERROR_NULL_ARGUMENT;
        if (id != state::kChunkId) return AAX_ERROR_INVALID_CHUNK_ID;
        state::Values values{};
        if (!collect(values)) return AAX_ERROR_INVALID_PARAMETER_ID;
        std::vector<uint8_t> blob;
        if (!saveBlob(values, blob)) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        const size_t bytes = state::encodedSize(blob.size());
        if (bytes == 0 || bytes > reserved_) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        if (!state::encode(values, blob.data(), blob.size(), reinterpret_cast<uint8_t*>(chunk->fData), bytes)) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        chunk->fSize = static_cast<int32_t>(bytes);
        chunk->fVersion = static_cast<int32_t>(state::kVersion);
        std::memset(chunk->fName, 0, sizeof chunk->fName);
        std::strncpy(reinterpret_cast<char*>(chunk->fName), "OpenPlugin State", sizeof chunk->fName - 1);
        return AAX_SUCCESS;
    }
    AAX_Result SetChunk(AAX_CTypeID id, const AAX_SPlugInChunk* chunk) override {
        if (!chunk) return AAX_ERROR_NULL_ARGUMENT;
        if (id != state::kChunkId) return AAX_ERROR_INVALID_CHUNK_ID;
        state::Values values{};
        if (!parse(chunk, values)) return AAX_ERROR_MALFORMED_CHUNK;
        return apply(values);
    }
    AAX_Result CompareActiveChunk(const AAX_SPlugInChunk* chunk, AAX_CBoolean* equal) const override {
        if (!chunk || !equal) return AAX_ERROR_NULL_ARGUMENT;
        // An id we do not know must not light the compare indicator.
        if (chunk->fChunkID != state::kChunkId) {
            *equal = true;
            return AAX_SUCCESS;
        }
        *equal = false;
        state::Values stored{}, current{};
        if (!parse(chunk, stored) || !collect(current)) return AAX_SUCCESS;
        bool same = stored.bypass == current.bypass && stored.count == current.count;
        for (uint32_t i = 0; same && i < current.count; ++i) same = stored.plain[i] == current.plain[i];
        // A version 2 chunk also names the plugin's own state; a version 1 chunk has none to compare.
        if (same && stored.version == state::kVersion) {
            std::vector<uint8_t> blob;
            same = saveBlob(current, blob) && blob.size() == stored.blobSize && std::memcmp(blob.data(), stored.blob, blob.size()) == 0;
        }
        *equal = same;
        return AAX_SUCCESS;
    }
    // Hooks for the proofs, which have no AAX controller.
    bool proofSetPlain(uint32_t index, float value) {
        auto* parameter = index < ids_.size() ? mParameterManager.GetParameterByID(ids_[index].c_str()) : nullptr;
        return parameter && parameter->SetValueWithFloat(value);
    }
    // Text through the host entry points (GetParameterStringFromValue / GetParameterValueFromString).
    bool proofText(uint32_t index, double normalized, int32_t maxChars, std::string* out) const {
        if (index >= ids_.size() || !out) return false;
        AAX_CString text;
        if (GetParameterStringFromValue(ids_[index].c_str(), normalized, &text, maxChars) != AAX_SUCCESS) return false;
        *out = text.Get();
        return true;
    }
    bool proofValue(uint32_t index, const char* text, double* normalized) const {
        return index < ids_.size() && text && normalized &&
               GetParameterValueFromString(ids_[index].c_str(), normalized, AAX_CString(text)) == AAX_SUCCESS;
    }
    bool proofSetBypass(bool on) {
        auto* parameter = mParameterManager.GetParameterByID(cDefaultMasterBypassID);
        return parameter && parameter->SetValueWithBool(on);
    }
    bool proofPacket(PacketBuffer* out) const { return out && fill(*out) == AAX_SUCCESS; }
    AAX_Result UpdateParameterNormalizedValue(AAX_CParamID id, double value, AAX_EUpdateSource source) override {
        const AAX_Result result = AAX_CEffectParameters::UpdateParameterNormalizedValue(id, value, source);
        dirty_ = true;
        return result;
    }
    // Posts the whole packet when any value or the bypass changed.
    AAX_Result GenerateCoefficients() override {
        if (!dirty_) return AAX_SUCCESS;
        dirty_ = false;
        const AAX_Result result = fill(posted_);
        if (result != AAX_SUCCESS || !Controller()) return result;
        return Controller()->PostPacket(controlsField, posted_.get(), static_cast<uint32_t>(posted_.bytes()));
    }
    // What GenerateCoefficients would post, without a controller: false if nothing changed.
    bool proofTakePacket(PacketBuffer* out) {
        if (!out || !dirty_) return false;
        dirty_ = false;
        return fill(*out) == AAX_SUCCESS;
    }
    bool proofUpdate(uint32_t index, double normalized) {
        const char* id = index == kProofBypass ? cDefaultMasterBypassID : index < ids_.size() ? ids_[index].c_str() : nullptr;
        return id && UpdateParameterNormalizedValue(id, normalized, AAX_eUpdateSource_Unspecified) == AAX_SUCCESS;
    }
    // The host's offline bounce (AAX_eNotificationEvent_Entering/ExitingOfflineMode, Pro Tools 11 and
    // later) reaches the plugin as set_offline. AudioSuite is a different plug-in type and is not
    // described. The call is lock-free and safe from the notification's thread.
    AAX_Result NotificationReceived(AAX_CTypeID type, const void* data, uint32_t size) override {
        if (type == AAX_eNotificationEvent_EnteringOfflineMode || type == AAX_eNotificationEvent_ExitingOfflineMode) {
            offline_ = type == AAX_eNotificationEvent_EnteringOfflineMode;
            if (state_.engine) op_shell_set_offline(state_.engine, offline_ ? 1u : 0u);
            return AAX_SUCCESS;
        }
        return AAX_CEffectParameters::NotificationReceived(type, data, size);
    }
    void* engine() const { return state_.engine; }
    // ---- telling the host about changes (ABI 6) ----------------------------------------
    // The host calls TimerWakeup about every 30 ms on its own non-real-time thread; the
    // shell asks the engine there what changed. A new latency goes to SetSignalLatency (the
    // host applies it and sends AAX_eNotificationEvent_SignalLatencyChanged; GetSignalLatency
    // then reads it). New parameter info goes out as AAX_eNotificationEvent_ParameterNameChanged
    // for each parameter, with its id as the payload, so the host reads the names again.
    // Nothing is taken without a controller, so a change waits until there is one to tell.
    AAX_Result TimerWakeup() override {
        const AAX_Result result = AAX_CEffectParameters::TimerWakeup();
        tellChanges(Controller());
        return result;
    }
    uint32_t tellChanges(AAX_IController* controller) {
        if (!controller || !state_.engine) return 0;
        const uint32_t flags = op_shell_take_changes(state_.engine) & (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO);
        if (flags & OP_CHANGE_LATENCY) controller->SetSignalLatency(static_cast<int32_t>(op_shell_latency(state_.engine)));
        if (flags & OP_CHANGE_PARAM_INFO) {
            for (const std::string& id : ids_)
                controller->SendNotification(AAX_eNotificationEvent_ParameterNameChanged, id.c_str(), static_cast<uint32_t>(id.size() + 1));
        }
        return flags;
    }
    AAX_Result ResetFieldData(AAX_CFieldIndex field, void* destination, uint32_t size) const override {
        if (field == stateField) {
            if (!destination || size != sizeof(ProcessState*)) return AAX_ERROR_INVALID_ARGUMENT;
            state_.reset();
            *static_cast<ProcessState**>(destination) = &state_;
            return AAX_SUCCESS;
        }
        if (field == controlsField) {
            const PacketBuffer initial = defaults();
            if (!destination || initial.count() == 0 || size != initial.bytes()) return AAX_ERROR_INVALID_ARGUMENT;
            std::memcpy(destination, initial.get(), size);
            return AAX_SUCCESS;
        }
        return AAX_CEffectParameters::ResetFieldData(field, destination, size);
    }
    AAX_Result GetCustomData(AAX_CTypeID id, uint32_t size, void* data, uint32_t* written) const override {
        if (id != kEngineData) return AAX_CEffectParameters::GetCustomData(id, size, data, written);
        if (!data || !written || size != sizeof(void*)) return AAX_ERROR_INVALID_ARGUMENT;
        *static_cast<void**>(data) = state_.engine;
        *written = sizeof(void*);
        return AAX_SUCCESS;
    }
private:
    mutable std::atomic<uint32_t> reserved_{0};  // bytes the last GetChunkSize promised
    // Saves the engine's state through the size-query protocol: ask for the size, allocate,
    // save, and ask again if the state grew in between. The engine first receives the
    // parameter values this object holds, so the blob matches the values saved beside it
    // even before any block has carried them. Main thread only: it allocates.
    bool saveBlob(const state::Values& values, std::vector<uint8_t>& blob) const {
        void* engine = state_.engine;
        if (!engine) return false;
        for (uint32_t i = 0; i < values.count; ++i)
            if (!op_shell_set_plain(engine, i, values.plain[i])) return false;
        uint32_t needed = 0;
        if (op_shell_save_state(engine, nullptr, 0, &needed) != OP_STATE_TOO_SMALL) return false;
        for (int attempt = 0; attempt < 8 && needed > 0; ++attempt) {
            blob.assign(needed, 0);
            uint32_t written = 0;
            const uint32_t result = op_shell_save_state(engine, blob.data(), static_cast<uint32_t>(blob.size()), &written);
            if (result == OP_STATE_OK) {
                if (written == 0 || written > blob.size()) return false;
                blob.resize(written);
                return true;
            }
            if (result != OP_STATE_TOO_SMALL) return false;
            needed = written;
        }
        return false;
    }
    // Reads every parameter into `out`, which is resized to the parameter count.
    AAX_Result fill(PacketBuffer& out) const {
        PacketBuffer next(static_cast<uint32_t>(ids_.size()));
        for (uint32_t i = 0; i < ids_.size(); ++i) {
            const auto* parameter = mParameterManager.GetParameterByID(ids_[i].c_str());
            float value = 0.f;
            if (!parameter || !parameter->GetValueAsFloat(&value)) return AAX_ERROR_INVALID_PARAMETER_ID;
            next.plain()[i] = value;
        }
        bool bypass = false;
        if (const auto* parameter = mParameterManager.GetParameterByID(cDefaultMasterBypassID))
            parameter->GetValueAsBool(&bypass);
        next.get()->bypass = bypass ? 1u : 0u;
        out = std::move(next);
        return AAX_SUCCESS;
    }
    bool collect(state::Values& out) const {
        PacketBuffer current;
        if (ids_.empty() || fill(current) != AAX_SUCCESS) return false;
        out = {};
        out.count = current.count();
        out.bypass = current.get()->bypass != 0;
        for (uint32_t i = 0; i < current.count(); ++i) out.plain[i] = current.plain()[i];
        return true;
    }
    static bool parse(const AAX_SPlugInChunk* chunk, state::Values& out) {
        if (chunk->fSize < 0) return false;
        if (state::decode(reinterpret_cast<const uint8_t*>(chunk->fData), static_cast<size_t>(chunk->fSize), out) != state::Status::ok) return false;
        // The header version and the payload's own version must agree.
        return chunk->fVersion == static_cast<int32_t>(out.version);
    }
    // `values` is already validated. Everything that can still fail is checked before
    // anything changes: every parameter exists. Then the plugin's blob (version 2) is
    // applied; the plugin refuses a bad blob without changing anything, and then no
    // parameter is touched either. Only then are the AAX parameters written, which cannot fail.
    AAX_Result apply(const state::Values& values) {
        if (values.count != ids_.size()) return AAX_ERROR_MALFORMED_CHUNK;
        std::vector<AAX_IParameter*> targets(values.count, nullptr);
        for (uint32_t i = 0; i < values.count; ++i) {
            targets[i] = mParameterManager.GetParameterByID(ids_[i].c_str());
            if (!targets[i]) return AAX_ERROR_INVALID_PARAMETER_ID;
        }
        AAX_IParameter* bypass = mParameterManager.GetParameterByID(cDefaultMasterBypassID);
        if (!bypass) return AAX_ERROR_INVALID_PARAMETER_ID;
        if (values.version == state::kVersion) {
            if (!state_.engine || values.blobSize > UINT32_MAX ||
                !op_shell_load_state(state_.engine, values.blob, static_cast<uint32_t>(values.blobSize)))
                return AAX_ERROR_MALFORMED_CHUNK;
            // The chunk's values are the ones the host sees; give them to the engine too.
            for (uint32_t i = 0; i < values.count; ++i) op_shell_set_plain(state_.engine, i, values.plain[i]);
        }
        for (uint32_t i = 0; i < values.count; ++i) targets[i]->SetValueWithFloat(values.plain[i]);
        bypass->SetValueWithBool(values.bypass);
        dirty_ = true;
        return AAX_SUCCESS;
    }
};
AAX_CEffectParameters* AAX_CALLBACK createParameters() { return Parameters::create(); }
bool proofInit(AAX_CEffectParameters* p, double rate, uint32_t channels) { return static_cast<Parameters*>(p)->build(rate, channels) == AAX_SUCCESS; }
bool proofInitStems(AAX_CEffectParameters* p, double rate, AAX_EStemFormat input, AAX_EStemFormat output) { return static_cast<Parameters*>(p)->buildStems(rate, input, output) == AAX_SUCCESS; }
bool proofNotify(AAX_CEffectParameters* p, AAX_CTypeID event) { return static_cast<Parameters*>(p)->NotificationReceived(event, nullptr, 0) == AAX_SUCCESS; }
void* proofEngine(AAX_CEffectParameters* p) { return static_cast<Parameters*>(p)->engine(); }
bool proofSetPlain(AAX_CEffectParameters* p, uint32_t index, float value) { return static_cast<Parameters*>(p)->proofSetPlain(index, value); }
bool proofSetBypass(AAX_CEffectParameters* p, bool on) { return static_cast<Parameters*>(p)->proofSetBypass(on); }
bool proofTakePacket(AAX_CEffectParameters* p, PacketBuffer* out) { return static_cast<Parameters*>(p)->proofTakePacket(out); }
bool proofText(AAX_CEffectParameters* p, uint32_t index, double normalized, int32_t maxChars, std::string* out) { return static_cast<Parameters*>(p)->proofText(index, normalized, maxChars, out); }
bool proofValue(AAX_CEffectParameters* p, uint32_t index, const char* text, double* normalized) { return static_cast<Parameters*>(p)->proofValue(index, text, normalized); }
bool proofUpdate(AAX_CEffectParameters* p, uint32_t index, double normalized) { return static_cast<Parameters*>(p)->proofUpdate(index, normalized); }
bool proofPacket(AAX_CEffectParameters* p, PacketBuffer* out) { return static_cast<Parameters*>(p)->proofPacket(out); }
uint32_t proofWakeup(AAX_CEffectParameters* p, AAX_IController* controller) { return static_cast<Parameters*>(p)->tellChanges(controller); }
AAX_Result proofTimerWakeup(AAX_CEffectParameters* p) { return p->TimerWakeup(); }
}
