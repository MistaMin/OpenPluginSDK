// SPDX-License-Identifier: MIT
#include "OpenPluginAAX.h"
#include "OpenPluginAAXState.h"
#include "main_thread.h"
#include "op_shell_calls.h"
#include "AAX_CBinaryDisplayDelegate.h"
#include "AAX_CBinaryTaperDelegate.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CLinearTaperDelegate.h"
#include "AAX_CLogTaperDelegate.h"
#include "AAX_CString.h"
#include "AAX_IDisplayDelegate.h"
#include "AAX_IString.h"
#include <algorithm>
#include <cmath>
#include "AAX_CParameter.h"
#include "AAX_Errors.h"
#include <atomic>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>
// The most steps an AAX parameter may have ("Number of parameter steps exceeds limit
// (2048)" in the AAX Validator 2024.6).
static constexpr int64_t kMaxDiscreteSteps = 2048;

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
std::string shellText(const void* engine, uint32_t index, double plain) { return op_shell_calls::instance_param_text(engine, index, plain); }
std::string shellText(uint32_t index, double plain) { return shellText(nullptr, index, plain); }
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
// The text is the engine's (ABI 9), so a renamed table shows its own text; `state` is the
// effect's process state, which outlives every parameter, and its engine may be replaced.
class ShellDisplay final : public AAX_IDisplayDelegate<float> {
    uint32_t index_;
    const ProcessState* state_;
public:
    ShellDisplay(uint32_t index, const ProcessState* state) : index_(index), state_(state) {}
    ShellDisplay* Clone() const override { return new ShellDisplay(*this); }
    const void* engine() const { return state_ ? state_->engine : nullptr; }
    bool ValueToString(float value, AAX_CString* out) const override {
        if (!out) return false;
        *out = AAX_CString(shellText(engine(), index_, value));
        return true;
    }
    bool ValueToString(float value, int32_t maxNumChars, AAX_CString* out) const override {
        std::string fitted;
        if (!out || !fitText(shellText(engine(), index_, value), maxNumChars, &fitted)) return false;
        *out = AAX_CString(fitted);
        return true;
    }
    bool StringToValue(const AAX_CString& text, float* value) const override {
        double plain = 0.0;
        if (!value || !op_shell_instance_param_parse(engine(), index_, text.Get(), &plain) || !std::isfinite(plain)) return false;
        *value = static_cast<float>(plain);
        return true;
    }
};
}
class Parameters final : public AAX_CEffectParameters {
    mutable ProcessState state_;
    std::vector<std::string> ids_;
    // The parameter manager's objects in table order, and its bypass: a value read is
    // a pointer away, not a search by string id.
    std::vector<AAX_IParameter*> params_;
    AAX_IParameter* bypass_ = nullptr;
    // The controls packet is posted by this class, not by AAX_CPacketDispatcher,
    // because its size follows the parameter count and a dispatcher packet has a
    // compile-time size. Parameter updates only set the flag; the host's
    // GenerateCoefficients call posts, as AAX requires.
    mutable PacketBuffer posted_;
    bool dirty_ = true;
    std::atomic<bool> offline_{false};
    // The host's track: Pro Tools names it (AAX_eNotificationEvent_TrackNameChanged) and
    // says nothing else; AAX has no notification or call for a track's colour or kind.
    // The notification may come on any thread. The name is kept at once (trackLock_), for
    // the view to read, and the plugin is told on the main thread: at once when the
    // notification came there, else from the main queue (main_thread.h).
    mutable std::mutex trackLock_;
    std::string trackName_;                  // UTF-8; empty for none
    uint32_t trackSerial_ = 0;               // 0: the host has said nothing
    std::atomic<bool> trackPending_{false};  // the engine has not heard the current name
    std::atomic<int32_t> stateChanges_{0};   // OP_CHANGE_STATE notices, added to the host's change count
    std::mutex trackTelling_;                // one delivery at a time, so the last name told is the last one given
    // What a block of work waiting on the main queue holds: this object, or null once it
    // is gone. The lock is held while the work runs, so the destructor waits for it.
    struct MainThreadLink {
        std::mutex lock;
        Parameters* owner;
    };
    std::shared_ptr<MainThreadLink> link_ = std::make_shared<MainThreadLink>();
    // Any thread but the algorithm's. A name equal to the last one changes nothing.
    void noteTrack(const char* text) {
        std::string name(text ? text : "");
        {
            std::lock_guard<std::mutex> lock(trackLock_);
            if (trackSerial_ != 0 && trackName_ == name) return;
            trackName_ = std::move(name);
            if (++trackSerial_ == 0) trackSerial_ = 1;
            trackPending_.store(true, std::memory_order_release);
        }
        tellTrack();
    }
    // Tells the engine on the main thread: now, or from the main queue.
    void tellTrack() {
        if (!trackPending_.load(std::memory_order_acquire)) return;
        if (onMainThread()) {
            deliverTrack();
            return;
        }
        postToMainThread(new std::shared_ptr<MainThreadLink>(link_), [](void* context) {
            const std::unique_ptr<std::shared_ptr<MainThreadLink>> link(static_cast<std::shared_ptr<MainThreadLink>*>(context));
            std::lock_guard<std::mutex> lock((*link)->lock);
            if ((*link)->owner) (*link)->owner->deliverTrack();
        });
    }
    // Main thread. Tells the engine what the host last said, once.
    void deliverTrack() {
        std::lock_guard<std::mutex> telling(trackTelling_);
        if (!state_.engine || !trackPending_.exchange(false, std::memory_order_acq_rel)) return;
        std::string name;
        {
            std::lock_guard<std::mutex> lock(trackLock_);
            name = trackName_;
        }
        op_shell_set_track_info(state_.engine, name.empty() ? nullptr : name.c_str(), 0, 0);
    }
public:
    Parameters() { link_->owner = this; }
    ~Parameters() override {
        // Work still waiting on the main queue finds no one.
        std::lock_guard<std::mutex> lock(link_->lock);
        link_->owner = nullptr;
    }
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
        {
            // A track named before there was an engine.
            std::lock_guard<std::mutex> lock(trackLock_);
            if (trackSerial_ != 0) trackPending_.store(true, std::memory_order_release);
        }
        tellTrack();
        ids_.assign(info.param_count, std::string());
        params_.clear();
        posted_ = PacketBuffer(info.param_count);
        for (uint32_t i = 0; i < info.param_count; ++i) {
            OpParamDesc desc{};
            if (!op_shell_param(i, &desc) || desc.id[0] == '\0') return AAX_ERROR_INVALID_PARAMETER_ID;
            ids_[i] = shellParamId(i);
            if (ids_[i].empty()) return AAX_ERROR_INVALID_PARAMETER_ID;
            for (uint32_t earlier = 0; earlier < i; ++earlier) {
                if (ids_[earlier] == ids_[i]) return AAX_ERROR_INVALID_PARAMETER_ID;
            }
            const float initial = static_cast<float>(desc.default_plain);
            std::unique_ptr<AAX_IParameter> parameter;
            if (desc.flags & OP_PARAM_LOG) {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLogTaperDelegate<float, 0>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i, &state_), true);
                parameter->SetType(AAX_eParameterType_Continuous);
            } else if ((desc.flags & OP_PARAM_DISCRETE) && desc.steps > 1 && desc.steps <= kMaxDiscreteSteps) {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i, &state_), true);
                parameter->SetNumberOfSteps(static_cast<uint32_t>(desc.steps));
                parameter->SetType(AAX_eParameterType_Discrete);
            } else {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    ShellDisplay(i, &state_), true);
                // The step count stays at the AAX default for a continuous parameter.
                // The AAX Validator rejects 10001 ("Number of parameter steps exceeds limit").
                // A discrete parameter with more steps than AAX takes (a bit mask of
                // shown items, for example) is described as continuous too; its plain
                // value still crosses the ABI unchanged.
                parameter->SetType(AAX_eParameterType_Continuous);
            }
            params_.push_back(parameter.get());
            mParameterManager.AddParameter(parameter.release());
        }
        auto bypass = std::make_unique<AAX_CParameter<bool>>(
            cDefaultMasterBypassID, AAX_CString("Bypass"), false,
            AAX_CBinaryTaperDelegate<bool>(), AAX_CBinaryDisplayDelegate<bool>("Off", "On"), true);
        bypass->SetNumberOfSteps(2);
        bypass->SetType(AAX_eParameterType_Discrete);
        bypass_ = bypass.get();
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
        blobBytes_ = needed;  // the GetChunk that follows saves into a buffer of this size, once
        reserved_ = static_cast<uint32_t>(bytes);
        *size = reserved_;
        return AAX_SUCCESS;
    }
    AAX_Result GetChunk(AAX_CTypeID id, AAX_SPlugInChunk* chunk) const override {
        if (!chunk) return AAX_ERROR_NULL_ARGUMENT;
        if (id != state::kChunkId) return AAX_ERROR_INVALID_CHUNK_ID;
        state::Values values{};
        if (!collect(values)) return AAX_ERROR_INVALID_PARAMETER_ID;
        // The engine first receives the parameter values this object holds, so the blob matches
        // the values saved beside it even before any block has carried them.
        for (uint32_t i = 0; state_.engine && i < values.count; ++i)
            if (!op_shell_set_plain(state_.engine, i, values.plain[i])) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
        state_.engineWritten();
        std::vector<uint8_t> blob;
        if (!saveBlob(blob)) return AAX_ERROR_INCORRECT_CHUNK_SIZE;
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
        if (id != state::kChunkId) {
            // A chunk of the plug-in this one replaces (a Pro Tools session made before): offered
            // to the plugin when it reads that kind (ABI 10), refused otherwise.
            if (foreignKinds_ & (1u << (OP_FOREIGN_AAX_CHUNK - 1))) return setForeignChunk(id, chunk);
            return AAX_ERROR_INVALID_CHUNK_ID;
        }
        state::Values values{};
        if (!parse(chunk, values)) return AAX_ERROR_MALFORMED_CHUNK;
        return apply(values);
    }
    // Pro Tools calls this often, for its Compare light. The values are compared first and a
    // difference ends the call; only equal values cost a save, one, and nothing is written to
    // the engine. A value the engine has not yet received from a block shows as a difference
    // until the next block carries it.
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
        if (stored.bypass != current.bypass || stored.count != current.count) return AAX_SUCCESS;
        // A chunk of another parameter layout holds its values in its blob alone.
        for (uint32_t i = 0; !stored.byBlob && i < current.count; ++i)
            if (stored.plain[i] != current.plain[i]) return AAX_SUCCESS;
        // A version 2 chunk also names the plugin's own state; a version 1 chunk has none to compare.
        std::vector<uint8_t> blob;
        *equal = stored.version != state::kVersion ||
                 (saveBlob(blob) && blob.size() == stored.blobSize && std::memcmp(blob.data(), stored.blob, blob.size()) == 0);
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
        // The track's name (Pro Tools 11.2 and later; Media Composer does not send it). The
        // payload is a const AAX_IString. The plugin hears it on the main thread (noteTrack);
        // the view reads it from GetCustomData on the main thread.
        if (type == AAX_eNotificationEvent_TrackNameChanged) {
            if (!data) return AAX_ERROR_NULL_ARGUMENT;
            noteTrack(static_cast<const AAX_IString*>(data)->Get());
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
    // The host asks whether anything changed since it last saved: parameter changes, plus
    // changes to saved state that is not a parameter.
    AAX_Result GetNumberOfChanges(int32_t* changes) const override {
        if (!changes) return AAX_ERROR_NULL_ARGUMENT;
        int32_t parameters = 0;
        const AAX_Result result = AAX_CEffectParameters::GetNumberOfChanges(&parameters);
        *changes = static_cast<int32_t>((static_cast<uint32_t>(parameters) + static_cast<uint32_t>(stateChanges_.load())) & 0x7fffffffu);
        return result;
    }
    uint32_t tellChanges(AAX_IController* controller) {
        if (!controller || !state_.engine) return 0;
        const uint32_t flags = op_shell_take_changes(state_.engine) & (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO | OP_CHANGE_STATE);
        if (flags & OP_CHANGE_STATE) stateChanges_.fetch_add(1);
        if (flags & OP_CHANGE_LATENCY) controller->SetSignalLatency(static_cast<int32_t>(op_shell_latency(state_.engine)));
        if (flags & OP_CHANGE_PARAM_INFO) {
            // The engine's names first (ABI 9), so the host reads the renamed table when it hears.
            for (uint32_t i = 0; i < ids_.size(); ++i) {
                OpParamDesc desc{};
                AAX_IParameter* parameter = mParameterManager.GetParameterByID(ids_[i].c_str());
                if (parameter && op_shell_instance_param(state_.engine, i, &desc)) parameter->SetName(AAX_CString(desc.name));
            }
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
        if (id == kTrackData) {
            if (!data || !written || size != sizeof(TrackData)) return AAX_ERROR_INVALID_ARGUMENT;
            auto* out = static_cast<TrackData*>(data);
            std::lock_guard<std::mutex> lock(trackLock_);
            out->serial = trackSerial_;
            size_t n = std::min(trackName_.size(), sizeof(out->name) - 1);
            while (n > 0 && n < trackName_.size() && (static_cast<unsigned char>(trackName_[n]) & 0xC0) == 0x80) --n;  // never inside a sequence
            std::memcpy(out->name, trackName_.data(), n);
            out->name[n] = '\0';
            *written = sizeof(TrackData);
            return AAX_SUCCESS;
        }
        if (id != kEngineData) return AAX_CEffectParameters::GetCustomData(id, size, data, written);
        if (!data || !written || size != sizeof(void*)) return AAX_ERROR_INVALID_ARGUMENT;
        *static_cast<void**>(data) = state_.engine;
        *written = sizeof(void*);
        return AAX_SUCCESS;
    }
private:
    mutable std::atomic<uint32_t> reserved_{0};  // bytes the last GetChunkSize promised
    mutable std::atomic<uint32_t> blobBytes_{0};  // the engine's state size at the last save or size answer
    // Saves the engine's state in one call, into a buffer of the size the state had last
    // time. Only a state that grew since costs another save, at the size the engine then
    // asked for. Main thread only: it allocates.
    bool saveBlob(std::vector<uint8_t>& blob) const {
        if (!op_shell_calls::save_blob(state_.engine, blob, blobBytes_)) return false;
        blobBytes_ = static_cast<uint32_t>(blob.size());
        return true;
    }
    // Reads every parameter into `out`, which is given the parameter count if it has another.
    AAX_Result fill(PacketBuffer& out) const {
        if (params_.size() != ids_.size()) return AAX_ERROR_INVALID_PARAMETER_ID;
        if (out.count() != params_.size()) out = PacketBuffer(static_cast<uint32_t>(params_.size()));
        for (size_t i = 0; i < params_.size(); ++i) {
            if (!params_[i]->GetValueAsFloat(&out.plain()[i])) return AAX_ERROR_INVALID_PARAMETER_ID;
        }
        bool bypass = false;
        if (bypass_) bypass_->GetValueAsBool(&bypass);
        out.get()->bypass = bypass ? 1u : 0u;
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
    // The plugin gets the chunk's id as four bytes in reading order (so 'abcd' is the bytes "abcd"),
    // then the chunk's data. All or nothing: the plugin refuses what it does not recognise and
    // changes nothing. After a success the values the plugin holds are the ones the host sees.
    AAX_Result setForeignChunk(AAX_CTypeID id, const AAX_SPlugInChunk* chunk) {
        if (!state_.engine || chunk->fSize < 0 || chunk->fSize > (16 << 20) || params_.size() != ids_.size()) return AAX_ERROR_MALFORMED_CHUNK;
        std::vector<uint8_t> bytes(4 + static_cast<size_t>(chunk->fSize));
        for (int i = 0; i < 4; ++i) bytes[static_cast<size_t>(i)] = static_cast<uint8_t>(id >> (24 - 8 * i));
        if (chunk->fSize > 0) std::memcpy(bytes.data() + 4, chunk->fData, static_cast<size_t>(chunk->fSize));
        if (!op_shell_load_foreign_state(state_.engine, OP_FOREIGN_AAX_CHUNK, bytes.data(), static_cast<uint32_t>(bytes.size()))) return AAX_ERROR_MALFORMED_CHUNK;
        state_.engineWritten();
        for (uint32_t i = 0; i < ids_.size(); ++i) params_[i]->SetValueWithFloat(op_shell_get_plain(state_.engine, i));
        dirty_ = true;
        return AAX_SUCCESS;
    }
    const uint32_t foreignKinds_ = op_shell_foreign_kinds();
    // `values` is already validated. Everything that can still fail is checked before
    // anything changes: every parameter exists. Then the plugin's blob (version 2) is
    // applied; the plugin refuses a bad blob without changing anything, and then no
    // parameter is touched either. Only then are the AAX parameters written, which cannot fail.
    AAX_Result apply(const state::Values& values) {
        if (values.count != ids_.size()) return AAX_ERROR_MALFORMED_CHUNK;
        const std::vector<AAX_IParameter*>& targets = params_;
        AAX_IParameter* bypass = bypass_;
        if (targets.size() != ids_.size() || !bypass) return AAX_ERROR_INVALID_PARAMETER_ID;
        if (values.version == state::kVersion) {
            if (!state_.engine || values.blobSize > UINT32_MAX ||
                !op_shell_load_state(state_.engine, values.blob, static_cast<uint32_t>(values.blobSize)))
                return AAX_ERROR_MALFORMED_CHUNK;
            state_.engineWritten();
            if (values.byBlob) {
                // Written by a build with another parameter order: the blob names its values
                // by parameter, so the engine now holds each one in its place. Read them back.
                for (uint32_t i = 0; i < values.count; ++i) targets[i]->SetValueWithFloat(op_shell_get_plain(state_.engine, i));
                bypass->SetValueWithBool(values.bypass);
                dirty_ = true;
                return AAX_SUCCESS;
            }
            // The chunk's values are the ones the host sees; give them to the engine too.
            for (uint32_t i = 0; i < values.count; ++i) op_shell_set_plain(state_.engine, i, values.plain[i]);
            state_.engineWritten();
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
AAX_Result proofNotifyData(AAX_CEffectParameters* p, AAX_CTypeID event, const void* data, uint32_t size) { return static_cast<Parameters*>(p)->NotificationReceived(event, data, size); }
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
