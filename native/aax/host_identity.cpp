// SPDX-License-Identifier: MIT
// The identity pinning proof (porting toolkit task 2) through the AAX shell. The
// plugin is Pinned (examples/pinned) in four builds:
//
//   Pinned A   pinned ids, parameters level, offset, invert
//   Pinned B   pinned ids, parameters invert, drive (new), offset, level
//   Plain A    the first table, no pins
//   Plain B    the second table, no pins
//
// An AAX build links one plugin library, so this program is built once per build of
// the plugin and the proof is two processes: one saves a project, the other opens it.
// The project is a text file with what Pro Tools keeps: the manufacturer, product
// and plug-in ids, the state chunk, the value of every parameter by its AAX
// parameter id, an automation lane (points that name a parameter by that id), and
// the audio the saving build made of the lane.
//
//   AAXIdentity<build> save <project>            write a project
//   AAXIdentity<build> open <project> same       the project must open: same ids, every
//                                                value on its own parameter, same audio
//   AAXIdentity<build> open <project> refused    the chunk must be refused, nothing changed
//
// native/aax/identity_proof.cmake runs them: Pinned A to Pinned B and back must
// open; Plain A to Plain B must be refused (the chunk of another parameter layout).
// It drives the shell's AAX_CEffectParameters without an AAX controller, like the
// other AAX proofs. Not a Pro Tools test.
#include "OpenPluginAAX.h"
#include "OpenPluginAAXState.h"
#include "AAX_CEffectParameters.h"
#include "AAX_CString.h"
#include "AAX_Errors.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <memory>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

using namespace openplugin_aax;

static void require(bool ok, const char* message) {
    if (!ok) {
        std::fprintf(stderr, "AAX identity proof failed: %s\n", message);
        std::exit(1);
    }
}

#ifndef OP_IDENTITY_BUILD
#define OP_IDENTITY_BUILD "this build"
#endif

namespace {
constexpr int kFrames = 64;
// Which build of the plugin this program was linked with ("Pinned A"): for the messages.
const char* const kBuild = OP_IDENTITY_BUILD;
using Audio = std::vector<float>;

struct Chunk {
    std::vector<unsigned char> storage;
    explicit Chunk(size_t dataBytes) : storage(sizeof(AAX_SPlugInChunk) + dataBytes + 8, 0) {}
    AAX_SPlugInChunk* get() { return reinterpret_cast<AAX_SPlugInChunk*>(storage.data()); }
    unsigned char* data() { return reinterpret_cast<unsigned char*>(get()->fData); }
};

// One automation point: the parameter as the host stored it, and the normalized value.
// AAX automation is not sample-accurate: a point is a value before a block.
struct LanePoint {
    std::string id;
    double value;
};

struct Project {
    uint32_t manufacturer = 0, product = 0, plugin = 0;
    std::vector<unsigned char> chunk;
    int32_t chunkVersion = 0;
    std::vector<std::pair<std::string, double>> values;
    std::vector<LanePoint> lane;
    Audio heard;
};

struct Instance {
    std::unique_ptr<AAX_CEffectParameters> params;
    ProcessState state;
    Instance() : params(createParameters()) {
        require(params && proofInit(params.get(), 48000.0, 2), "parameters build without a controller");
        require(state.prepare(48000.0, 2), "prepare");
    }
    // The plugin's own parameter ids, in the order the data model lists them (the master
    // bypass the shell adds is left out).
    std::vector<std::string> ids() const {
        int32_t count = 0;
        require(params->GetNumberOfParameters(&count) == AAX_SUCCESS, "GetNumberOfParameters");
        std::vector<std::string> list;
        for (int32_t i = 0; i < count; ++i) {
            AAX_CString id;
            require(params->GetParameterIDFromIndex(i, &id) == AAX_SUCCESS, "GetParameterIDFromIndex");
            if (std::strcmp(id.Get(), cDefaultMasterBypassID) != 0) list.push_back(id.Get());
        }
        return list;
    }
    bool has(const std::string& id) const {
        double value = 0.0;
        return params->GetParameterNormalizedValue(id.c_str(), &value) == AAX_SUCCESS;
    }
    double get(const std::string& id) const {
        double value = -1.0;
        require(params->GetParameterNormalizedValue(id.c_str(), &value) == AAX_SUCCESS, "GetParameterNormalizedValue");
        return value;
    }
    void set(const std::string& id, double normalized) {
        require(params->UpdateParameterNormalizedValue(id.c_str(), normalized, AAX_eUpdateSource_Unspecified) == AAX_SUCCESS, "UpdateParameterNormalizedValue");
    }
    std::string name(const std::string& id) const {
        AAX_CString text;
        require(params->GetParameterName(id.c_str(), &text) == AAX_SUCCESS, "GetParameterName");
        return text.Get();
    }
    // The id of the parameter the user sees as `name`.
    std::string idOf(const char* wanted) const {
        for (const std::string& id : ids())
            if (name(id) == wanted) return id;
        std::fprintf(stderr, "AAX identity proof: no parameter named %s\n", wanted);
        std::exit(1);
    }
    // One block of a constant input with the values the data model holds now; the left channel.
    Audio block(float input) {
        PacketBuffer packet;
        require(proofPacket(params.get(), &packet), "read packet");
        Audio inL(kFrames, input), inR(kFrames, input), outL(kFrames, 9.0f), outR(kFrames, 9.0f);
        float* ins[] = {inL.data(), inR.data()};
        float* outs[] = {outL.data(), outR.data()};
        int32_t frames = kFrames;
        auto* statePtr = &state;
        Context context{ins, outs, &frames, packet.get(), &statePtr};
        Context* contexts[] = {&context};
        process(contexts, contexts + 1);
        require(outL == outR, "both channels carry the same audio");
        return outL;
    }
    // The lane: each point, then a block.
    Audio play(float input, const std::vector<LanePoint>& lane) {
        Audio all;
        for (const LanePoint& point : lane) {
            // A point for a parameter this build does not have is skipped, as a host does.
            if (has(point.id)) set(point.id, point.value);
            const Audio out = block(input);
            all.insert(all.end(), out.begin(), out.end());
        }
        return all;
    }
    Chunk save() const {
        uint32_t size = 0;
        require(params->GetChunkSize(state::kChunkId, &size) == AAX_SUCCESS && size > 0, "GetChunkSize");
        Chunk chunk(size);
        chunk.get()->fChunkID = state::kChunkId;
        require(params->GetChunk(state::kChunkId, chunk.get()) == AAX_SUCCESS, "GetChunk");
        return chunk;
    }
};

std::string hexOf(const unsigned char* bytes, size_t count) {
    static const char digits[] = "0123456789abcdef";
    std::string text;
    for (size_t i = 0; i < count; ++i) {
        text.push_back(digits[bytes[i] >> 4]);
        text.push_back(digits[bytes[i] & 15]);
    }
    return text;
}
std::vector<unsigned char> bytesOf(const std::string& hex) {
    std::vector<unsigned char> bytes;
    for (size_t i = 0; i + 1 < hex.size(); i += 2) bytes.push_back(static_cast<unsigned char>(std::stoul(hex.substr(i, 2), nullptr, 16)));
    return bytes;
}
std::string hexOfAudio(const Audio& audio) { return hexOf(reinterpret_cast<const unsigned char*>(audio.data()), audio.size() * sizeof(float)); }

void write(const Project& p, const char* path) {
    std::ofstream out(path);
    out.precision(17);
    out << "ids " << p.manufacturer << ' ' << p.product << ' ' << p.plugin << '\n';
    out << "chunk " << p.chunkVersion << ' ' << hexOf(p.chunk.data(), p.chunk.size()) << '\n';
    for (const auto& [id, value] : p.values) out << "value " << id << ' ' << value << '\n';
    for (const LanePoint& point : p.lane) out << "point " << point.id << ' ' << point.value << '\n';
    out << "heard " << hexOfAudio(p.heard) << '\n';
    require(out.good(), "write the project");
}
Project read(const char* path) {
    std::ifstream in(path);
    require(in.good(), "read the project (was it saved?)");
    Project p;
    std::string line;
    while (std::getline(in, line)) {
        std::istringstream row(line);
        std::string kind;
        row >> kind;
        if (kind == "ids") row >> p.manufacturer >> p.product >> p.plugin;
        else if (kind == "chunk") {
            std::string hex;
            row >> p.chunkVersion >> hex;
            p.chunk = bytesOf(hex);
        } else if (kind == "value") {
            std::string id;
            double value = 0.0;
            row >> id >> value;
            p.values.push_back({id, value});
        } else if (kind == "point") {
            LanePoint point;
            row >> point.id >> point.value;
            p.lane.push_back(point);
        } else if (kind == "heard") {
            std::string hex;
            row >> hex;
            const auto bytes = bytesOf(hex);
            p.heard.resize(bytes.size() / sizeof(float));
            std::memcpy(p.heard.data(), bytes.data(), p.heard.size() * sizeof(float));
        }
    }
    require(!p.chunk.empty() && !p.values.empty() && !p.lane.empty() && !p.heard.empty(), "the project is whole");
    return p;
}

OpShellInfo shellInfo() {
    OpShellInfo info{};
    require(op_shell_info(&info) != 0 && info.param_count > 0, "shell info");
    return info;
}

int save(const char* path) {
    const OpShellInfo info = shellInfo();
    Instance host;
    // Normalized 0.25 of level (0..2) is 0.5, 0.625 of offset (-1..1) is 0.25.
    const std::string level = host.idOf("Level"), offset = host.idOf("Offset"), invert = host.idOf("Invert");
    host.set(level, 0.25);
    host.set(offset, 0.625);
    host.set(invert, 1.0);
    const bool hasDrive = host.ids().size() > 3;
    if (hasDrive) host.set(host.idOf("Drive"), 0.5);
    Project p;
    p.manufacturer = info.manufacturer_id;
    p.product = info.product_id;
    p.plugin = info.plugin_id_stereo;
    Chunk chunk = host.save();
    p.chunk.assign(chunk.data(), chunk.data() + chunk.get()->fSize);
    p.chunkVersion = chunk.get()->fVersion;
    for (const std::string& id : host.ids()) p.values.push_back({id, host.get(id)});
    // Level to 1.5, offset to -0.5, level to 0.25: a block after each.
    p.lane = {{level, 0.75}, {offset, 0.25}, {level, 0.125}};
    p.heard = host.play(0.5f, p.lane);
    // The audio is the plugin's formula on those values, exactly (drive 0.5 is a factor of 1.5).
    const float drive = hasDrive ? 1.5f : 1.0f;
    const float want[3] = {-(0.5f * 1.5f + 0.25f) * drive, -(0.5f * 1.5f - 0.5f) * drive, -(0.5f * 0.25f - 0.5f) * drive};
    for (size_t n = 0; n < p.heard.size(); ++n) require(p.heard[n] == want[n / kFrames], "the saving build plays its own lane");
    write(p, path);
    std::printf("AAX identity: %s saved a project: ids %08X %08X %08X, parameters", kBuild, p.manufacturer, p.product, p.plugin);
    for (const auto& [id, value] : p.values) std::printf(" %s=%g", id.c_str(), value);
    std::printf(", a %zu-point lane, chunk of %zu bytes\n", p.lane.size(), p.chunk.size());
    return 0;
}

int open(const char* path, bool mustOpen) {
    const OpShellInfo info = shellInfo();
    const Project p = read(path);
    Instance host;
    std::vector<std::pair<std::string, double>> before;
    for (const std::string& id : host.ids()) before.push_back({id, host.get(id)});
    Chunk chunk(p.chunk.size());
    chunk.get()->fChunkID = state::kChunkId;
    chunk.get()->fVersion = p.chunkVersion;
    chunk.get()->fSize = static_cast<int32_t>(p.chunk.size());
    std::memcpy(chunk.data(), p.chunk.data(), p.chunk.size());
    const AAX_Result result = host.params->SetChunk(state::kChunkId, chunk.get());
    if (!mustOpen) {
        require(result != AAX_SUCCESS, "the chunk of another parameter layout must be refused without pins");
        for (const auto& [id, value] : before) require(host.get(id) == value, "a refused chunk changes nothing");
        std::printf("AAX identity, negative control: %s refuses the project's chunk (another parameter layout, no pins) and changes nothing\n", kBuild);
        return 0;
    }
    // The ids Pro Tools finds the plug-in by.
    require(info.manufacturer_id == p.manufacturer && info.product_id == p.product && info.plugin_id_stereo == p.plugin, "the manufacturer, product and plug-in ids are the project's");
    require(result == AAX_SUCCESS, "the chunk opens");
    // Every saved value is on the parameter of the same id; a parameter the project
    // does not name has its default; a value for a parameter this build lacks is dropped.
    size_t shared = 0;
    for (const auto& [id, value] : p.values) {
        if (!host.has(id)) continue;
        require(host.get(id) == value, "a saved value is on its own parameter");
        ++shared;
    }
    require(shared >= 3, "level, offset and invert are in both builds");
    for (const auto& [id, value] : before) {
        bool named = false;
        for (const auto& saved : p.values) named = named || saved.first == id;
        if (!named) require(host.get(id) == value, "a parameter the project does not name keeps its default");
    }
    // The host's compare light: the call answers for a chunk of either layout.
    AAX_CBoolean equal = false;
    require(host.params->CompareActiveChunk(chunk.get(), &equal) == AAX_SUCCESS, "CompareActiveChunk");
    // The lane, point by point by parameter id.
    for (const LanePoint& point : p.lane) require(host.has(point.id), "every lane still has its parameter");
    const Audio heard = host.play(0.5f, p.lane);
    require(heard.size() == p.heard.size(), "the lane has the same length");
    // The project's audio includes the saving build's drive when it had one and this
    // build does not (or the reverse): the shared parameters must agree exactly.
    const bool savedDrive = p.values.size() > 3, hasDrive = host.ids().size() > 3;
    const float scale = savedDrive && !hasDrive ? 1.5f : 1.0f;
    for (size_t n = 0; n < heard.size(); ++n) require(heard[n] * scale == p.heard[n], "the lane sounds the same");
    // Saving again writes this build's own layout, which it reads back.
    Chunk again = host.save();
    Instance fresh;
    require(fresh.params->SetChunk(state::kChunkId, again.get()) == AAX_SUCCESS, "the build reads its own chunk back");
    std::printf("AAX identity: %s opens the project: same ids %08X %08X %08X, %zu saved values on their own parameters%s, the lane's audio identical over %zu samples\n",
                kBuild, p.manufacturer, p.product, p.plugin, shared, savedDrive && !hasDrive ? " (drive, which this build lacks, dropped)" : "", heard.size());
    return 0;
}
}

int main(int argc, char** argv) {
    if (argc == 3 && std::strcmp(argv[1], "save") == 0) return save(argv[2]);
    if (argc == 4 && std::strcmp(argv[1], "open") == 0 && (std::strcmp(argv[3], "same") == 0 || std::strcmp(argv[3], "refused") == 0))
        return open(argv[2], std::strcmp(argv[3], "same") == 0);
    std::fprintf(stderr, "usage: %s save <project> | open <project> same|refused\n", argv[0]);
    return 2;
}
