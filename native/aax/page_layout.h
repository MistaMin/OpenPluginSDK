// SPDX-License-Identifier: MIT
// Which parameter sits on which encoder of an AAX page table (pages.cpp), and
// the writer itself, for the page table proof (host_pages.cpp).
#pragma once
#include <string>
#include <vector>

namespace op_aax {
// Pages of `cells` encoders for parameters whose group paths are `groups`
// (groups[i] is parameter i's, "" for the top level). An entry is a
// parameter index, or -1 for an empty cell.
//
// The parameters of one group stay together: groups are laid out in the
// order they first appear, each group's parameters in their declared order.
// A group that would not fit in what is left of a page starts the next page
// (leaving cells empty) when it fits on one page; a group longer than a page
// starts on a fresh page and runs on. A plugin without groups is one group:
// its parameters fill the pages in declared order, as before groups existed.
std::vector<std::vector<int>> group_pages(const std::vector<std::string>& groups, int cells);

// Writes the page table XML for the plugin linked in (op_shell_*) to `path`.
// Returns 0, or 1 with a message on stderr.
int write_page_table(const char* path);
}  // namespace op_aax
