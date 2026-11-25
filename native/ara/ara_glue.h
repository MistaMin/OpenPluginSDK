// SPDX-License-Identifier: MIT
// ARA 2 (Celemony's Audio Random Access) for the VST3 and Audio Unit shells, built when OP_ARA_SDK
// names an ARA SDK checkout (OP_HAS_ARA=1; OP_ARA_VST3=1 in the VST3 target adds its factory
// class). It uses the ARA Library's plug-in classes (Apache 2.0).
//
// What the shell does with ARA, for any plugin:
// - The factory lists an ARA main factory class; the processor is an ARA plug-in entry point
//   (IPlugInEntryPoint, IPlugInEntryPoint2), so an ARA host binds instances to its document.
// - The document controller reads every audio source the host enables sample access for into
//   memory, once, and hands the whole source to the plugin: op_ara_analyze (op_shell.h).
// - An instance bound as a playback renderer plays the host's playback regions from those
//   copies: the shell renders them into the plugin's input, so the plugin processes the clips
//   as it would its input.
#pragma once
#if OP_HAS_ARA

#include <atomic>
#include <cstdint>
#include <vector>

#if OP_ARA_VST3
#include "ARA_API/ARAVST3.h"
#endif
#include "ARA_Library/PlugIn/ARAPlug.h"

namespace openplugin_ara {

// The ARA factory of this plugin (identity from op_shell_info).
const ARA::ARAFactory* factory() noexcept;

class Renderer : public ARA::PlugIn::PlaybackRenderer {
public:
    using ARA::PlugIn::PlaybackRenderer::PlaybackRenderer;
    void enable(double sampleRate, int channels) noexcept {
        rate_ = sampleRate;
        channels_ = channels;
    }
    // Fills out[0..channels) with `frames` samples of the playback regions from song sample
    // `position` (silence while not playing or while the host edits the document).
    void render(float* const* out, int64_t position, int64_t frames, bool playing) noexcept;

private:
    double rate_ = 0.0;
    int channels_ = 0;
};

#if OP_ARA_VST3
// The VST3 class an ARA host asks for the factory (category kARAMainFactoryClass).
class MainFactory : public ARA::IMainFactory {
public:
    MainFactory() { FUNKNOWN_CTOR }
    virtual ~MainFactory() { FUNKNOWN_DTOR }
    static Steinberg::FUnknown* create(void*) { return static_cast<ARA::IMainFactory*>(new MainFactory()); }
    DECLARE_FUNKNOWN_METHODS
    const ARA::ARAFactory* PLUGIN_API getFactory() SMTG_OVERRIDE { return factory(); }
};

// The class ID of the main factory: the processor's, with its last byte inverted (stable per
// plugin, and never equal to the processor or controller ID).
void main_factory_cid(const uint8_t processor[16], uint8_t out[16]) noexcept;
#endif

// Whether an instance passes its input through instead of processing it: a host may split the
// roles over two instances (a hidden playback renderer per clip and the editor instance on the
// track, whose parameters the user edits and automates). The plugin's settings are its
// parameters, so processing stays with the editor instance; a renderer-only instance plays the
// clips unprocessed. An instance with every role does both.
inline bool renderer_only(ARA::PlugIn::PlugInExtension& ext) noexcept {
    return ext.getPlaybackRenderer() && !ext.getEditorRenderer() && !ext.getEditorView();
}

}  // namespace openplugin_ara

#endif
