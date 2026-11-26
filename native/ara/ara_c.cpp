// SPDX-License-Identifier: MIT
// ARA 2 for the CLAP entry (crates/openplugin-targets/src/clap.rs), as a small library the CLAP
// bundle carries (Contents/Resources/libOpenPluginARA.dylib) and loads at run time. The CLAP entry
// is Rust built by cargo with no C++; this library holds ara_glue.cpp and the ARA Library behind
// a C interface. What ara_glue.cpp asks the plugin (op_shell_info, op_ara_analyze, op_ara_url)
// comes from the callbacks the CLAP entry passes in op_ara_clap_init.
#if OP_HAS_ARA

#include <cstddef>
#include <cstring>
#include <new>
#include <string>
#include <vector>

#include "ara_glue.h"
#include "op_shell.h"

#define OP_ARA_EXPORT extern "C" __attribute__((visibility("default")))

namespace {
struct Callbacks {
    std::string name, vendor, version, id, url;
    void (*analyze)(const char*, uint32_t, uint64_t, double, const float* const*) = nullptr;
    void (*select)(const char* const*, uint32_t) = nullptr;
};
Callbacks& callbacks() {
    static Callbacks c;
    return c;
}

// One CLAP instance: its ARA binding and the buffers a renderer plays the regions into.
struct Ext {
    ARA::PlugIn::PlugInExtension ext;
};
}  // namespace

// What ara_glue.cpp calls, answered from the callbacks.
extern "C" uint32_t op_shell_info(OpShellInfo* out) {
    if (!out) return 0;
    std::memset(out, 0, sizeof(*out));
    const Callbacks& c = callbacks();
    std::strncpy(out->name, c.name.c_str(), sizeof(out->name) - 1);
    std::strncpy(out->vendor, c.vendor.c_str(), sizeof(out->vendor) - 1);
    std::strncpy(out->version, c.version.c_str(), sizeof(out->version) - 1);
    std::strncpy(out->id, c.id.c_str(), sizeof(out->id) - 1);
    return 1;
}
extern "C" void op_ara_analyze(const char* id, uint32_t channels, uint64_t frames, double rate, const float* const* data) {
    if (callbacks().analyze) callbacks().analyze(id, channels, frames, rate, data);
}
extern "C" const char* op_ara_url(void) { return callbacks().url.c_str(); }
extern "C" void op_ara_select(const char* const* ids, uint32_t count) {
    if (callbacks().select) callbacks().select(ids, count);
}

struct OpAraClapInit {
    uint32_t struct_size;
    const char* name;
    const char* vendor;
    const char* version;
    const char* id;
    const char* url;
    void (*analyze)(const char*, uint32_t, uint64_t, double, const float* const*);
    // Since the second version of this struct (struct_size tells): the selection, or NULL.
    void (*select)(const char* const*, uint32_t);
};

// Called once by the CLAP entry before anything else.
OP_ARA_EXPORT int op_ara_clap_init(const OpAraClapInit* init) {
    constexpr size_t kFirst = offsetof(OpAraClapInit, select);
    if (!init || init->struct_size < kFirst || !init->name || !init->id) return 0;
    Callbacks& c = callbacks();
    c.name = init->name;
    c.vendor = init->vendor ? init->vendor : "";
    c.version = init->version ? init->version : "";
    c.id = init->id;
    c.url = init->url ? init->url : "";
    c.analyze = init->analyze;
    c.select = init->struct_size >= sizeof(OpAraClapInit) ? init->select : nullptr;
    return 1;
}

OP_ARA_EXPORT const void* op_ara_clap_factory(void) { return openplugin_ara::factory(); }

OP_ARA_EXPORT void* op_ara_clap_ext_new(void) { return new (std::nothrow) Ext(); }

OP_ARA_EXPORT void op_ara_clap_ext_delete(void* ext) { delete static_cast<Ext*>(ext); }

OP_ARA_EXPORT const void* op_ara_clap_bind(void* ext, void* documentController, uint32_t knownRoles, uint32_t assignedRoles) {
    if (!ext) return nullptr;
    return static_cast<Ext*>(ext)->ext.bindToARA(static_cast<ARA::ARADocumentControllerRef>(documentController), knownRoles, assignedRoles);
}

// At activation: 1 if the instance is a playback renderer (its input comes from the regions).
OP_ARA_EXPORT int op_ara_clap_enable(void* ext, double sampleRate, int channels) {
    if (!ext) return 0;
    auto* renderer = static_cast<Ext*>(ext)->ext.getPlaybackRenderer<openplugin_ara::Renderer>();
    if (!renderer) return 0;
    renderer->enable(sampleRate, channels);
    return 1;
}

// Audio thread: 0 not a renderer (process the host input); 1 the regions were rendered into `out`,
// process them; 2 rendered, and this instance only renders (play them unprocessed).
OP_ARA_EXPORT int op_ara_clap_render(void* ext, float* const* out, int64_t position, int64_t frames, int playing) {
    if (!ext) return 0;
    auto& e = static_cast<Ext*>(ext)->ext;
    auto* renderer = e.getPlaybackRenderer<openplugin_ara::Renderer>();
    if (!renderer) return 0;
    renderer->render(out, position, frames, playing != 0);
    return openplugin_ara::renderer_only(e) ? 2 : 1;
}

#endif
