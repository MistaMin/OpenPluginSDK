// SPDX-License-Identifier: MIT
// Telling an AAX host about changes (shell ABI 6), through the shell's real
// AAX_CEffectParameters over the stand-in ABI (proof_stub.cpp, three
// parameters), with a fake AAX_IController that records SetSignalLatency and
// SendNotification and answers GetSignalLatency with what was set, as Pro
// Tools does once it has applied a new latency.
//
// Proved:
//  - a latency the plugin moves mid-session (from the audio thread) is told
//    once, as SetSignalLatency with the new value, from the host's wakeup
//    thread (TimerWakeup is never the audio thread), and the host reads it
//    back through GetSignalLatency;
//  - new parameter info is told once, as one
//    AAX_eNotificationEvent_ParameterNameChanged per parameter, each payload
//    the parameter's id with its NUL;
//  - a wakeup with nothing new says nothing; a latency the host already read
//    is not told again;
//  - with no controller (the data model's own TimerWakeup in a proof) nothing
//    is taken, so the change waits for a wakeup that can tell it;
//  - ABI 9, a renamed table: after the plugin renames its parameters the host
//    reads the new names (GetParameterName) and the new value text
//    (GetParameterValueString) and typed text is read by the new table; before
//    the wakeup that tells it, and through the calls without an instance, the
//    first table stays (the negative controls);
//  - ABI 9, the bypass follows the latency: a plugin that declares its largest
//    latency (256) and moves its latency mid-session, while bypassed, outputs
//    the input delayed by exactly the new latency (0 -> 100 -> 30 -> 256);
//    without the declaration the delay stays at the latency of prepare (the
//    negative control: delayed by 0, not by 100).
#include "OpenPluginAAX.h"
#include "proof_stub.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CString.h"
#include "AAX_IController.h"
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <thread>
#include <vector>

using namespace openplugin_aax;

namespace {
void require(bool ok, const char* what) {
    if (!ok) {
        std::fprintf(stderr, "AAX changes proof failed: %s\n", what);
        std::exit(1);
    }
}

// The host side of the data model. Only what the shell calls is implemented.
struct FakeController final : AAX_IController {
    int latencySets = 0;
    int32_t published = 0;  // what SetSignalLatency asked for; Pro Tools applies it, then reports it
    std::thread::id latencyThread;
    std::vector<std::string> renamed;
    int badPayloads = 0;
    std::thread::id renameThread;

    AAX_Result SetSignalLatency(int32_t samples) override {
        ++latencySets;
        published = samples;
        latencyThread = std::this_thread::get_id();
        return AAX_SUCCESS;
    }
    AAX_Result GetSignalLatency(int32_t* samples) const override {
        if (!samples) return AAX_ERROR_NULL_ARGUMENT;
        *samples = published;
        return AAX_SUCCESS;
    }
    AAX_Result SendNotification(AAX_CTypeID type, const void* data, uint32_t size) override {
        if (type != AAX_eNotificationEvent_ParameterNameChanged) return AAX_SUCCESS;
        renameThread = std::this_thread::get_id();
        const char* id = static_cast<const char*>(data);
        if (!id || size == 0 || id[size - 1] != '\0' || std::strlen(id) + 1 != size) {
            ++badPayloads;
            return AAX_ERROR_INVALID_ARGUMENT;
        }
        renamed.emplace_back(id);
        return AAX_SUCCESS;
    }
    AAX_Result SendNotification(AAX_CTypeID type) override { return SendNotification(type, nullptr, 0); }

    AAX_Result GetEffectID(AAX_IString*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetSampleRate(AAX_CSampleRate* rate) const override {
        *rate = 48000.0;
        return AAX_SUCCESS;
    }
    AAX_Result GetInputStemFormat(AAX_EStemFormat* format) const override {
        *format = AAX_eStemFormat_Stereo;
        return AAX_SUCCESS;
    }
    AAX_Result GetOutputStemFormat(AAX_EStemFormat* format) const override {
        *format = AAX_eStemFormat_Stereo;
        return AAX_SUCCESS;
    }
    AAX_Result GetCycleCount(AAX_EProperty, AAX_CPropertyValue*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetTODLocation(AAX_CTimeOfDay*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result SetCycleCount(AAX_EProperty*, AAX_CPropertyValue*, int32_t) override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result PostPacket(AAX_CFieldIndex, const void*, uint32_t) override { return AAX_SUCCESS; }
    AAX_Result GetCurrentMeterValue(AAX_CTypeID, float*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetMeterPeakValue(AAX_CTypeID, float*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result ClearMeterPeakValue(AAX_CTypeID) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetMeterCount(uint32_t*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetMeterClipped(AAX_CTypeID, AAX_CBoolean*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result ClearMeterClipped(AAX_CTypeID) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetNextMIDIPacket(AAX_CFieldIndex*, AAX_CMidiPacket*) override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetHybridSignalLatency(int32_t*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetCurrentAutomationTimestamp(AAX_CTransportCounter*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetHostName(AAX_IString*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetPlugInTargetPlatform(AAX_CTargetPlatform*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_Result GetIsAudioSuite(AAX_CBoolean*) const override { return AAX_ERROR_UNIMPLEMENTED; }
    AAX_IPageTable* CreateTableCopyForEffect(AAX_CPropertyValue, AAX_CPropertyValue, AAX_CPropertyValue, uint32_t, int32_t) const override { return nullptr; }
    AAX_IPageTable* CreateTableCopyForLayout(const char*, const char*, uint32_t, int32_t) const override { return nullptr; }
    AAX_IPageTable* CreateTableCopyForEffectFromFile(const char*, AAX_ETextEncoding, AAX_CPropertyValue, AAX_CPropertyValue, AAX_CPropertyValue, uint32_t, int32_t) const override { return nullptr; }
    AAX_IPageTable* CreateTableCopyForLayoutFromFile(const char*, AAX_ETextEncoding, const char*, uint32_t, int32_t) const override { return nullptr; }
};

// The host's wakeup: TimerWakeup runs on a thread of its own, never the audio thread.
uint32_t wakeup(AAX_CEffectParameters* parameters, FakeController& host, std::thread::id* on) {
    uint32_t told = 0;
    std::thread thread([&] {
        told = proofWakeup(parameters, &host);
        if (on) *on = std::this_thread::get_id();
    });
    thread.join();
    return told;
}

// What the host reads as a parameter's name.
std::string nameOf(AAX_CEffectParameters* parameters, const char* id) {
    AAX_CString name;
    require(parameters->GetParameterName(id, &name) == AAX_SUCCESS, "GetParameterName");
    return name.Get();
}
std::string textOf(AAX_CEffectParameters* parameters, uint32_t index, double normalized) {
    std::string text;
    require(proofText(parameters, index, normalized, 64, &text), "GetParameterValueString");
    return text;
}

// Bypassed blocks of 512 frames through the real process callback, a latency set before
// each one. Returns the input and output of channel 0.
struct Bypassed {
    std::vector<float> in, out;
};
Bypassed bypassed(ProcessState& state, const std::vector<uint32_t>& latencyPerBlock) {
    constexpr int32_t kBlock = 512;
    PacketBuffer packet(3);
    for (uint32_t i = 0; i < 3; ++i) {
        OpParamDesc desc{};
        require(op_shell_param(i, &desc) != 0, "param");
        packet.plain()[i] = static_cast<float>(desc.default_plain);
    }
    packet.get()->bypass = 1;
    Bypassed r;
    uint32_t x = 99;
    for (uint32_t latency : latencyPerBlock) {
        op_stub_set_latency(state.engine, latency);
        std::vector<float> a[2], b[2];
        float* input[2];
        float* output[2];
        for (int c = 0; c < 2; ++c) {
            a[c].resize(kBlock);
            for (auto& v : a[c]) {
                x = x * 1664525u + 1013904223u;
                v = static_cast<float>((x >> 8) & 0xFFFF) / 65536.0f - 0.5f;
            }
            b[c].assign(kBlock, 0.f);
            input[c] = a[c].data();
            output[c] = b[c].data();
        }
        int32_t frames = kBlock;
        auto* statePtr = &state;
        Context context{input, output, &frames, packet.get(), &statePtr};
        Context* contexts[] = {&context};
        process(contexts, contexts + 1);
        r.in.insert(r.in.end(), a[0].begin(), a[0].end());
        r.out.insert(r.out.end(), b[0].begin(), b[0].end());
    }
    return r;
}
// Is block `block` the input delayed by `latency`, sample for sample?
bool delayedBy(const Bypassed& r, size_t block, uint32_t latency) {
    for (size_t n = block * 512; n < (block + 1) * 512; ++n) {
        const float want = n >= latency ? r.in[n - latency] : 0.f;
        if (r.out[n] != want) return false;
    }
    return true;
}
}

int main() {
    std::unique_ptr<AAX_CEffectParameters> parameters(createParameters());
    require(parameters && proofInit(parameters.get(), 48000.0, 2), "the data model builds over the stand-in");
    void* engine = proofEngine(parameters.get());
    require(engine != nullptr, "an engine exists");
    FakeController host;
    int32_t read = -1;

    // Nothing has changed yet.
    require(wakeup(parameters.get(), host, nullptr) == 0 && host.latencySets == 0 && host.renamed.empty(), "a quiet wakeup tells nothing");

    // 1. The plugin moves its latency on the audio thread, mid-session.
    // The audio thread stays alive until the wakeup is done, so the two thread ids cannot be the same.
    std::atomic<bool> changed{false}, finished{false};
    std::thread::id audioThread;
    std::thread audio([&] {
        audioThread = std::this_thread::get_id();
        op_stub_set_latency(engine, 64);
        changed = true;
        while (!finished) std::this_thread::yield();
    });
    while (!changed) std::this_thread::yield();
    std::thread::id wakeThread;
    const uint32_t told = wakeup(parameters.get(), host, &wakeThread);
    finished = true;
    audio.join();
    require(told == OP_CHANGE_LATENCY, "the wakeup takes the latency change");
    require(host.latencySets == 1 && host.published == 64, "SetSignalLatency(64), once");
    require(host.latencyThread == wakeThread && host.latencyThread != audioThread, "told from the wakeup thread, not the audio thread");
    require(host.GetSignalLatency(&read) == AAX_SUCCESS && read == 64, "the host reads the new latency back");
    require(wakeup(parameters.get(), host, nullptr) == 0 && host.latencySets == 1, "told once");
    std::printf("AAX changes: latency 64 -> SetSignalLatency(64) once, from the wakeup thread; GetSignalLatency reads 64\n");

    // 2. New parameter info: one name notification per parameter, with the id as payload.
    op_stub_raise_changes(engine, OP_CHANGE_PARAM_INFO);
    require(wakeup(parameters.get(), host, &wakeThread) == OP_CHANGE_PARAM_INFO, "the wakeup takes the parameter info change");
    require(host.badPayloads == 0, "every payload is an id with its NUL");
    require(host.renamed == std::vector<std::string>({"p0", "p1", "p2"}), "AAX_eNotificationEvent_ParameterNameChanged for p0, p1, p2");
    require(host.renameThread == wakeThread && host.latencySets == 1, "names from the wakeup thread; the latency is not told again");
    require(wakeup(parameters.get(), host, nullptr) == 0 && host.renamed.size() == 3, "told once");
    std::printf("AAX changes: new parameter info -> ParameterNameChanged for each of %zu parameters, once\n", host.renamed.size());

    // 3. No controller: the data model's own wakeup takes nothing, so the change waits.
    op_stub_set_latency(engine, 10);
    const uint32_t takesBefore = op_stub_takes();
    require(proofTimerWakeup(parameters.get()) == AAX_SUCCESS && op_stub_takes() == takesBefore, "no controller, nothing taken");
    require(wakeup(parameters.get(), host, nullptr) == OP_CHANGE_LATENCY && host.latencySets == 2 && host.published == 10, "told at the next wakeup with a controller");

    // 4. A latency the host already read (op_shell_latency) is not told again; both at once are one wakeup.
    op_stub_set_latency(engine, 20);
    require(op_shell_latency(engine) == 20, "read");
    require(wakeup(parameters.get(), host, nullptr) == 0 && host.latencySets == 2, "a latency already read is quiet");
    op_stub_set_latency(engine, 30);
    op_stub_raise_changes(engine, OP_CHANGE_PARAM_INFO);
    require(wakeup(parameters.get(), host, nullptr) == (OP_CHANGE_LATENCY | OP_CHANGE_PARAM_INFO), "both in one wakeup");
    require(host.latencySets == 3 && host.published == 30 && host.renamed.size() == 6, "both told");

    // 5. ABI 9: a renamed table. The host reads the names and text again when told.
    require(nameOf(parameters.get(), "p0") == "Param 0" && textOf(parameters.get(), 0, 0.5) == "0.500", "the first table before the rename");
    op_stub_rename(engine);
    require(nameOf(parameters.get(), "p0") == "Param 0", "negative control: the name stays until the host is told");
    require(wakeup(parameters.get(), host, nullptr) == OP_CHANGE_PARAM_INFO && host.renamed.size() == 9, "the rename is told");
    for (uint32_t i = 0; i < 3; ++i) {
        const std::string id = "p" + std::to_string(i);
        require(nameOf(parameters.get(), id.c_str()) == "Param " + std::to_string(i) + " (renamed)", "the host reads the renamed name");
    }
    require(textOf(parameters.get(), 0, 0.5) == "0.500 renamed", "the host reads the renamed value text");
    double typed = -1.0;
    require(proofValue(parameters.get(), 1, "0.250 renamed", &typed) && typed == 0.25, "typed text is read by the renamed table");
    require(shellText(0, 0.5) == "0.500", "negative control: the call without an instance keeps the first table");
    std::printf("AAX changes: renamed table -> GetParameterName reads \"%s\", GetParameterValueString \"%s\"; the first table stays without an instance\n",
                nameOf(parameters.get(), "p0").c_str(), textOf(parameters.get(), 0, 0.5).c_str());

    // 6. ABI 9: the bypass follows a latency that changes mid-session.
    {
        op_stub_set_max_latency(256);
        ProcessState state;
        require(state.prepare(48000.0, 2) && state.delayCap == 256 && state.line[0].size() == 257, "the delay holds the declared largest latency");
        // Blocks 0-1 at 0; 2-4 at 100; 5-7 at 30; 8-10 at 256. The first block of each new latency
        // is the switch; the blocks after it are compared.
        const auto r = bypassed(state, {0, 0, 100, 100, 100, 30, 30, 30, 256, 256, 256});
        require(delayedBy(r, 1, 0), "latency 0: the input itself");
        require(delayedBy(r, 3, 100) && delayedBy(r, 4, 100), "latency 100: delayed by 100");
        require(!delayedBy(r, 3, 0), "control: the check tells 100 from 0");
        require(delayedBy(r, 6, 30) && delayedBy(r, 7, 30), "latency 30: delayed by 30");
        require(!delayedBy(r, 6, 100), "control: the check tells 30 from 100");
        require(delayedBy(r, 9, 256) && delayedBy(r, 10, 256), "latency 256: delayed by 256");
        std::printf("AAX changes: bypassed output follows the latency 0 -> 100 -> 30 -> 256, delayed exactly by each\n");
        // Negative control: no declaration. The delay was sized for the latency at prepare (0).
        op_stub_set_max_latency(0);
        ProcessState plain;
        require(plain.prepare(48000.0, 2) && plain.delayCap == 0, "undeclared: the delay holds the latency of prepare");
        const auto q = bypassed(plain, {0, 100, 100, 100});
        require(!delayedBy(q, 2, 100) && delayedBy(q, 2, 0), "negative control: undeclared, the bypass stays at the old latency");
        std::printf("AAX changes: negative control: without a declared largest latency the bypass keeps 0 after a change to 100\n");
    }
    std::printf("AAX changes proof passed\n");
    return 0;
}
