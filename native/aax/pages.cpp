// SPDX-License-Identifier: MIT
// Writes the AAX page table for the plugin this tool is linked with: which
// parameter sits on which encoder of an Avid control surface. Built and run
// by the shell build when OP_AAX_PAGES is on.
//
//   OpenPluginAAXPages <out.xml>
//
// The generic table (one control a page) lists the parameters in their
// declared order, as Avid requires. The control surface grids keep each
// parameter group on one page where it fits (op_aax::group_pages); without
// groups they are filled in declared order. The page table names the plugin
// by its manufacturer, product and plug-in IDs as four characters each, so
// the IDs must be printable; the hashed development IDs are refused.
//
// Built with -DOP_AAX_PAGES_NO_MAIN, the file is the writer alone, for the
// page table proof.
#include "op_shell.h"
#include "page_layout.h"
#include <cstdio>
#include <string>
#include <vector>

namespace op_aax {
std::vector<std::vector<int>> group_pages(const std::vector<std::string>& groups, int cells) {
    std::vector<std::vector<int>> pages;
    if (cells <= 0) return pages;
    // The groups in the order they first appear, each with its parameters.
    std::vector<std::string> names;
    std::vector<std::vector<int>> members;
    for (size_t i = 0; i < groups.size(); ++i) {
        size_t at = 0;
        while (at < names.size() && names[at] != groups[i]) ++at;
        if (at == names.size()) {
            names.push_back(groups[i]);
            members.emplace_back();
        }
        members[at].push_back(static_cast<int>(i));
    }
    int used = cells;  // cells taken on the last page; a full page means "start one"
    for (const std::vector<int>& group : members) {
        const int size = static_cast<int>(group.size());
        // Start a new page when the group would be split but need not be, or is longer than a page.
        if (used > 0 && used < cells && (size > cells || size > cells - used)) used = cells;
        for (int index : group) {
            if (used == cells) {
                pages.emplace_back(static_cast<size_t>(cells), -1);
                used = 0;
            }
            pages.back()[static_cast<size_t>(used++)] = index;
        }
    }
    return pages;
}
}  // namespace op_aax

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
// The group path of parameter `index` (ABI 5).
std::string groupOf(uint32_t index) {
    char small[128];
    const size_t length = op_shell_param_group(index, small, sizeof(small));
    if (length < sizeof(small)) return std::string(small, length);
    std::string big(length + 1, '\0');
    const size_t again = op_shell_param_group(index, big.data(), big.size());
    big.resize(again < length ? again : length);
    return big;
}
}  // namespace

int op_aax::write_page_table(const char* path) {
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
    std::vector<std::string> groups;
    std::vector<std::string> discrete;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        if (!op_shell_param(i, &desc) || desc.id[0] == '\0') {
            std::fprintf(stderr, "page table: parameter %u has no id\n", i);
            return 1;
        }
        // The ID Pro Tools knows the parameter by (ABI 8): the pin, or the string id.
        char aaxId[64] = {};
        if (op_shell_param_aax_id(i, aaxId, sizeof aaxId) == 0 || aaxId[0] == '\0') {
            std::fprintf(stderr, "page table: parameter %u has no AAX id\n", i);
            return 1;
        }
        ids.push_back(escaped(aaxId));
        groups.push_back(groupOf(i));
        if ((desc.flags & OP_PARAM_DISCRETE) && desc.steps > 1) discrete.push_back(ids.back());
    }
    const std::string name = escaped(info.name);
    const std::string vendor = escaped(info.vendor);
    struct Variant {
        const std::string* id;
        const char* stems;
    };
    const Variant variants[] = {{&mono, "1 -&gt; 1"}, {&stereo, "2 -&gt; 2"}};

    FILE* out = std::fopen(path, "w");
    if (!out) {
        std::fprintf(stderr, "page table: cannot write %s\n", path);
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
        const std::vector<std::vector<int>> pages = op_aax::group_pages(groups, cells);
        for (size_t page = 0; page < pages.size(); ++page) {
            std::fprintf(out, "\t\t\t\t<Page num='%zu'>\n", page + 1);
            size_t cell = 0;
            for (int col = 1; col <= grid.cols; ++col) {
                for (int row = 1; row <= grid.rows; ++row, ++cell) {
                    const int index = pages[page][cell];
                    std::fprintf(out, "\t\t\t\t\t<Cell row='%d' col='%d' knobID=\"%s\" inOutButtonID=\"\" selectButtonID=\"\"/>\n", row, col,
                                 index >= 0 ? ids[static_cast<size_t>(index)].c_str() : "");
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

#ifndef OP_AAX_PAGES_NO_MAIN
int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: OpenPluginAAXPages <out.xml>\n");
        return 2;
    }
    return op_aax::write_page_table(argv[1]);
}
#endif
