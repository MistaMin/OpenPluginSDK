# SPDX-License-Identifier: MIT
# The native shells on Linux (GCC or Clang). native/CMakeLists.txt includes this before
# its own project() when the build machine is Linux and then stops, as it does for
# Windows (cmake/windows.cmake). Linux has one shell, VST3 (vst3/linux/vst3.cmake):
# CLAP and LV2 are the Rust library itself (scripts/build_plugin_linux.sh), and there is
# no Audio Unit or AAX on Linux.
#
#   cargo build --release --lib -p example-editor
#   cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
#     -DOP_RUST_LIB="$PWD/target/release/libexample_editor.a" \
#     -DOP_VST3_SDK=/path/to/vst3sdk \
#     -DOP_PRODUCT="Example Gain" -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
#     -DOP_TAIL=ON -DOP_EDITOR=ON
#   cmake --build build-shell -j 2
#
# The module exports only the VST3 entry points (a version script): the Rust library's
# op_shell_* and op_editor_* stay inside it, so two plug-ins built with this SDK in one
# host process never call into each other.
project(OpenPluginShell LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
# The VST3 SDK's static libraries end up in a shared module.
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
option(OP_EDITOR "The plugin exports op_editor_* (include/op_editor.h, op_editor_x11.h); host its view" OFF)
option(OP_TAIL "The plugin exports op_shell_tail_seconds; report its tail to the host" OFF)
set(OP_TRIM_PATHS "" CACHE STRING "Path prefixes to cut from __FILE__ (a ;-list), for shipped builds")

if(NOT EXISTS "${OP_RUST_LIB}")
  message(FATAL_ERROR "OP_RUST_LIB must be the built static library (target/release/lib<name>.a)")
endif()

function(op_link_rust target_name)
  target_link_libraries(${target_name} PRIVATE "${OP_RUST_LIB}" ${OP_RUST_NATIVE_LIBS})
endfunction()

# No folder of the build machine in a shipped binary (an assert in an SDK header
# carries its path).
foreach(prefix IN LISTS OP_TRIM_PATHS)
  add_compile_options("-ffile-prefix-map=${prefix}/=")
endforeach()

if(OP_BUILD_VST3)
  include("${OP_NATIVE_DIR}/vst3/linux/vst3.cmake")
endif()
