// SPDX-License-Identifier: MIT
// Configure-time probe, built and run by native/CMakeLists.txt against the plugin's
// library.
//
// Without arguments it prints the Audio Unit component type the plugin needs, from
// op_shell_kind (when OP_AU_TYPE is "auto"). An effect is aufx whatever its note ports
// say: the default note_ports() of a plugin has a MIDI input, so a plugin that wants to
// be a music effect (aumf) says so with -DOP_AU_TYPE=aumf.
//
// With the argument "codes" it prints the subtype and manufacturer the plugin pins
// (Plugin::au_ids, op_shell_au_code, ABI 8) as "pinned SUBT MANU", or "none".
#include "op_shell.h"
#include <stdio.h>
#include <string.h>
static void four(uint32_t code, char out[5]) {
    for (int i = 0; i < 4; ++i) out[i] = (char)(code >> (24 - 8 * i));
    out[4] = 0;
}
int main(int argc, char** argv) {
    if (argc > 1 && strcmp(argv[1], "codes") == 0) {
        uint32_t subtype = 0, manufacturer = 0;
        if (op_shell_abi_version() >= 8) {
            subtype = op_shell_au_code(OP_AU_CODE_SUBTYPE);
            manufacturer = op_shell_au_code(OP_AU_CODE_MANUFACTURER);
        }
        if (subtype == 0 || manufacturer == 0) {
            puts("none");
            return 0;
        }
        char s[5], m[5];
        four(subtype, s);
        four(manufacturer, m);
        printf("pinned %s %s\n", s, m);
        return 0;
    }
    if (op_shell_abi_version() < 2) {
        puts("aufx");
        return 0;
    }
    switch (op_shell_kind()) {
    case OP_KIND_INSTRUMENT: puts("aumu"); break;
    case OP_KIND_NOTE_EFFECT: puts("aumf"); break;
    default: puts("aufx"); break;
    }
    return 0;
}
