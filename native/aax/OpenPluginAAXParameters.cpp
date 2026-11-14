// SPDX-License-Identifier: MIT
#include "OpenPluginAAX.h"
#include "AAX_CBinaryDisplayDelegate.h"
#include "AAX_CBinaryTaperDelegate.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CLinearTaperDelegate.h"
#include "AAX_CLogTaperDelegate.h"
#include "AAX_CNumberDisplayDelegate.h"
#include "AAX_CParameter.h"
#include "AAX_Errors.h"
#include <memory>
#include <string>
#include <vector>
namespace openplugin_aax {
namespace {
Packet defaults() {
    Packet packet{};
    OpShellInfo info{};
    if (!op_shell_info(&info)) return packet;
    packet.count = info.param_count;
    for (uint32_t i = 0; i < info.param_count && i < kMaxParams; ++i) {
        OpParamDesc desc{};
        if (op_shell_param(i, &desc)) packet.plain[i] = static_cast<float>(desc.default_plain);
    }
    return packet;
}
}
class Parameters final : public AAX_CEffectParameters {
    mutable ProcessState state_;
    std::vector<std::string> ids_;
public:
    static AAX_CEffectParameters* AAX_CALLBACK create() { return new Parameters; }
    AAX_Result EffectInit() override {
        if (!Controller()) return AAX_ERROR_NOT_INITIALIZED;
        OpShellInfo info{};
        if (!op_shell_info(&info) || info.abi != OP_SHELL_ABI || info.param_count == 0 || info.param_count > kMaxParams)
            return AAX_ERROR_INVALID_ARGUMENT;
        AAX_CSampleRate rate = 0;
        AAX_EStemFormat format = AAX_eStemFormat_None;
        AAX_Result result = Controller()->GetSampleRate(&rate);
        if (result != AAX_SUCCESS) return result;
        result = Controller()->GetInputStemFormat(&format);
        if (result != AAX_SUCCESS) return result;
        const uint32_t channels = format == AAX_eStemFormat_Mono ? 1u : format == AAX_eStemFormat_Stereo ? 2u : 0u;
        if (!state_.prepare(rate, channels)) return AAX_ERROR_INVALID_ARGUMENT;
        ids_.resize(info.param_count);
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
                    AAX_CNumberDisplayDelegate<float>(), true);
                parameter->SetType(AAX_eParameterType_Continuous);
            } else if ((desc.flags & OP_PARAM_DISCRETE) && desc.steps > 1) {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    AAX_CNumberDisplayDelegate<float>(), true);
                parameter->SetNumberOfSteps(static_cast<uint32_t>(desc.steps));
                parameter->SetType(AAX_eParameterType_Discrete);
            } else {
                parameter = std::make_unique<AAX_CParameter<float>>(
                    ids_[i].c_str(), AAX_CString(desc.name), initial,
                    AAX_CLinearTaperDelegate<float>(static_cast<float>(desc.min), static_cast<float>(desc.max)),
                    AAX_CNumberDisplayDelegate<float>(), true);
                // The step count stays at the AAX default for a continuous parameter.
                // The AAX Validator rejects 10001 ("Number of parameter steps exceeds limit").
                parameter->SetType(AAX_eParameterType_Continuous);
            }
            mParameterManager.AddParameter(parameter.release());
            result = mPacketDispatcher.RegisterPacket(ids_[i].c_str(), controlsField, this, &Parameters::packet);
            if (result != AAX_SUCCESS) return result;
        }
        auto bypass = std::make_unique<AAX_CParameter<bool>>(
            cDefaultMasterBypassID, AAX_CString("Bypass"), false,
            AAX_CBinaryTaperDelegate<bool>(), AAX_CBinaryDisplayDelegate<bool>("Off", "On"), true);
        bypass->SetNumberOfSteps(2);
        bypass->SetType(AAX_eParameterType_Discrete);
        mParameterManager.AddParameter(bypass.release());
        result = mPacketDispatcher.RegisterPacket(cDefaultMasterBypassID, controlsField, this, &Parameters::packet);
        if (result != AAX_SUCCESS) return result;
        return Controller()->SetSignalLatency(static_cast<int32_t>(op_shell_latency(state_.engine)));
    }
    AAX_Result ResetFieldData(AAX_CFieldIndex field, void* destination, uint32_t size) const override {
        if (field == stateField) {
            if (!destination || size != sizeof(ProcessState*)) return AAX_ERROR_INVALID_ARGUMENT;
            if (state_.engine) op_shell_reset(state_.engine);
            *static_cast<ProcessState**>(destination) = &state_;
            return AAX_SUCCESS;
        }
        if (field == controlsField) {
            if (!destination || size != sizeof(Packet)) return AAX_ERROR_INVALID_ARGUMENT;
            *static_cast<Packet*>(destination) = defaults();
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
    AAX_Result packet(AAX_CPacket& destination) {
        Packet next = defaults();
        for (uint32_t i = 0; i < ids_.size(); ++i) {
            auto* parameter = mParameterManager.GetParameterByID(ids_[i].c_str());
            float value = 0.f;
            if (!parameter || !parameter->GetValueAsFloat(&value)) return AAX_ERROR_INVALID_PARAMETER_ID;
            next.plain[i] = value;
        }
        bool bypass = false;
        if (auto* parameter = mParameterManager.GetParameterByID(cDefaultMasterBypassID))
            parameter->GetValueAsBool(&bypass);
        next.bypass = bypass ? 1u : 0u;
        next.count = static_cast<uint32_t>(ids_.size());
        *destination.GetPtr<Packet>() = next;
        return AAX_SUCCESS;
    }
};
AAX_CEffectParameters* AAX_CALLBACK createParameters() { return Parameters::create(); }
}
