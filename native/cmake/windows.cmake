# SPDX-License-Identifier: MIT
# The native shells on Windows (MSVC). native/CMakeLists.txt includes this before its
# own project() when the build machine is Windows and then stops; nothing of the macOS
# configuration there is used (docs/PLATFORMS.md). This file holds what every Windows
# shell needs (the options, the Rust library and how to link it); each format has a
# file of its own next to its Windows sources (vst3/win/vst3.cmake, aax/win/aax.cmake).
# There is no Audio Unit on Windows.
#
# From the repository root, in PowerShell (the SDK stays outside this tree):
#
#   cargo build --release --lib -p example-editor -p example-role-tags -p example-expression-synth
#   cmake -S native -B build-shell -A x64 `
#     -DOP_RUST_LIB="$PWD\target\release\example_editor.lib" `
#     -DOP_VST3_SDK=C:\path\to\vst3sdk `
#     -DOP_AAX_SDK=C:\path\to\aax-sdk `
#     -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example `
#     "-DOP_VERSION=0.3.1" -DOP_MANUFACTURER=OpenGrid `
#     -DOP_TAIL=ON -DOP_EDITOR=OFF
#   cmake --build build-shell --config Release -j 2
#   ctest --test-dir build-shell -C Release --output-on-failure
#
# (PowerShell splits an unquoted -DOP_VERSION=0.3.1 at the first dot; quote it.)
# -DOP_BUILD_VST3=OFF or -DOP_BUILD_AAX=OFF leaves a format out, and its SDK is then
# not needed.
#
# The C and C++ runtime is linked statically (/MT), so no plug-in imports
# vcruntime140.dll or msvcp140.dll and none needs the Visual C++ Redistributable; the
# Universal CRT is part of Windows 10 and 11. The Rust library is `<name>.lib`, built
# for the MSVC target with +crt-static (.cargo/config.toml) against the release
# static runtime (Rust has no build against the debug one). So every configuration
# of the shells links the release static runtime too, Debug included: `--config
# Debug` gives unoptimized code with debug information and the SDKs' own checks,
# without the debug heap and the checked iterators of the debug runtime, and it
# links and runs with the same Rust library as Release. The bundles in the build folder are those
# of the configuration built last. OP_RUST_NATIVE_LIBS
# lists the system libraries the Rust library needs; take the list from
#   cargo rustc --release --lib -p <crate> --crate-type staticlib -- --print native-static-libs
# when a plugin needs more.
project(OpenPluginShell LANGUAGES C CXX RC)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT MSVC)
  message(FATAL_ERROR "The Windows shells are built with MSVC (Visual Studio or its Build Tools)")
endif()
# The release C runtime, linked statically (/MT), in every configuration: the Rust
# library brings the release static runtime (+crt-static), and one image cannot mix
# it with the debug one (/MTd). The VST3 and AAX SDK libraries get the same
# (vst3/win/vst3.cmake, aax/win/aax.cmake). scripts/windows/check_imports.ps1 fails
# on a binary that imports vcruntime*, msvcp* or concrt*.
set(OP_MSVC_RUNTIME "MultiThreaded")
set(CMAKE_MSVC_RUNTIME_LIBRARY "${OP_MSVC_RUNTIME}")
get_property(OP_MULTI_CONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(NOT OP_MULTI_CONFIG AND NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

set(OP_NATIVE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")

set(OP_RUST_LIB "" CACHE FILEPATH "Rust static library exporting op_shell_*")
set(OP_VST3_SDK "" CACHE PATH "Existing VST3 SDK checkout")
set(OP_AAX_SDK "" CACHE PATH "Existing AAX SDK checkout")
set(OP_PRODUCT "Plugin" CACHE STRING "Module file and display name")
set(OP_BUNDLE_ID "com.example.openplugin" CACHE STRING "Bundle identifier prefix (not used on Windows)")
set(OP_VERSION "0.1.0" CACHE STRING "Version")
set(OP_MANUFACTURER "OpenPlugin" CACHE STRING "Manufacturer display name")
set(OP_RUST_NATIVE_LIBS "user32;gdi32;kernel32;ntdll;userenv;ws2_32;dbghelp;synchronization;bcrypt;advapi32;shell32;comdlg32;comctl32;oleaut32;uiautomationcore"
  CACHE STRING "System libraries the Rust static library needs (rustc --print native-static-libs)")
option(OP_BUILD_VST3 "Build the VST3 shell" ON)
option(OP_BUILD_AAX "Build the unsigned AAX shell" ON)
option(OP_EDITOR "The plugin exports op_editor_* (include/op_editor.h); host its view" OFF)
option(OP_TAIL "The plugin exports op_shell_tail_seconds; report its tail to the host" OFF)
option(OP_AAX_PAGES "Write an AAX page table into the bundle; needs four-character AAX IDs" OFF)

if(NOT EXISTS "${OP_RUST_LIB}")
  message(FATAL_ERROR "OP_RUST_LIB must be the built static library (target\\release\\<name>.lib)")
endif()

# Role Tags (examples/role_tags) for the channel order proofs, found next to OP_RUST_LIB.
set(OP_ROLE_LIB "" CACHE FILEPATH "Rust static library of examples/role_tags, for the role proofs")
if(NOT OP_ROLE_LIB)
  get_filename_component(OP_RUST_LIB_DIR "${OP_RUST_LIB}" DIRECTORY)
  if(EXISTS "${OP_RUST_LIB_DIR}/example_role_tags.lib")
    set(OP_ROLE_LIB "${OP_RUST_LIB_DIR}/example_role_tags.lib")
  endif()
endif()
if(OP_ROLE_LIB AND EXISTS "${OP_ROLE_LIB}")
  set(OP_ROLE_PROOFS ON)
  message(STATUS "Role proofs over ${OP_ROLE_LIB}")
else()
  set(OP_ROLE_PROOFS OFF)
  message(STATUS "Role proofs not built: no example_role_tags.lib (cargo build --release --lib -p example-role-tags)")
endif()

# Expression Synth (examples/expression_synth) for the per-note expression proof,
# found next to OP_RUST_LIB.
set(OP_EXPRESSION_LIB "" CACHE FILEPATH "Rust static library of examples/expression_synth, for the expression proof")
if(NOT OP_EXPRESSION_LIB)
  get_filename_component(OP_RUST_LIB_DIR "${OP_RUST_LIB}" DIRECTORY)
  if(EXISTS "${OP_RUST_LIB_DIR}/example_expression_synth.lib")
    set(OP_EXPRESSION_LIB "${OP_RUST_LIB_DIR}/example_expression_synth.lib")
  endif()
endif()
if(OP_EXPRESSION_LIB AND EXISTS "${OP_EXPRESSION_LIB}")
  set(OP_EXPRESSION_PROOFS ON)
  message(STATUS "Expression proof over ${OP_EXPRESSION_LIB}")
else()
  set(OP_EXPRESSION_PROOFS OFF)
  message(STATUS "Expression proof not built: no example_expression_synth.lib (cargo build --release --lib -p example-expression-synth)")
endif()

# Links the Rust library: OP_RUST_LIB, or the library given as the second argument.
function(op_link_rust target_name)
  set(rust_lib "${OP_RUST_LIB}")
  if(ARGC GREATER 1)
    set(rust_lib "${ARGV1}")
  endif()
  target_link_libraries(${target_name} PRIVATE "${rust_lib}" ${OP_RUST_NATIVE_LIBS})
endfunction()

# The version resource of a plug-in binary (native/win/version.rc.in): the product,
# OP_VERSION and OP_MANUFACTURER, with `description` as the file description and
# `file_name` as the original file name.
function(op_version_resource target_name product description file_name)
  set(numbers 0 0 0)
  if(OP_VERSION MATCHES "^([0-9]+)\\.([0-9]+)\\.([0-9]+)")
    set(numbers ${CMAKE_MATCH_1} ${CMAKE_MATCH_2} ${CMAKE_MATCH_3})
  elseif(OP_VERSION MATCHES "^([0-9]+)\\.([0-9]+)")
    set(numbers ${CMAKE_MATCH_1} ${CMAKE_MATCH_2} 0)
  elseif(OP_VERSION MATCHES "^([0-9]+)")
    set(numbers ${CMAKE_MATCH_1} 0 0)
  endif()
  list(JOIN numbers "," OP_RC_VERSION_COMMAS)
  set(OP_RC_VERSION_COMMAS "${OP_RC_VERSION_COMMAS},0")
  # A resource string: a quote is doubled, a backslash escaped.
  foreach(pair IN ITEMS "OP_RC_VERSION|${OP_VERSION}" "OP_RC_COMPANY|${OP_MANUFACTURER}" "OP_RC_PRODUCT|${product}"
      "OP_RC_DESCRIPTION|${description}" "OP_RC_FILE|${file_name}")
    string(FIND "${pair}" "|" bar)
    string(SUBSTRING "${pair}" 0 ${bar} name)
    math(EXPR after "${bar} + 1")
    string(SUBSTRING "${pair}" ${after} -1 text)
    string(REPLACE "\\" "\\\\" text "${text}")
    string(REPLACE "\"" "\"\"" text "${text}")
    set(${name} "${text}")
  endforeach()
  set(rc "${CMAKE_BINARY_DIR}/version/${target_name}.rc")
  configure_file("${OP_NATIVE_DIR}/win/version.rc.in" "${rc}" @ONLY)
  target_sources(${target_name} PRIVATE "${rc}")
endfunction()

# Folders taken off the front of __FILE__ in the C++ code (an assert in an SDK header
# carries its path), so a shipped binary names no folder of the build machine.
# scripts/windows/build_plugin.ps1 gives the home, SDK and build folders.
set(OP_TRIM_PATHS "" CACHE STRING "Path prefixes to cut from __FILE__ (a ;-list), for shipped builds")
foreach(prefix IN LISTS OP_TRIM_PATHS)
  file(TO_NATIVE_PATH "${prefix}" prefix)
  add_compile_options("/d1trimfile:${prefix}\\")
endforeach()

enable_testing()

if(OP_BUILD_VST3)
  include("${OP_NATIVE_DIR}/vst3/win/vst3.cmake")
endif()
if(OP_BUILD_AAX)
  include("${OP_NATIVE_DIR}/aax/win/aax.cmake")
endif()
