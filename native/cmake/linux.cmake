# SPDX-License-Identifier: MIT
# The native shell on Linux (GCC or Clang). native/CMakeLists.txt includes this before
# its own project() when the build machine is Linux and then stops; nothing of the macOS
# configuration there is used (docs/PLATFORMS.md). This file holds what the Linux shell
# needs (the options, the Rust library and how to link it); the format has a file of its
# own next to its Linux sources (vst3/linux/vst3.cmake). VST3 is the one format with a
# native shell on Linux: CLAP and LV2 are Rust alone, and there is no Audio Unit or AAX.
#
# From the repository root (the SDK stays outside this tree):
#
#   cargo build --release --lib -p example-editor -p example-role-tags -p example-expression-synth
#   cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
#     -DOP_RUST_LIB="$PWD/target/release/libexample_editor.a" \
#     -DOP_VST3_SDK=/path/to/vst3sdk \
#     -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
#     -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
#     -DOP_TAIL=ON -DOP_EDITOR=OFF
#   cmake --build build-shell -j 2
#   ctest --test-dir build-shell --output-on-failure
#
# OP_RUST_NATIVE_LIBS lists the system libraries the Rust library needs; take the list
# from
#   cargo rustc --release --lib -p <crate> --crate-type staticlib -- --print native-static-libs
# when a plugin needs more.
project(OpenPluginShell LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

set(OP_NATIVE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")

set(OP_RUST_LIB "" CACHE FILEPATH "Rust static library exporting op_shell_*")
set(OP_VST3_SDK "" CACHE PATH "Existing VST3 SDK checkout")
set(OP_PRODUCT "Plugin" CACHE STRING "Module file and display name")
set(OP_BUNDLE_ID "com.example.openplugin" CACHE STRING "Bundle identifier prefix (not used on Linux)")
set(OP_VERSION "0.1.0" CACHE STRING "Version")
set(OP_MANUFACTURER "OpenPlugin" CACHE STRING "Manufacturer display name")
set(OP_RUST_NATIVE_LIBS "gcc_s;util;rt;pthread;m;dl;c"
  CACHE STRING "System libraries the Rust static library needs (rustc --print native-static-libs)")
option(OP_BUILD_VST3 "Build the VST3 shell" ON)
option(OP_EDITOR "The plugin exports op_editor_* (include/op_editor.h); host its view" OFF)
option(OP_TAIL "The plugin exports op_shell_tail_seconds; report its tail to the host" OFF)
# A module that carries its own C++ runtime loads in a host whose libstdc++ is older
# than the build machine's, and next to plug-ins built against another one.
option(OP_STATIC_CXX "Link the C++ runtime (libstdc++, libgcc) into the module" ON)

if(NOT EXISTS "${OP_RUST_LIB}")
  message(FATAL_ERROR "OP_RUST_LIB must be the built static library (target/release/lib<name>.a)")
endif()

# Role Tags (examples/role_tags) for the channel order proof, found next to OP_RUST_LIB.
set(OP_ROLE_LIB "" CACHE FILEPATH "Rust static library of examples/role_tags, for the role proof")
if(NOT OP_ROLE_LIB)
  get_filename_component(OP_RUST_LIB_DIR "${OP_RUST_LIB}" DIRECTORY)
  if(EXISTS "${OP_RUST_LIB_DIR}/libexample_role_tags.a")
    set(OP_ROLE_LIB "${OP_RUST_LIB_DIR}/libexample_role_tags.a")
  endif()
endif()
if(OP_ROLE_LIB AND EXISTS "${OP_ROLE_LIB}")
  set(OP_ROLE_PROOFS ON)
  message(STATUS "Role proof over ${OP_ROLE_LIB}")
else()
  set(OP_ROLE_PROOFS OFF)
  message(STATUS "Role proof not built: no libexample_role_tags.a (cargo build --release --lib -p example-role-tags)")
endif()

# Expression Synth (examples/expression_synth) for the per-note expression proof,
# found next to OP_RUST_LIB.
set(OP_EXPRESSION_LIB "" CACHE FILEPATH "Rust static library of examples/expression_synth, for the expression proof")
if(NOT OP_EXPRESSION_LIB)
  get_filename_component(OP_RUST_LIB_DIR "${OP_RUST_LIB}" DIRECTORY)
  if(EXISTS "${OP_RUST_LIB_DIR}/libexample_expression_synth.a")
    set(OP_EXPRESSION_LIB "${OP_RUST_LIB_DIR}/libexample_expression_synth.a")
  endif()
endif()
if(OP_EXPRESSION_LIB AND EXISTS "${OP_EXPRESSION_LIB}")
  set(OP_EXPRESSION_PROOFS ON)
  message(STATUS "Expression proof over ${OP_EXPRESSION_LIB}")
else()
  set(OP_EXPRESSION_PROOFS OFF)
  message(STATUS "Expression proof not built: no libexample_expression_synth.a (cargo build --release --lib -p example-expression-synth)")
endif()

# Links the Rust library: OP_RUST_LIB, or the archive given as the second argument.
function(op_link_rust target_name)
  set(rust_lib "${OP_RUST_LIB}")
  if(ARGC GREATER 1)
    set(rust_lib "${ARGV1}")
  endif()
  target_link_libraries(${target_name} PRIVATE "${rust_lib}" ${OP_RUST_NATIVE_LIBS})
endfunction()

# Folders taken off the front of __FILE__ in the C++ code (an assert in an SDK header
# carries its path), so a shipped binary names no folder of the build machine.
# scripts/build_plugin_linux.sh gives the home, SDK and build folders.
set(OP_TRIM_PATHS "" CACHE STRING "Path prefixes to cut from __FILE__ (a ;-list), for shipped builds")
foreach(prefix IN LISTS OP_TRIM_PATHS)
  add_compile_options("-ffile-prefix-map=${prefix}/=")
endforeach()

enable_testing()

if(OP_BUILD_VST3)
  include("${OP_NATIVE_DIR}/vst3/linux/vst3.cmake")
endif()
