// SPDX-License-Identifier: MIT
// Parameter groups in the AAX page table (pages.cpp, written when the shell is
// built with OP_AAX_PAGES). Not a Pro Tools test: Pro Tools reads the table
// only for control surfaces, and without one it has nowhere to show a group.
//
//   AAXPagesProof <scratch.xml>
//
// 1. op_aax::group_pages on made-up parameter lists: without groups the
//    encoders fill in declared order (the table as it was before groups);
//    a group that fits on one page is never split; a group longer than a page
//    starts on a fresh one; scattered members of a group are gathered.
// 2. The table for the linked plugin is written and read back: every grid
//    lists the knobs group_pages gives for the plugin's groups (from
//    op_shell_param_group), no group that fits a page is split, and the
//    generic one-control table keeps the declared order after Master Bypass.
#include "op_shell.h"
#include "page_layout.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#define CHECK(cond) do { if (!(cond)) { std::fprintf(stderr, "AAX pages proof failed line %d: %s\n", __LINE__, #cond); std::exit(1); } } while (0)

namespace {
using Pages = std::vector<std::vector<int>>;

void layouts() {
    using op_aax::group_pages;
    // No groups: declared order, the last page padded.
    const std::vector<std::string> flat(10, "");
    CHECK((group_pages(flat, 8) == Pages{{0, 1, 2, 3, 4, 5, 6, 7}, {8, 9, -1, -1, -1, -1, -1, -1}}));
    CHECK((group_pages(flat, 4) == Pages{{0, 1, 2, 3}, {4, 5, 6, 7}, {8, 9, -1, -1}}));
    CHECK(group_pages({}, 8).empty());
    // Three bands of two on five encoders: the third band moves to page 2 rather than split.
    CHECK((group_pages({"Low", "Low", "Mid", "Mid", "High", "High"}, 5) == Pages{{0, 1, 2, 3, -1}, {4, 5, -1, -1, -1}}));
    // Members of a group are gathered, in declared order, groups in order of first appearance.
    CHECK((group_pages({"A", "B", "A", "", "B"}, 8) == Pages{{0, 2, 1, 4, 3, -1, -1, -1}}));
    // A nested group is its own group.
    CHECK((group_pages({"Filter", "Filter/Env", "Filter", "Filter/Env"}, 2) == Pages{{0, 2}, {1, 3}}));
    // Longer than a page: starts on a fresh page and runs on.
    CHECK((group_pages({"X", "Y", "Y", "Y", "Y", "Y"}, 4) == Pages{{0, -1, -1, -1}, {1, 2, 3, 4}, {5, -1, -1, -1}}));
    // A group that exactly fills what is left stays on the page.
    CHECK((group_pages({"A", "A", "B", "B"}, 4) == Pages{{0, 1, 2, 3}}));
    std::puts("AAX pages: groups stay on one page where they fit; without groups the declared order");
}

std::string abi_group(uint32_t index) {
    char buf[512];
    const size_t length = op_shell_param_group(index, buf, sizeof(buf));
    CHECK(length < sizeof(buf));
    return std::string(buf, length);
}

// The knobID of every cell of every page of the PageTable of `type`, in file order ("" empty).
std::vector<std::vector<std::string>> grid_of(const std::string& xml, const std::string& type) {
    std::vector<std::vector<std::string>> pages;
    const size_t start = xml.find("<PageTable type='" + type + "'");
    CHECK(start != std::string::npos);
    const size_t end = xml.find("</PageTable>", start);
    CHECK(end != std::string::npos);
    const std::string table = xml.substr(start, end - start);
    for (size_t at = table.find("<Page num="); at != std::string::npos; at = table.find("<Page num=", at + 1)) {
        pages.emplace_back();
        const size_t close = table.find("</Page>", at);
        CHECK(close != std::string::npos);
        for (size_t cell = table.find("knobID=\"", at); cell != std::string::npos && cell < close; cell = table.find("knobID=\"", cell + 1)) {
            const size_t from = cell + std::strlen("knobID=\"");
            pages.back().push_back(table.substr(from, table.find('"', from) - from));
        }
    }
    return pages;
}

// The <ID> of every page of the generic table, in order.
std::vector<std::string> generic_of(const std::string& xml) {
    std::vector<std::string> ids;
    const size_t start = xml.find("<PageTable type='PgTL' pgsz='1'>");
    CHECK(start != std::string::npos);
    const size_t end = xml.find("</PageTable>", start);
    for (size_t at = xml.find("<ID>", start); at != std::string::npos && at < end; at = xml.find("<ID>", at + 1)) {
        const size_t from = at + 4;
        ids.push_back(xml.substr(from, xml.find("</ID>", from) - from));
    }
    return ids;
}

void table(const char* path) {
    OpShellInfo info{};
    CHECK(op_shell_info(&info) != 0 && info.abi >= OP_SHELL_ABI);
    std::vector<std::string> ids;
    std::vector<std::string> groups;
    size_t grouped = 0;
    for (uint32_t i = 0; i < info.param_count; ++i) {
        OpParamDesc desc{};
        CHECK(op_shell_param(i, &desc) != 0);
        ids.emplace_back(desc.id);
        groups.push_back(abi_group(i));
        if (!groups.back().empty()) ++grouped;
    }
    if (op_aax::write_page_table(path) != 0) {
        // The hashed development IDs cannot name a page table; the plugin sets none.
        std::puts("AAX pages: the linked plugin has no four-character AAX IDs, so no table to read back (layouts only)");
        return;
    }
    std::ifstream file(path);
    CHECK(file);
    std::stringstream text;
    text << file.rdbuf();
    const std::string xml = text.str();
    struct Grid {
        const char* type;
        int cells;
    };
    for (const Grid& grid : {Grid{"Av18", 8}, Grid{"Av1F", 16}, Grid{"Av41", 4}, Grid{"Av46", 24}, Grid{"Av48", 32}, Grid{"Av81", 8}}) {
        const auto read = grid_of(xml, grid.type);
        const Pages want = op_aax::group_pages(groups, grid.cells);
        CHECK(read.size() == want.size());
        for (size_t page = 0; page < want.size(); ++page) {
            CHECK(read[page].size() == static_cast<size_t>(grid.cells));
            for (int cell = 0; cell < grid.cells; ++cell) {
                const int index = want[page][static_cast<size_t>(cell)];
                CHECK(read[page][static_cast<size_t>(cell)] == (index >= 0 ? ids[static_cast<size_t>(index)] : std::string()));
            }
        }
        // Read back: each parameter once, and a group that fits on a page sits on one page.
        for (size_t i = 0; i < ids.size(); ++i) {
            size_t count = 0;
            for (const auto& page : read)
                for (const std::string& knob : page) count += knob == ids[i];
            CHECK(count == 1);
        }
        for (const std::string& group : groups) {
            size_t members = 0;
            for (const std::string& other : groups) members += other == group;
            if (members > static_cast<size_t>(grid.cells)) continue;
            size_t pagesWith = 0;
            for (const auto& page : read) {
                bool has = false;
                for (const std::string& knob : page)
                    for (size_t i = 0; i < ids.size(); ++i) has = has || (knob == ids[i] && groups[i] == group);
                pagesWith += has;
            }
            CHECK(pagesWith == 1);
        }
        std::printf("AAX pages: %s, %zu page(s) read back\n", grid.type, read.size());
    }
    const std::vector<std::string> generic = generic_of(xml);
    CHECK(generic.size() == ids.size() + 1);
    CHECK(generic[0] == "MasterBypassID");
    for (size_t i = 0; i < ids.size(); ++i) CHECK(generic[i + 1] == ids[i]);
    std::printf("AAX pages: %u parameters, %zu in groups; the generic table keeps the declared order\n", info.param_count, grouped);
}
}  // namespace

int main(int argc, char** argv) {
    CHECK(argc == 2);
    layouts();
    table(argv[1]);
    std::puts("AAX pages proof: PASS");
    return 0;
}
