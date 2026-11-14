// SPDX-License-Identifier: MIT
// Writes the AAX page table for the plugin this tool is linked with: which
// parameter sits on which encoder of an Avid control surface. Built and run
// by the shell build when OP_AAX_PAGES is on.
//
//   OpenPluginAAXPages <out.xml>
//
// Parameters are laid out in their declared order. The page table names the
// plugin by its manufacturer, product and plug-in IDs as four characters each,
// so the IDs must be printable; the hashed development IDs are refused.
#include "op_shell.h"
#include <cstdio>
#include <string>
#include <vector>

namespace {
bool fourChars(uint32_t id, std::string& out) {
    out.clear();
    for (int shift = 24; shift >= 0; shift -= 8) {
        const char c = static_cast<char>((id >> shift) & 0xff);
        if (c < 0x21 || c > 0x7e || c == '\'' || c == '<' || c == '>' || c == '&') return false;
        out.push_back(c);
    }
    return true;
}
std::string escaped(const char* text) {
    std::string out;
    for (const char* p = text; *p; ++p) {
        switch (*p) {
            case '&': out += "&amp;"; break;
            case '<': out += "&lt;"; break;
            case '>': out += "&gt;"; break;
            case '\'': out += "&apos;"; break;
            case '"': out += "&quot;"; break;
            default: out.push_back(*p);
        }
    }
    return out;
}
// A surface with a grid of encoders. Cells are listed column by column.
struct Grid {
    const char* type;
    int rows;
    int cols;
};
const Grid kGrids[] = {{"Av18", 1, 8}, {"Av1F", 1, 16}, {"Av41", 4, 1}, {"Av46", 4, 6}, {"Av48", 4, 8}, {"Av81", 8, 1}};
const int kCategories[] = {0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048};
void firstPages(FILE* out) {
    for (int category : kCategories) std::fprintf(out, "\t\t\t\t<FirstPg cat='%d'>0</FirstPg>\n", category);
}
}

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: OpenPluginAAXPages <out.xml>\n");
        return 2;
    }
    OpShellInfo info{};
    if (!op_shell_info(&info) || info.abi != OP_SHELL_ABI || info.param_count == 0) {
        std::fprintf(stderr, "page table: the plugin did not describe itself\n");
        return 1;
    }
    std::string manufacturer, product, mono, stereo;
    if (!fourChars(info.manufacturer_id, manufacturer) || !fourChars(info.product_id, product) ||
        !fourChars(info.plugin_id_mono, mono) || !fourChars(info.plugin_id_stereo, stereo)) {
        std::fprintf(stderr, "page table: the AAX IDs are not four printable characters; set them in the plugin\n");
        return 1;
    }
    std::vector<std::string> ids;
    std::vector<std::string> discrete;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        if (!op_shell_param(i, &desc) || desc.id[0] == '\0') {
            std::fprintf(stderr, "page table: parameter %u has no id\n", i);
            return 1;
        }
        ids.push_back(escaped(desc.id));
        if ((desc.flags & OP_PARAM_DISCRETE) && desc.steps > 1) discrete.push_back(ids.back());
    }
    const std::string name = escaped(info.name);
    const std::string vendor = escaped(info.vendor);
    struct Variant {
        const std::string* id;
        const char* stems;
    };
    const Variant variants[] = {{&mono, "1 -&gt; 1"}, {&stereo, "2 -&gt; 2"}};

    FILE* out = std::fopen(argv[1], "w");
    if (!out) {
        std::fprintf(stderr, "page table: cannot write %s\n", argv[1]);
        return 1;
    }
    std::fprintf(out, "<?xml version='1.0' encoding='US-ASCII' standalone='yes'?>\n<PageTables vers='6.4.0.89'>\n\t<PageTableLayouts>\n");
    for (const Variant& v : variants) {
        std::fprintf(out, "\t\t<Plugin manID='%s' prodID='%s' plugID='%s'>\n\t\t\t<Desc>%s %s %s</Desc>\n\t\t\t<Layout>StandardLayout</Layout>\n\t\t</Plugin>\n",
                     manufacturer.c_str(), product.c_str(), v.id->c_str(), name.c_str(), v.stems, vendor.c_str());
    }
    std::fprintf(out, "\t\t<PTLayout name='StandardLayout'>\n");
    for (const Grid& grid : kGrids) {
        const int cells = grid.rows * grid.cols;
        std::fprintf(out, "\t\t\t<PageTable type='%s' pgsz='%d'>\n", grid.type, cells * 3);
        const int pages = static_cast<int>((ids.size() + cells - 1) / cells);
        for (int page = 0; page < pages; ++page) {
            std::fprintf(out, "\t\t\t\t<Page num='%d'>\n", page + 1);
            int cell = 0;
            for (int col = 1; col <= grid.cols; ++col) {
                for (int row = 1; row <= grid.rows; ++row, ++cell) {
                    const size_t index = static_cast<size_t>(page) * cells + cell;
                    std::fprintf(out, "\t\t\t\t\t<Cell row='%d' col='%d' knobID=\"%s\" inOutButtonID=\"\" selectButtonID=\"\"/>\n", row, col,
                                 index < ids.size() ? ids[index].c_str() : "");
                }
            }
            std::fprintf(out, "\t\t\t\t</Page>\n");
        }
        firstPages(out);
        std::fprintf(out, "\t\t\t</PageTable>\n");
    }
    // The generic table: one control a page, bypass first.
    std::fprintf(out, "\t\t\t<PageTable type='PgTL' pgsz='1'>\n\t\t\t\t<Page num='1'>\n\t\t\t\t\t<ID>MasterBypassID</ID>\n\t\t\t\t</Page>\n");
    for (size_t i = 0; i < ids.size(); ++i)
        std::fprintf(out, "\t\t\t\t<Page num='%zu'>\n\t\t\t\t\t<ID>%s</ID>\n\t\t\t\t</Page>\n", i + 2, ids[i].c_str());
    firstPages(out);
    std::fprintf(out, "\t\t\t</PageTable>\n\t\t</PTLayout>\n\t</PageTableLayouts>\n");
    std::fprintf(out, "\t<Editor vers='1.3.7.1'>\n\t\t<PluginList>\n\t\t\t<RTAS>\n");
    for (const Variant& v : variants) {
        std::fprintf(out, "\t\t\t\t<PluginID manID='%s' prodID='%s' plugID='%s'>\n\t\t\t\t\t<MenuStr>AAX Native: %s, %s</MenuStr>\n\t\t\t\t</PluginID>\n",
                     manufacturer.c_str(), product.c_str(), v.id->c_str(), name.c_str(), v.stems);
    }
    std::fprintf(out, "\t\t\t</RTAS>\n\t\t</PluginList>\n\t\t<DiscCtrls>\n\t\t\t<CtrlID>MasterBypassID</CtrlID>\n");
    for (const std::string& id : discrete) std::fprintf(out, "\t\t\t<CtrlID>%s</CtrlID>\n", id.c_str());
    std::fprintf(out, "\t\t</DiscCtrls>\n\t</Editor>\n</PageTables>\n");
    return std::fclose(out) == 0 ? 0 : 1;
}
