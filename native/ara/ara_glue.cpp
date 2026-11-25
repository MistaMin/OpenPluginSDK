// SPDX-License-Identifier: MIT
// ARA 2 for the VST3 and Audio Unit shells: see ara_glue.h. The region rendering follows the ARA SDK
// examples' test plug-in (Celemony Software GmbH, Apache 2.0).
#if OP_HAS_ARA

#include "ara_glue.h"

#include <algorithm>
#include <cstring>
#include <memory>
#include <string>
#include <thread>

#include "op_shell.h"

#if OP_ARA_VST3
ARA_DISABLE_VST3_WARNINGS_BEGIN
DEF_CLASS_IID(ARA::IMainFactory)
DEF_CLASS_IID(ARA::IPlugInEntryPoint)
DEF_CLASS_IID(ARA::IPlugInEntryPoint2)
IMPLEMENT_FUNKNOWN_METHODS(openplugin_ara::MainFactory, ARA::IMainFactory, ARA::IMainFactory::iid)
ARA_DISABLE_VST3_WARNINGS_END
#endif

namespace openplugin_ara {
namespace {

struct Identity {
    std::string name, vendor, version, url, factoryId, archiveId;
    Identity() {
        OpShellInfo info{};
        std::memset(&info, 0, sizeof(info));
        op_shell_info(&info);
        name = info.name;
        vendor = info.vendor;
        version = info.version;
        const char* page = op_ara_url();
        url = page && *page ? page : "https://www.ara-audio.org";
        factoryId = std::string(info.id) + ".ara.factory";
        archiveId = std::string(info.id) + ".ara.archive.1";
    }
};

const Identity& identity() {
    static const Identity id;
    return id;
}

// One audio source: its samples, read once when the host enables access.
class Source : public ARA::PlugIn::AudioSource {
public:
    using ARA::PlugIn::AudioSource::AudioSource;
    void read() {
        const auto channels = static_cast<size_t>(std::max<ARA::ARAChannelCount>(getChannelCount(), 0));
        const auto frames = static_cast<size_t>(std::max<ARA::ARASampleCount>(getSampleCount(), 0));
        cache_.assign(channels * frames, 0.0f);
        std::vector<void*> ptrs(channels);
        for (size_t c = 0; c < channels; ++c) ptrs[c] = cache_.data() + c * frames;
        if (channels && frames) {
            ARA::PlugIn::HostAudioReader reader(this);
            reader.readAudioSamples(0, static_cast<ARA::ARASampleCount>(frames), ptrs.data());
        }
        frames_ = frames;
        channels_ = channels;
    }
    void drop() {
        cache_.clear();
        cache_.shrink_to_fit();
        frames_ = 0;
        channels_ = 0;
    }
    // The plugin sees each source once per change of its samples.
    void analyze() {
        if (!frames_ || !channels_) return;
        std::vector<const float*> ptrs(channels_);
        for (size_t c = 0; c < channels_; ++c) ptrs[c] = cache_.data() + c * frames_;
        op_ara_analyze(getPersistentID().c_str(), static_cast<uint32_t>(channels_), static_cast<uint64_t>(frames_), getSampleRate(), ptrs.data());
    }
    const float* channel(size_t c) const { return cache_.data() + c * frames_; }
    size_t frames() const { return frames_; }
    size_t channels() const { return channels_; }

private:
    std::vector<float> cache_;
    size_t frames_ = 0, channels_ = 0;
};

// The editor view: the host tells it which regions (or region sequences) the user selected;
// their clips go to the plugin (op_ara_select), so its editor works on what is selected.
class View : public ARA::PlugIn::EditorView {
public:
    // ARA forwards selections only to an open editor. The plugin's editor reads the selection
    // whenever it shows, so the view counts as open from the start.
    explicit View(ARA::PlugIn::DocumentController* controller) noexcept : ARA::PlugIn::EditorView(controller) { setEditorOpen(true); }

protected:
    void doNotifySelection(const ARA::PlugIn::ViewSelection* selection) noexcept override {
        std::vector<std::string> ids;
        auto add = [&ids](const ARA::PlugIn::PlaybackRegion* region) {
            const std::string& id = region->getAudioModification()->getAudioSource()->getPersistentID();
            if (std::find(ids.begin(), ids.end(), id) == ids.end()) ids.push_back(id);
        };
        if (selection) {
            for (const auto* region : selection->getPlaybackRegions()) add(region);
            for (const auto* sequence : selection->getRegionSequences())
                for (const auto* region : sequence->getPlaybackRegions()) add(region);
        }
        std::vector<const char*> ptrs;
        for (const auto& id : ids) ptrs.push_back(id.c_str());
        op_ara_select(ptrs.data(), static_cast<uint32_t>(ptrs.size()));
    }
};

class Controller : public ARA::PlugIn::DocumentController {
public:
    using ARA::PlugIn::DocumentController::DocumentController;

    // Renderers read the sources only while no edit is under way: the host edits the
    // document on the main thread, renderers run on the audio thread.
    bool rendererEnter() noexcept {
        if (!open_.load(std::memory_order_acquire)) return false;
        inside_.fetch_add(1, std::memory_order_acq_rel);
        if (!open_.load(std::memory_order_acquire)) {
            inside_.fetch_sub(1, std::memory_order_acq_rel);
            return false;
        }
        return true;
    }
    void rendererLeave() noexcept { inside_.fetch_sub(1, std::memory_order_acq_rel); }

protected:
    void close() noexcept {
        open_.store(false, std::memory_order_release);
        while (inside_.load(std::memory_order_acquire) > 0) std::this_thread::yield();
    }
    void open() noexcept { open_.store(true, std::memory_order_release); }

    void willBeginEditing() noexcept override { close(); }
    void didEndEditing() noexcept override { open(); }

    // No content of its own to store: the analysis is made again from the samples.
    bool doRestoreObjectsFromArchive(ARA::PlugIn::HostArchiveReader*, const ARA::PlugIn::RestoreObjectsFilter*) noexcept override { return true; }
    bool doStoreObjectsToArchive(ARA::PlugIn::HostArchiveWriter*, const ARA::PlugIn::StoreObjectsFilter*) noexcept override { return true; }

    // Tempo, bars and key do not change what the plugin does.
    void doUpdateMusicalContextContent(ARA::PlugIn::MusicalContext*, const ARA::ARAContentTimeRange*, ARA::ContentUpdateScopes) noexcept override {}

    ARA::PlugIn::AudioSource* doCreateAudioSource(ARA::PlugIn::Document* document, ARA::ARAAudioSourceHostRef hostRef) noexcept override {
        return new Source(document, hostRef);
    }
    void willEnableAudioSourceSamplesAccess(ARA::PlugIn::AudioSource*, bool) noexcept override {
        if (!isHostEditingDocument()) close();
    }
    void didEnableAudioSourceSamplesAccess(ARA::PlugIn::AudioSource* audioSource, bool enable) noexcept override {
        auto* source = static_cast<Source*>(audioSource);
        if (enable) {
            source->read();
            source->analyze();
        } else {
            source->drop();
        }
        if (!isHostEditingDocument()) open();
    }
    void doUpdateAudioSourceContent(ARA::PlugIn::AudioSource* audioSource, const ARA::ARAContentTimeRange*, ARA::ContentUpdateScopes scopes) noexcept override {
        auto* source = static_cast<Source*>(audioSource);
        if (!scopes.affectSamples() || !source->isSampleAccessEnabled()) return;
        const bool editing = isHostEditingDocument();
        if (!editing) close();
        source->read();
        source->analyze();
        if (!editing) open();
    }
    ARA::PlugIn::PlaybackRenderer* doCreatePlaybackRenderer() noexcept override { return new Renderer(this); }
    ARA::PlugIn::EditorView* doCreateEditorView() noexcept override { return new View(this); }

private:
    std::atomic<bool> open_{true};
    std::atomic<int> inside_{0};
};

class Config : public ARA::PlugIn::FactoryConfig {
public:
    ARA::ARAAPIGeneration getHighestSupportedApiGeneration() const noexcept override { return ARA::kARAAPIGeneration_2_3_Final; }
    const char* getFactoryID() const noexcept override { return identity().factoryId.c_str(); }
    const char* getPlugInName() const noexcept override { return identity().name.c_str(); }
    const char* getManufacturerName() const noexcept override { return identity().vendor.c_str(); }
    const char* getInformationURL() const noexcept override { return identity().url.c_str(); }
    const char* getVersion() const noexcept override { return identity().version.c_str(); }
    const char* getDocumentArchiveID() const noexcept override { return identity().archiveId.c_str(); }
};

}  // namespace

const ARA::ARAFactory* factory() noexcept {
    return ARA::PlugIn::PlugInEntry::getPlugInEntry<Config, Controller>()->getFactory();
}

#if OP_ARA_VST3
void main_factory_cid(const uint8_t processor[16], uint8_t out[16]) noexcept {
    std::memcpy(out, processor, 16);
    out[15] = static_cast<uint8_t>(~out[15]);
    out[14] = static_cast<uint8_t>(out[14] ^ 0xA5);
}
#endif

void Renderer::render(float* const* out, int64_t position, int64_t frames, bool playing) noexcept {
    for (int c = 0; c < channels_; ++c) std::memset(out[c], 0, sizeof(float) * static_cast<size_t>(frames));
    if (!playing || channels_ <= 0) return;
    auto* controller = getDocumentController<Controller>();
    if (!controller->rendererEnter()) return;
    const int64_t end = position + frames;
    for (const auto& region : getPlaybackRegions()) {
        const auto* source = region->getAudioModification()->getAudioSource<const Source>();
        if (!source->isSampleAccessEnabled() || source->getSampleRate() != rate_ || !source->frames()) continue;
        const int64_t regionStart = region->getStartInPlaybackSamples(rate_);
        const int64_t regionEnd = region->getEndInPlaybackSamples(rate_);
        if (end <= regionStart || regionEnd <= position) continue;
        const int64_t offset = region->getStartInAudioModificationSamples() - regionStart;
        const int64_t sourceFirst = std::max<int64_t>(0, region->getStartInAudioModificationSamples());
        const int64_t sourceLast = std::min<int64_t>(static_cast<int64_t>(source->frames()), region->getEndInAudioModificationSamples());
        const int64_t from = std::max({regionStart, position, sourceFirst - offset});
        const int64_t to = std::min({regionEnd, end, sourceLast - offset});
        if (to <= from) continue;
        const size_t inChannels = source->channels();
        for (int64_t song = from; song < to; ++song) {
            const auto i = static_cast<size_t>(song - position);
            const auto s = static_cast<size_t>(song + offset);
            if (inChannels == static_cast<size_t>(channels_)) {
                for (size_t c = 0; c < inChannels; ++c) out[c][i] += source->channel(c)[s];
            } else {
                float mono = 0.0f;
                for (size_t c = 0; c < inChannels; ++c) mono += source->channel(c)[s];
                mono /= static_cast<float>(inChannels);
                for (int c = 0; c < channels_; ++c) out[c][i] += mono;
            }
        }
    }
    controller->rendererLeave();
}

}  // namespace openplugin_ara

#endif
