// SPDX-License-Identifier: MIT
// Windows stand-in for <unistd.h> in the VST3 proofs: getcwd, which the C runtime
// declares in <direct.h>. Proof builds only.
#pragma once
#include <direct.h>
#include <string.h>
