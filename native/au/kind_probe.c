// SPDX-License-Identifier: MIT
// Configure-time probe: prints the Audio Unit component type the plugin needs, from
// op_shell_kind. Built and run by native/CMakeLists.txt when OP_AU_TYPE is "auto".
// An effect is aufx whatever its note ports say: the default note_ports() of a plugin
// has a MIDI input, so a plugin that wants to be a music effect (aumf) says so with
// -DOP_AU_TYPE=aumf.
#include "op_shell.h"
#include <stdio.h>
int main(void) {
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
