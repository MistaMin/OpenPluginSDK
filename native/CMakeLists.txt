# SPDX-License-Identifier: MIT
# Thin VST3, Audio Unit, and AAX shells over a Rust staticlib that exports op_shell_*.
# Does not sign, install, or copy an SDK into this tree.
#
# The proofs load the SDK example (examples/editor, "Example Gain"). Build and
# run them from the repository root; the SDK checkouts stay outside this tree:
#
#   CARGO_BUILD_JOBS=2 cargo build --release --lib -p example-editor -p example-role-tags
#   cmake -S native -B build-shell -DCMAKE_BUILD_TYPE=Release \
#     -DOP_RUST_LIB="$PWD/target/release/libexample_editor.a" \
#     -DOP_VST3_SDK=/path/to/vst3sdk \
#     -DOP_AU_SDK=/path/to/AudioUnitSDK \
#     -DOP_AAX_SDK=/path/to/aax-sdk \
#     -DOP_PRODUCT="Example Gain" -DOP_BUNDLE_ID=com.opengrid.example \
#     -DOP_VERSION=0.3.1 -DOP_MANUFACTURER=OpenGrid \
#     -DOP_AU_SUBTYPE=ExGn -DOP_AU_MANUFACTURER=OpGd \
#     -DOP_TAIL=ON -DOP_EDITOR=OFF
#   cmake --build build-shell -j 2
#   ctest --test-dir build-shell --output-on-failure
#
# OP_TAIL=ON is what makes vst3_proof and au_proof exercise the tail report.
# The channel order proofs (vst3_roles_proof, au_roles_proof, aax_roles_proof) load
# Role Tags (examples/role_tags) instead; without its library they are not built.
# The per-note expression proof (vst3_expression_proof) loads Expression Synth
# (examples/expression_synth): cargo build --release --lib -p example-expression-synth.
cmake_minimum_required(VERSION 3.20)
# Windows has its own configuration, in its own files (docs/PLATFORMS.md): MSVC has no
# Objective-C++, and nothing below this block is used there.
if(CMAKE_HOST_WIN32)
  include("${CMAKE_CURRENT_LIST_DIR}/cmake/windows.cmake")
  return()
endif()
project(OpenPluginShell LANGUAGES C CXX OBJCXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

set(OP_RUST_LIB "" CACHE FILEPATH "Rust static library exporting op_shell_*")
set(OP_VST3_SDK "" CACHE PATH "Existing VST3 SDK checkout")
set(OP_AU_SDK "" CACHE PATH "Existing AudioUnitSDK checkout")
set(OP_AAX_SDK "" CACHE PATH "Existing AAX SDK checkout")
set(OP_PRODUCT "Plugin" CACHE STRING "Bundle executable and display name")
set(OP_BUNDLE_ID "com.example.openplugin" CACHE STRING "Bundle identifier prefix")
set(OP_VERSION "0.1.0" CACHE STRING "Bundle version")
set(OP_MANUFACTURER "OpenPlugin" CACHE STRING "Audio Unit manufacturer display name")
set(OP_AU_SUBTYPE "OPSh" CACHE STRING "Audio Unit subtype, four characters")
set(OP_AU_MANUFACTURER "OPSh" CACHE STRING "Audio Unit manufacturer code, four characters")
option(OP_BUILD_VST3 "Build the VST3 shell" ON)
option(OP_BUILD_AU "Build the Audio Unit shell" ON)
option(OP_BUILD_AAX "Build the unsigned AAX shell" ON)
option(OP_EDITOR "The plugin exports op_editor_* (include/op_editor.h); host its view" OFF)
option(OP_TAIL "The plugin exports op_shell_tail_seconds; report its tail to the host" OFF)
option(OP_AAX_PAGES "Write an AAX page table into the bundle; needs four-character AAX IDs" OFF)
if(OP_TAIL)
  add_compile_definitions(OP_HAS_TAIL=1)
else()
  add_compile_definitions(OP_HAS_TAIL=0)
endif()
if(OP_EDITOR)
  add_compile_definitions(OP_HAS_EDITOR=1)
  # The shells use the size range and resize calls of op_editor.h.
  add_compile_definitions(OP_EDITOR_RESIZE=1)
  # Objective-C class names are global in the host process.
  string(MAKE_C_IDENTIFIER "${OP_PRODUCT}_${OP_VERSION}" OP_CLASS_SUFFIX)
else()
  add_compile_definitions(OP_HAS_EDITOR=0)
endif()

if(NOT EXISTS "${OP_RUST_LIB}")
  message(FATAL_ERROR "OP_RUST_LIB must be the built static library")
endif()

# The channel order proof (0.6 task 8) links Role Tags (examples/role_tags) in place of
# the example: build it with `cargo build --release --lib -p example-role-tags`. When
# OP_ROLE_LIB is not given, libexample_role_tags.a next to OP_RUST_LIB is used if it
# is there; without it the role proofs are not built (and say so).
set(OP_ROLE_LIB "" CACHE FILEPATH "Rust static library of examples/role_tags, for the role proofs")
if(NOT OP_ROLE_LIB)
  get_filename_component(OP_RUST_LIB_DIR "${OP_RUST_LIB}" DIRECTORY)
  if(EXISTS "${OP_RUST_LIB_DIR}/libexample_role_tags.a")
    set(OP_ROLE_LIB "${OP_RUST_LIB_DIR}/libexample_role_tags.a")
  endif()
endif()
if(OP_ROLE_LIB AND EXISTS "${OP_ROLE_LIB}")
  set(OP_ROLE_PROOFS ON)
  message(STATUS "Role proofs over ${OP_ROLE_LIB}")
else()
  set(OP_ROLE_PROOFS OFF)
  message(STATUS "Role proofs not built: no libexample_role_tags.a (cargo build --release --lib -p example-role-tags)")
endif()

# The per-note expression proof (0.7 task 4) links Expression Synth
# (examples/expression_synth): build it with `cargo build --release --lib -p
# example-expression-synth`. As for the role proofs, libexample_expression_synth.a
# next to OP_RUST_LIB is used when OP_EXPRESSION_LIB is not given; without it the
# proof is not built (and says so).
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
  target_link_libraries(${target_name} PRIVATE
    "${rust_lib}"
    "-framework CoreFoundation"
    "-framework Foundation"
    "-framework Security"
    "-framework SystemConfiguration"
    "-liconv")
  # A static library does not carry its framework links. The example always
  # exports the editor ABI, so the archive references AppKit, CoreGraphics and
  # libobjc even when the shell is built with -DOP_EDITOR=OFF. OP_EDITOR only
  # decides whether the shell hosts the view; name the frameworks for every
  # target that links the archive, proofs included.
  target_link_libraries(${target_name} PRIVATE
    "-framework AppKit"
    "-framework CoreGraphics"
    "-lobjc")
endfunction()

enable_testing()

if(OP_BUILD_VST3)
  if(NOT EXISTS "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
    message(FATAL_ERROR "OP_VST3_SDK must point at a VST3 SDK checkout")
  endif()
  set(SMTG_ENABLE_VST3_PLUGIN_EXAMPLES OFF CACHE BOOL "" FORCE)
  set(SMTG_ENABLE_VST3_HOSTING_EXAMPLES OFF CACHE BOOL "" FORCE)
  set(SMTG_ENABLE_VSTGUI_SUPPORT OFF CACHE BOOL "" FORCE)
  set(SMTG_ADD_VST3_UTILITIES OFF CACHE BOOL "" FORCE)
  set(SMTG_CREATE_PLUGIN_LINK OFF CACHE BOOL "" FORCE)
  set(SMTG_RUN_VST_VALIDATOR OFF CACHE BOOL "" FORCE)
  set(SMTG_CREATE_MODULE_INFO OFF CACHE BOOL "" FORCE)
  set(SMTG_DISABLE_CODE_SIGNING ON CACHE BOOL "" FORCE)
  add_subdirectory("${OP_VST3_SDK}" vst3-sdk EXCLUDE_FROM_ALL)
  configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-Info.plist" @ONLY)
  add_library(OpenPluginVST3 MODULE
    vst3/OpenPluginVST3.cpp
    "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
  target_include_directories(OpenPluginVST3 PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  set_target_properties(OpenPluginVST3 PROPERTIES
    OUTPUT_NAME "${OP_PRODUCT}"
    BUNDLE TRUE
    BUNDLE_EXTENSION vst3
    PREFIX ""
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-Info.plist")
  target_link_libraries(OpenPluginVST3 PRIVATE sdk)
  op_link_rust(OpenPluginVST3)
  add_custom_command(TARGET OpenPluginVST3 POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_BUNDLE_DIR:OpenPluginVST3>/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_VST3_SDK}/LICENSE.txt" "$<TARGET_BUNDLE_DIR:OpenPluginVST3>/Contents/Resources/VST3-SDK-LICENSE.txt"
    COMMAND ${CMAKE_COMMAND} -E copy "${CMAKE_CURRENT_SOURCE_DIR}/vst3/PkgInfo" "$<TARGET_BUNDLE_DIR:OpenPluginVST3>/Contents/PkgInfo"
    # The copies above change the bundle after the linker signed it; sign it
    # again (ad hoc) so codesign, auval and hosts accept it.
    COMMAND codesign --force --sign - "$<TARGET_BUNDLE_DIR:OpenPluginVST3>")
  add_executable(VST3Proof vst3/host.cpp
    "${OP_VST3_SDK}/public.sdk/source/common/memorystream.cpp"
    "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
  target_include_directories(VST3Proof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_link_libraries(VST3Proof PRIVATE sdk "-framework CoreFoundation")
  op_link_rust(VST3Proof)
  add_dependencies(VST3Proof OpenPluginVST3)
  add_test(NAME vst3_proof COMMAND VST3Proof "$<TARGET_BUNDLE_DIR:OpenPluginVST3>")
  # process() must not allocate: counted through operator new and libmalloc's malloc_logger hook.
  # Needs no plugin ABI of its own; it reads the effect class from the bundle.
  add_executable(VST3NoAllocProof vst3/noalloc.cpp
    "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
  target_include_directories(VST3NoAllocProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_link_libraries(VST3NoAllocProof PRIVATE sdk "-framework CoreFoundation")
  add_dependencies(VST3NoAllocProof OpenPluginVST3)
  add_test(NAME vst3_noalloc_proof COMMAND VST3NoAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3>")
  if(NOT OP_EDITOR)
    # The same shell over a stub ABI that reports 37 samples of latency, because
    # the SDK examples report none and cannot show the bypass delay. The stub
    # does not export the editor ABI, so this is built without OP_EDITOR only.
    set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
    set(OP_PRODUCT "Latency Stub")
    configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-latency-Info.plist" @ONLY)
    set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
    add_library(OpenPluginVST3Latency MODULE
      vst3/OpenPluginVST3.cpp
      vst3/latency_stub.cpp
      "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
    target_include_directories(OpenPluginVST3Latency PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    set_target_properties(OpenPluginVST3Latency PROPERTIES
      OUTPUT_NAME "Latency Stub"
      BUNDLE TRUE
      BUNDLE_EXTENSION vst3
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-latency-Info.plist")
    target_link_libraries(OpenPluginVST3Latency PRIVATE sdk "-framework CoreFoundation")
    add_executable(VST3BypassLatencyProof vst3/bypass_latency.cpp
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
    target_include_directories(VST3BypassLatencyProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(VST3BypassLatencyProof PRIVATE sdk "-framework CoreFoundation")
    add_dependencies(VST3BypassLatencyProof OpenPluginVST3Latency)
    add_test(NAME vst3_bypass_latency_proof COMMAND VST3BypassLatencyProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Latency>")
    add_dependencies(VST3NoAllocProof OpenPluginVST3Latency)
    add_test(NAME vst3_noalloc_latency_proof COMMAND VST3NoAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Latency>")
    # The same shell over a stub plugin that has a chunk of its own, for the
    # state proof: the example has none, so it cannot show the chunk travel.
    set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
    set(OP_PRODUCT "State Stub")
    configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-state-Info.plist" @ONLY)
    set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
    add_library(OpenPluginVST3State MODULE
      vst3/OpenPluginVST3.cpp
      vst3/state_stub.cpp
      "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
    target_include_directories(OpenPluginVST3State PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    set_target_properties(OpenPluginVST3State PROPERTIES
      OUTPUT_NAME "State Stub"
      BUNDLE TRUE
      BUNDLE_EXTENSION vst3
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-state-Info.plist")
    target_link_libraries(OpenPluginVST3State PRIVATE sdk "-framework CoreFoundation")
    add_executable(VST3StateProof vst3/state.cpp
      "${OP_VST3_SDK}/public.sdk/source/common/memorystream.cpp")
    target_include_directories(VST3StateProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    target_link_libraries(VST3StateProof PRIVATE sdk "-framework CoreFoundation")
    add_dependencies(VST3StateProof OpenPluginVST3State)
    add_test(NAME vst3_state_proof COMMAND VST3StateProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3State>")
    # The host's track (Vst::ChannelContext::IInfoListener): the same shell over a stub plugin that
    # records op_shell_set_track_info, because the example keeps no track to read back.
    set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
    set(OP_PRODUCT "Track Stub")
    configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-track-Info.plist" @ONLY)
    set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
    add_library(OpenPluginVST3Track MODULE
      vst3/OpenPluginVST3.cpp
      vst3/track_stub.cpp
      "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
    target_include_directories(OpenPluginVST3Track PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    set_target_properties(OpenPluginVST3Track PROPERTIES
      OUTPUT_NAME "Track Stub"
      BUNDLE TRUE
      BUNDLE_EXTENSION vst3
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-track-Info.plist")
    target_link_libraries(OpenPluginVST3Track PRIVATE sdk "-framework CoreFoundation")
    add_executable(VST3TrackProof vst3/track.cpp
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/hostclasses.cpp"
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/pluginterfacesupport.cpp")
    target_include_directories(VST3TrackProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    target_link_libraries(VST3TrackProof PRIVATE sdk "-framework CoreFoundation")
    add_dependencies(VST3TrackProof OpenPluginVST3Track)
    add_test(NAME vst3_track_proof COMMAND VST3TrackProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Track>")
    # Telling the host about changes (shell ABI 6): the same shell over a stub whose latency follows
    # its parameter (vst3/changes_stub.cpp), with a host that records restartComponent.
    set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
    set(OP_PRODUCT "Changes Stub")
    configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-changes-Info.plist" @ONLY)
    set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
    add_library(OpenPluginVST3Changes MODULE
      vst3/OpenPluginVST3.cpp
      vst3/changes_stub.cpp
      "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
    target_include_directories(OpenPluginVST3Changes PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    set_target_properties(OpenPluginVST3Changes PROPERTIES
      OUTPUT_NAME "Changes Stub"
      BUNDLE TRUE
      BUNDLE_EXTENSION vst3
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-changes-Info.plist")
    target_link_libraries(OpenPluginVST3Changes PRIVATE sdk "-framework CoreFoundation")
    add_executable(VST3ChangesProof vst3/changes.cpp
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/hostclasses.cpp"
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/pluginterfacesupport.cpp"
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
    target_include_directories(VST3ChangesProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    target_link_libraries(VST3ChangesProof PRIVATE sdk "-framework CoreFoundation")
    add_dependencies(VST3ChangesProof OpenPluginVST3Changes)
    add_test(NAME vst3_changes_proof COMMAND VST3ChangesProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Changes>")
    # process() still allocates nothing while the latency moves under it.
    add_dependencies(VST3NoAllocProof OpenPluginVST3Changes)
    add_test(NAME vst3_noalloc_changes_proof COMMAND VST3NoAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Changes>")
    # ABI 2 events proof: the same shell over three stand-in plugins (native/vst3/events_stub.cpp):
    # an effect with a sidechain and surround layouts, an instrument, and a note effect with no
    # audio. The example has no notes, sidechain or surround layouts, so it cannot show those.
    function(op_vst3_events_stub stub_variant stub_name stub_target)
      set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
      set(OP_PRODUCT "${stub_name}")
      configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/${stub_target}-Info.plist" @ONLY)
      set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
      add_library(${stub_target} MODULE
        vst3/OpenPluginVST3.cpp
        vst3/events_stub.cpp
        "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
      target_include_directories(${stub_target} PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
      target_compile_definitions(${stub_target} PRIVATE STUB_VARIANT=${stub_variant})
      set_target_properties(${stub_target} PROPERTIES
        OUTPUT_NAME "${stub_name}"
        BUNDLE TRUE
        BUNDLE_EXTENSION vst3
        PREFIX ""
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/${stub_target}-Info.plist")
      target_link_libraries(${stub_target} PRIVATE sdk "-framework CoreFoundation")
    endfunction()
    op_vst3_events_stub(0 "Events Stub" OpenPluginVST3Events)
    op_vst3_events_stub(1 "Synth Stub" OpenPluginVST3Synth)
    op_vst3_events_stub(2 "Note Stub" OpenPluginVST3Notes)
    add_executable(VST3EventsProof vst3/events_host.cpp
      "${OP_VST3_SDK}/public.sdk/source/common/memorystream.cpp"
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp"
      "${OP_VST3_SDK}/public.sdk/source/vst/hosting/eventlist.cpp")
    target_include_directories(VST3EventsProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/vst3")
    target_link_libraries(VST3EventsProof PRIVATE sdk "-framework CoreFoundation")
    add_dependencies(VST3EventsProof OpenPluginVST3Events OpenPluginVST3Synth OpenPluginVST3Notes)
    add_test(NAME vst3_noalloc_events_proof COMMAND VST3NoAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Events>")
    add_dependencies(VST3NoAllocProof OpenPluginVST3Events)
    add_test(NAME vst3_events_proof COMMAND VST3EventsProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Events>" "$<TARGET_BUNDLE_DIR:OpenPluginVST3Synth>" "$<TARGET_BUNDLE_DIR:OpenPluginVST3Notes>")
    # Channel order proof: the shell over Role Tags, every layout from mono to 9.1.6.
    if(OP_ROLE_PROOFS)
      set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
      set(OP_PRODUCT "Role Tags")
      configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-roles-Info.plist" @ONLY)
      set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
      add_library(OpenPluginVST3Roles MODULE
        vst3/OpenPluginVST3.cpp
        "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
      target_include_directories(OpenPluginVST3Roles PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
      set_target_properties(OpenPluginVST3Roles PROPERTIES
        OUTPUT_NAME "Role Tags"
        BUNDLE TRUE
        BUNDLE_EXTENSION vst3
        PREFIX ""
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-roles-Info.plist")
      target_link_libraries(OpenPluginVST3Roles PRIVATE sdk)
      op_link_rust(OpenPluginVST3Roles "${OP_ROLE_LIB}")
      add_executable(VST3RolesProof vst3/roles_host.cpp)
      target_include_directories(VST3RolesProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
      target_link_libraries(VST3RolesProof PRIVATE sdk "-framework CoreFoundation")
      add_dependencies(VST3RolesProof OpenPluginVST3Roles)
      add_test(NAME vst3_roles_proof COMMAND VST3RolesProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Roles>")
    endif()
    # Per-note expression proof (ABI 7): the shell over Expression Synth, note ids and
    # kNoteExpressionValueEvent in, INoteExpressionController read back.
    if(OP_EXPRESSION_PROOFS)
      set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
      set(OP_PRODUCT "Expression Synth")
      configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-expression-Info.plist" @ONLY)
      set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
      add_library(OpenPluginVST3Expression MODULE
        vst3/OpenPluginVST3.cpp
        "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
      target_include_directories(OpenPluginVST3Expression PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
      set_target_properties(OpenPluginVST3Expression PROPERTIES
        OUTPUT_NAME "Expression Synth"
        BUNDLE TRUE
        BUNDLE_EXTENSION vst3
        PREFIX ""
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-expression-Info.plist")
      target_link_libraries(OpenPluginVST3Expression PRIVATE sdk)
      op_link_rust(OpenPluginVST3Expression "${OP_EXPRESSION_LIB}")
      add_executable(VST3ExpressionProof vst3/expression_host.cpp
        "${OP_VST3_SDK}/public.sdk/source/vst/hosting/eventlist.cpp")
      target_include_directories(VST3ExpressionProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
      target_link_libraries(VST3ExpressionProof PRIVATE sdk "-framework CoreFoundation")
      add_dependencies(VST3ExpressionProof OpenPluginVST3Expression)
      add_test(NAME vst3_expression_proof COMMAND VST3ExpressionProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3Expression>")
    endif()
  endif()
  if(OP_EDITOR)
    # The editor in the VST3 shell. OP_EDITOR_PROOF_CLICK is "x;y;param":
    # a point (points from the top left) whose click edits that parameter.
    set(OP_EDITOR_PROOF_CLICK "" CACHE STRING "x;y;param for VST3EditorProof, or empty")
    enable_language(OBJCXX)
    add_executable(VST3EditorProof vst3/editor_host.mm
      "${OP_VST3_SDK}/public.sdk/source/vst/vstinitiids.cpp")
    set_source_files_properties(vst3/editor_host.mm PROPERTIES COMPILE_FLAGS "-fno-objc-arc")
    target_link_libraries(VST3EditorProof PRIVATE sdk "-framework Cocoa")
    add_dependencies(VST3EditorProof OpenPluginVST3)
    add_test(NAME vst3_editor_proof COMMAND VST3EditorProof "$<TARGET_BUNDLE_DIR:OpenPluginVST3>" ${OP_EDITOR_PROOF_CLICK})
  endif()
endif()

if(OP_BUILD_AU)
  if(NOT APPLE OR NOT EXISTS "${OP_AU_SDK}/include/AudioUnitSDK/AUEffectBase.h")
    message(FATAL_ERROR "OP_AU_SDK must point at an AudioUnitSDK checkout")
  endif()
  file(GLOB OP_AU_SDK_SOURCES CONFIGURE_DEPENDS "${OP_AU_SDK}/src/AudioUnitSDK/*.cpp")
  add_library(openplugin_au_sdk STATIC ${OP_AU_SDK_SOURCES})
  target_compile_features(openplugin_au_sdk PUBLIC cxx_std_23)
  set_target_properties(openplugin_au_sdk PROPERTIES POSITION_INDEPENDENT_CODE ON)
  target_include_directories(openplugin_au_sdk PUBLIC "${OP_AU_SDK}/include")
  target_link_libraries(openplugin_au_sdk PUBLIC "-framework AudioToolbox" "-framework CoreAudio" "-framework CoreFoundation" "-framework CoreMIDI")
  # The component type: aufx (effect), aumu (instrument) or aumf (music effect, an effect that
  # takes MIDI). "auto" asks the plugin (op_shell_kind) by building and running au/kind_probe.c
  # against OP_RUST_LIB; an effect is aufx even though every plugin's default note ports have a
  # MIDI input, so a plugin that wants aumf says -DOP_AU_TYPE=aumf. The same name is
  # @OP_AU_TYPE@ in the Info.plist and OP_AU_TYPE_CODE in the shell (op_au_type_code).
  set(OP_AU_TYPE "auto" CACHE STRING "Audio Unit component type: auto, aufx, aumu or aumf")
  if(OP_AU_TYPE STREQUAL "auto")
    unset(OP_AU_KIND_RUN CACHE)
    unset(OP_AU_KIND_COMPILE CACHE)
    try_run(OP_AU_KIND_RUN OP_AU_KIND_COMPILE
      "${CMAKE_BINARY_DIR}/au-kind-probe"
      "${CMAKE_CURRENT_SOURCE_DIR}/au/kind_probe.c"
      CMAKE_FLAGS "-DINCLUDE_DIRECTORIES=${CMAKE_CURRENT_SOURCE_DIR}/include"
      LINK_LIBRARIES "${OP_RUST_LIB}" "-framework CoreFoundation" "-framework Foundation" "-framework Security"
        "-framework SystemConfiguration" "-liconv" "-framework AppKit" "-framework CoreGraphics" "-lobjc"
      RUN_OUTPUT_VARIABLE OP_AU_KIND_OUTPUT)
    string(STRIP "${OP_AU_KIND_OUTPUT}" OP_AU_KIND_OUTPUT)
    if(OP_AU_KIND_COMPILE AND "${OP_AU_KIND_RUN}" STREQUAL "0" AND OP_AU_KIND_OUTPUT MATCHES "^(aufx|aumu|aumf)$")
      set(OP_AU_TYPE "${OP_AU_KIND_OUTPUT}")
    else()
      message(WARNING "OP_AU_TYPE=auto could not ask the plugin; building aufx. Pass -DOP_AU_TYPE=aufx|aumu|aumf.")
      set(OP_AU_TYPE "aufx")
    endif()
    message(STATUS "Audio Unit type from the plugin: ${OP_AU_TYPE}")
  elseif(NOT OP_AU_TYPE MATCHES "^(aufx|aumu|aumf)$")
    message(FATAL_ERROR "OP_AU_TYPE must be auto, aufx, aumu or aumf")
  endif()
  function(op_au_type_code out type)
    if(type STREQUAL "aumu")
      set(${out} 1 PARENT_SCOPE)
    elseif(type STREQUAL "aumf")
      set(${out} 2 PARENT_SCOPE)
    else()
      set(${out} 0 PARENT_SCOPE)
    endif()
  endfunction()
  # Every target that compiles au/OpenPluginAU.cpp says which component it is.
  function(op_au_set_type target type)
    op_au_type_code(code "${type}")
    target_compile_definitions(${target} PRIVATE OP_AU_TYPE_CODE=${code})
  endfunction()
  configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-Info.plist" @ONLY)
  add_library(OpenPluginAU MODULE au/OpenPluginAU.cpp)
  op_au_set_type(OpenPluginAU "${OP_AU_TYPE}")
  if(OP_EDITOR)
    target_sources(OpenPluginAU PRIVATE au/OpenPluginAUView.mm)
    target_compile_definitions(OpenPluginAU PRIVATE
      "OP_AU_BUNDLE_ID=\"${OP_BUNDLE_ID}.au\""
      "OP_AU_VIEW_CLASS=OpenPluginAUViewFactory_${OP_CLASS_SUFFIX}"
      "OP_AU_VIEW_CLASS_NAME=\"OpenPluginAUViewFactory_${OP_CLASS_SUFFIX}\""
      "OP_AU_CONTAINER_CLASS=OpenPluginAUView_${OP_CLASS_SUFFIX}")
    target_link_libraries(OpenPluginAU PRIVATE "-framework Cocoa" "-framework AudioUnit")
  endif()
  target_compile_features(OpenPluginAU PRIVATE cxx_std_23)
  target_include_directories(OpenPluginAU PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
  set_target_properties(OpenPluginAU PROPERTIES
    OUTPUT_NAME "${OP_PRODUCT}"
    BUNDLE TRUE
    BUNDLE_EXTENSION component
    PREFIX ""
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-Info.plist")
  target_link_libraries(OpenPluginAU PRIVATE openplugin_au_sdk)
  op_link_rust(OpenPluginAU)
  add_custom_command(TARGET OpenPluginAU POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_BUNDLE_DIR:OpenPluginAU>/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_AU_SDK}/LICENSE.txt" "$<TARGET_BUNDLE_DIR:OpenPluginAU>/Contents/Resources/AudioUnitSDK-LICENSE.txt"
    # Sign again (ad hoc) after changing the bundle, or registration refuses it.
    COMMAND codesign --force --sign - "$<TARGET_BUNDLE_DIR:OpenPluginAU>")
  add_executable(AUProof au/host.cpp)
  target_include_directories(AUProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_link_libraries(AUProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
  op_link_rust(AUProof)
  add_dependencies(AUProof OpenPluginAU)
  add_test(NAME au_proof COMMAND AUProof "$<TARGET_BUNDLE_DIR:OpenPluginAU>")
  add_library(OpenPluginAllocHook SHARED au/alloc_hook.cpp)
  add_executable(AUAllocProof au/alloc_host.cpp)
  target_include_directories(AUAllocProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/au")
  target_link_libraries(AUAllocProof PRIVATE OpenPluginAllocHook "-framework AudioToolbox" "-framework CoreFoundation")
  op_link_rust(AUAllocProof)
  add_dependencies(AUAllocProof OpenPluginAU)
  add_test(NAME au_alloc_proof COMMAND AUAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginAU>")
  if(OP_EDITOR)
    # The editor in the Audio Unit shell. Same OP_EDITOR_PROOF_CLICK ("x;y;param")
    # as the VST3 proof; empty clicks the Warm toggle of examples/editor.
    set(OP_EDITOR_PROOF_CLICK "" CACHE STRING "x;y;param for VST3EditorProof and AUEditorProof, or empty")
    enable_language(OBJCXX)
    add_executable(AUEditorProof au/editor_host.mm)
    set_source_files_properties(au/editor_host.mm PROPERTIES COMPILE_FLAGS "-fno-objc-arc")
    target_include_directories(AUEditorProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(AUEditorProof PRIVATE "-framework Cocoa" "-framework AudioToolbox" "-framework AudioUnit" "-framework CoreFoundation")
    add_dependencies(AUEditorProof OpenPluginAU)
    add_test(NAME au_editor_proof COMMAND AUEditorProof "$<TARGET_BUNDLE_DIR:OpenPluginAU>" ${OP_EDITOR_PROOF_CLICK})
  endif()
  if(NOT OP_EDITOR)
    # The same shell source over a C++ stand-in plugin that reports 37 samples of
    # latency, which the SDK example (latency 0) cannot exercise. Test only.
    function(op_au_latency_plist)
      set(OP_PRODUCT "LatencyTest")
      set(OP_BUNDLE_ID "com.opengrid.test.latency")
      set(OP_VERSION "1")
      set(OP_MANUFACTURER "OpenPlugin")
      set(OP_AU_SUBTYPE "LtTs")
      set(OP_AU_MANUFACTURER "OpTs")
      set(OP_AU_TYPE "aufx")
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-latency-Info.plist" @ONLY)
    endfunction()
    op_au_latency_plist()
    add_library(OpenPluginAULatency MODULE au/OpenPluginAU.cpp au/latency_shell.cpp)
    op_au_set_type(OpenPluginAULatency aufx)
    target_compile_features(OpenPluginAULatency PRIVATE cxx_std_23)
    target_include_directories(OpenPluginAULatency PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
    set_target_properties(OpenPluginAULatency PROPERTIES
      OUTPUT_NAME "LatencyTest"
      BUNDLE TRUE
      BUNDLE_EXTENSION component
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-latency-Info.plist")
    target_link_libraries(OpenPluginAULatency PRIVATE openplugin_au_sdk)
    add_executable(AUBypassProof au/bypass_host.cpp)
    target_include_directories(AUBypassProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(AUBypassProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
    add_dependencies(AUBypassProof OpenPluginAULatency)
    add_test(NAME au_bypass_proof COMMAND AUBypassProof "$<TARGET_BUNDLE_DIR:OpenPluginAULatency>")
    # Allocation with a non-zero latency: the delay line runs.
    add_test(NAME au_alloc_latency_proof COMMAND AUAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginAULatency>")
    set_tests_properties(au_alloc_latency_proof PROPERTIES DEPENDS au_bypass_proof)
    # Plugin state through kAudioUnitProperty_ClassInfo, over a stand-in with a chunk of its own.
    function(op_au_state_plist)
      set(OP_PRODUCT "StateTest")
      set(OP_BUNDLE_ID "com.opengrid.test.state")
      set(OP_VERSION "1")
      set(OP_MANUFACTURER "OpenPlugin")
      set(OP_AU_SUBTYPE "StTs")
      set(OP_AU_MANUFACTURER "OpTs")
      set(OP_AU_TYPE "aufx")
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-state-Info.plist" @ONLY)
    endfunction()
    op_au_state_plist()
    add_library(OpenPluginAUState MODULE au/OpenPluginAU.cpp au/state_shell.cpp)
    op_au_set_type(OpenPluginAUState aufx)
    target_compile_features(OpenPluginAUState PRIVATE cxx_std_23)
    target_include_directories(OpenPluginAUState PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
    set_target_properties(OpenPluginAUState PROPERTIES
      OUTPUT_NAME "StateTest"
      BUNDLE TRUE
      BUNDLE_EXTENSION component
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-state-Info.plist")
    target_link_libraries(OpenPluginAUState PRIVATE openplugin_au_sdk)
    add_executable(AUStateProof au/state_host.cpp)
    target_compile_features(AUStateProof PRIVATE cxx_std_23)
    target_include_directories(AUStateProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(AUStateProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
    add_dependencies(AUStateProof OpenPluginAUState)
    add_test(NAME au_state_proof COMMAND AUStateProof "$<TARGET_BUNDLE_DIR:OpenPluginAUState>")
    # Parameter text over a stand-in with a choice parameter, which the SDK example lacks.
    function(op_au_text_plist)
      set(OP_PRODUCT "TextTest")
      set(OP_BUNDLE_ID "com.opengrid.test.text")
      set(OP_VERSION "1")
      set(OP_MANUFACTURER "OpenPlugin")
      set(OP_AU_SUBTYPE "TxTs")
      set(OP_AU_MANUFACTURER "OpTs")
      set(OP_AU_TYPE "aufx")
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-text-Info.plist" @ONLY)
    endfunction()
    op_au_text_plist()
    add_library(OpenPluginAUText MODULE au/OpenPluginAU.cpp au/text_shell.cpp)
    op_au_set_type(OpenPluginAUText aufx)
    target_compile_features(OpenPluginAUText PRIVATE cxx_std_23)
    target_include_directories(OpenPluginAUText PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
    set_target_properties(OpenPluginAUText PROPERTIES
      OUTPUT_NAME "TextTest"
      BUNDLE TRUE
      BUNDLE_EXTENSION component
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-text-Info.plist")
    target_link_libraries(OpenPluginAUText PRIVATE openplugin_au_sdk)
    add_executable(AUTextProof au/text_host.cpp)
    target_include_directories(AUTextProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(AUTextProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
    add_dependencies(AUTextProof OpenPluginAUText)
    add_test(NAME au_text_proof COMMAND AUTextProof "$<TARGET_BUNDLE_DIR:OpenPluginAUText>")
    # Telling the host about changes (shell ABI 6): the shell over the stand-in whose latency
    # follows its parameter (vst3/changes_stub.cpp, shared with the VST3 proof), with a host that
    # listens to the latency and parameter properties and runs the main run loop.
    function(op_au_changes_plist)
      set(OP_PRODUCT "ChangesTest")
      set(OP_BUNDLE_ID "com.opengrid.test.changes")
      set(OP_VERSION "1")
      set(OP_MANUFACTURER "OpenPlugin")
      set(OP_AU_SUBTYPE "ChTs")
      set(OP_AU_MANUFACTURER "OpTs")
      set(OP_AU_TYPE "aufx")
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-changes-Info.plist" @ONLY)
    endfunction()
    op_au_changes_plist()
    add_library(OpenPluginAUChanges MODULE au/OpenPluginAU.cpp vst3/changes_stub.cpp)
    op_au_set_type(OpenPluginAUChanges aufx)
    target_compile_features(OpenPluginAUChanges PRIVATE cxx_std_23)
    target_include_directories(OpenPluginAUChanges PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
    set_target_properties(OpenPluginAUChanges PROPERTIES
      OUTPUT_NAME "ChangesTest"
      BUNDLE TRUE
      BUNDLE_EXTENSION component
      PREFIX ""
      MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-changes-Info.plist")
    target_link_libraries(OpenPluginAUChanges PRIVATE openplugin_au_sdk)
    add_executable(AUChangesProof au/changes_host.cpp)
    target_include_directories(AUChangesProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_libraries(AUChangesProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
    add_dependencies(AUChangesProof OpenPluginAUChanges)
    add_test(NAME au_changes_proof COMMAND AUChangesProof "$<TARGET_BUNDLE_DIR:OpenPluginAUChanges>")
    # The render path still allocates nothing while the latency moves under it.
    add_dependencies(AUAllocProof OpenPluginAUChanges)
    add_test(NAME au_alloc_changes_proof COMMAND AUAllocProof "$<TARGET_BUNDLE_DIR:OpenPluginAUChanges>")
    set_tests_properties(au_alloc_changes_proof PROPERTIES DEPENDS au_changes_proof)
    # Shell ABI 2 parity (events, MIDI in and out, transport, sidechain, layouts, offline, the ABI
    # check) over au/probe_shell.cpp, a stand-in that implements all of ABI 2 and reports what it
    # received. Five components of the same shell source: an effect (aufx), an instrument (aumu), a
    # music effect (aumf), and two effects whose library reports OP_SHELL_ABI + 1 (newer than the shell, accepted) and ABI 2 (older than OP_SHELL_ABI, refused).
    function(op_au_probe_bundle target product subtype type kind abi)
      set(OP_PRODUCT "${product}")
      set(OP_BUNDLE_ID "com.opengrid.test.${subtype}")
      set(OP_VERSION "1")
      set(OP_MANUFACTURER "OpenPlugin")
      set(OP_AU_SUBTYPE "${subtype}")
      set(OP_AU_MANUFACTURER "OpTs")
      set(OP_AU_TYPE "${type}")
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/${target}-Info.plist" @ONLY)
      add_library(${target} MODULE au/OpenPluginAU.cpp au/probe_shell.cpp)
      op_au_set_type(${target} ${type})
      target_compile_definitions(${target} PRIVATE OP_PROBE_KIND=${kind} OP_PROBE_ABI=${abi})
      target_compile_features(${target} PRIVATE cxx_std_23)
      target_include_directories(${target} PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
      set_target_properties(${target} PROPERTIES
        OUTPUT_NAME "${product}"
        BUNDLE TRUE
        BUNDLE_EXTENSION component
        PREFIX ""
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/${target}-Info.plist")
      target_link_libraries(${target} PRIVATE openplugin_au_sdk)
    endfunction()
    op_au_probe_bundle(OpenPluginAUProbeFx "ProbeFx" "PrFx" aufx 0 OP_SHELL_ABI)
    op_au_probe_bundle(OpenPluginAUProbeInst "ProbeInst" "PrIn" aumu 1 OP_SHELL_ABI)
    op_au_probe_bundle(OpenPluginAUProbeMidiFx "ProbeMidiFx" "PrMf" aumf 2 OP_SHELL_ABI)
    op_au_probe_bundle(OpenPluginAUProbeNewer "ProbeNewer" "PrNw" aufx 0 "(OP_SHELL_ABI+1)")
    op_au_probe_bundle(OpenPluginAUProbeOlder "ProbeOlder" "PrOl" aufx 0 2)
    add_executable(AUEventsProof au/events_host.cpp)
    target_compile_features(AUEventsProof PRIVATE cxx_std_23)
    target_include_directories(AUEventsProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/au")
    target_link_libraries(AUEventsProof PRIVATE OpenPluginAllocHook "-framework AudioToolbox" "-framework CoreFoundation" "-framework CoreMIDI")
    add_dependencies(AUEventsProof OpenPluginAUProbeFx OpenPluginAUProbeInst OpenPluginAUProbeMidiFx OpenPluginAUProbeNewer OpenPluginAUProbeOlder)
    add_test(NAME au_events_proof COMMAND AUEventsProof
      "$<TARGET_BUNDLE_DIR:OpenPluginAUProbeFx>" "$<TARGET_BUNDLE_DIR:OpenPluginAUProbeInst>"
      "$<TARGET_BUNDLE_DIR:OpenPluginAUProbeMidiFx>" "$<TARGET_BUNDLE_DIR:OpenPluginAUProbeNewer>"
      "$<TARGET_BUNDLE_DIR:OpenPluginAUProbeOlder>")
    # Channel order proof: the shell over Role Tags, every layout from mono to 9.1.6.
    if(OP_ROLE_PROOFS)
      function(op_au_roles_plist)
        set(OP_PRODUCT "RoleTags")
        set(OP_BUNDLE_ID "com.opengrid.test.roletags")
        set(OP_VERSION "1")
        set(OP_MANUFACTURER "OpenPlugin")
        set(OP_AU_SUBTYPE "RlTg")
        set(OP_AU_MANUFACTURER "OpTs")
        set(OP_AU_TYPE "aufx")
        configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-roles-Info.plist" @ONLY)
      endfunction()
      op_au_roles_plist()
      add_library(OpenPluginAURoles MODULE au/OpenPluginAU.cpp)
      op_au_set_type(OpenPluginAURoles aufx)
      target_compile_features(OpenPluginAURoles PRIVATE cxx_std_23)
      target_include_directories(OpenPluginAURoles PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
      set_target_properties(OpenPluginAURoles PROPERTIES
        OUTPUT_NAME "RoleTags"
        BUNDLE TRUE
        BUNDLE_EXTENSION component
        PREFIX ""
        MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-roles-Info.plist")
      target_link_libraries(OpenPluginAURoles PRIVATE openplugin_au_sdk)
      op_link_rust(OpenPluginAURoles "${OP_ROLE_LIB}")
      add_executable(AURolesProof au/roles_host.cpp)
      target_compile_features(AURolesProof PRIVATE cxx_std_23)
      target_include_directories(AURolesProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/au")
      target_link_libraries(AURolesProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
      add_dependencies(AURolesProof OpenPluginAURoles)
      add_test(NAME au_roles_proof COMMAND AURolesProof "$<TARGET_BUNDLE_DIR:OpenPluginAURoles>")
    endif()
  endif()
endif()

if(OP_BUILD_AAX)
  if(NOT APPLE OR NOT EXISTS "${OP_AAX_SDK}/Interfaces/AAX.h")
    message(FATAL_ERROR "OP_AAX_SDK must point at an existing AAX SDK")
  endif()
  file(GLOB OP_AAX_SDK_CPP CONFIGURE_DEPENDS "${OP_AAX_SDK}/Libs/AAXLibrary/source/*.cpp")
  list(FILTER OP_AAX_SDK_CPP EXCLUDE REGEX "\\.(Win|Windows)\\.cpp$")
  add_library(openplugin_aax_sdk STATIC ${OP_AAX_SDK_CPP} "${OP_AAX_SDK}/Libs/AAXLibrary/source/AAX_CAutoreleasePool.OSX.mm")
  target_include_directories(openplugin_aax_sdk PUBLIC
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF"
    "${OP_AAX_SDK}/Libs/AAXLibrary/include")
  target_compile_definitions(openplugin_aax_sdk PRIVATE UNICODE=1)
  set_target_properties(openplugin_aax_sdk PROPERTIES POSITION_INDEPENDENT_CODE ON)
  configure_file(aax/Info.plist.in "${CMAKE_BINARY_DIR}/aax-Info.plist" @ONLY)
  add_library(OpenPluginAAX MODULE
    aax/OpenPluginAAXDescribe.cpp
    aax/OpenPluginAAXParameters.cpp
    aax/OpenPluginAAXState.cpp
    aax/OpenPluginAAXProcess.cpp
    aax/OpenPluginAAXGUI.cpp
    "${OP_AAX_SDK}/Interfaces/AAX_Exports.cpp")
  target_include_directories(OpenPluginAAX PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF"
    "${OP_AAX_SDK}/Libs/AAXLibrary/include")
  set_target_properties(OpenPluginAAX PROPERTIES
    OUTPUT_NAME "${OP_PRODUCT}"
    BUNDLE TRUE
    BUNDLE_EXTENSION aaxplugin
    PREFIX ""
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/aax-Info.plist")
  target_link_libraries(OpenPluginAAX PRIVATE openplugin_aax_sdk "-framework Cocoa")
  op_link_rust(OpenPluginAAX)
  add_custom_command(TARGET OpenPluginAAX POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_BUNDLE_DIR:OpenPluginAAX>/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_AAX_SDK}/LICENSE.txt" "$<TARGET_BUNDLE_DIR:OpenPluginAAX>/Contents/Resources/AAX-SDK-LICENSE.txt")
  if(OP_AAX_PAGES)
    # The tool links the plugin, asks it for its parameters and writes the table.
    add_executable(OpenPluginAAXPages aax/pages.cpp)
    target_include_directories(OpenPluginAAXPages PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/aax")
    op_link_rust(OpenPluginAAXPages)
    add_dependencies(OpenPluginAAX OpenPluginAAXPages)
    target_compile_definitions(OpenPluginAAX PRIVATE "OP_AAX_PAGE_TABLE=\"${OP_PRODUCT}Pages.xml\"")
    add_custom_command(TARGET OpenPluginAAX POST_BUILD
      COMMAND OpenPluginAAXPages "$<TARGET_BUNDLE_DIR:OpenPluginAAX>/Contents/Resources/${OP_PRODUCT}Pages.xml")
  endif()
  # Last post-build step: sign again (ad hoc) after the licence and page table
  # changed the bundle. A release build re-signs with PACE and a Developer ID.
  add_custom_command(TARGET OpenPluginAAX POST_BUILD
    COMMAND codesign --force --sign - "$<TARGET_BUNDLE_DIR:OpenPluginAAX>")
  add_executable(AAXProof aax/host_process.cpp aax/OpenPluginAAXProcess.cpp)
  target_include_directories(AAXProof PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF")
  target_link_libraries(AAXProof PRIVATE openplugin_aax_sdk)
  op_link_rust(AAXProof)
  # State chunk proof: the shell's real AAX_CEffectParameters, no controller.
  add_executable(AAXStateProof aax/host_state.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp aax/OpenPluginAAXProcess.cpp)
  target_include_directories(AAXStateProof PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF"
    "${OP_AAX_SDK}/Libs/AAXLibrary/include")
  target_link_libraries(AAXStateProof PRIVATE openplugin_aax_sdk)
  op_link_rust(AAXStateProof)
  # Allocation proof: replaces operator new and hooks malloc, so it stays a separate binary.
  add_executable(AAXAllocProof aax/host_alloc.cpp aax/OpenPluginAAXProcess.cpp)
  target_include_directories(AAXAllocProof PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF")
  op_link_rust(AAXAllocProof)
  # Stand-in ABI proofs (aax/proof_stub.cpp, no Rust library): the SDK example reports
  # no latency and has two parameters, so these show the delayed bypass and a large
  # packet. AAXBypassProof: 1 parameter, 37 samples of latency, the process callback.
  # AAXParamsProof: 200 parameters through the real AAX_CEffectParameters and chunk.
  # AAXAllocLatencyProof: aax_alloc_proof over the 200-parameter stub, so the delay
  # line, the crossfade and a large packet run while the allocation counters are armed.
  set(OP_AAX_PROOF_INCLUDES
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF"
    "${OP_AAX_SDK}/Libs/AAXLibrary/include")
  add_executable(AAXBypassProof aax/host_bypass.cpp aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXBypassProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXBypassProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=37)
  add_executable(AAXParamsProof aax/host_params.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp
    aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXParamsProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXParamsProof PRIVATE OP_STUB_PARAMS=200 OP_STUB_LATENCY=37)
  target_link_libraries(AAXParamsProof PRIVATE openplugin_aax_sdk)
  # AAXStateBlobProof: the AAX state chunk (version 2) carrying a plugin-owned chunk through
  # op_shell_save_state/op_shell_load_state, over the 3-parameter stub.
  add_executable(AAXStateBlobProof aax/host_state_blob.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp
    aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXStateBlobProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXStateBlobProof PRIVATE OP_STUB_PARAMS=3 OP_STUB_LATENCY=37)
  target_link_libraries(AAXStateBlobProof PRIVATE openplugin_aax_sdk)
  add_executable(AAXAllocLatencyProof aax/host_alloc.cpp aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXAllocLatencyProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXAllocLatencyProof PRIVATE OP_STUB_PARAMS=200 OP_STUB_LATENCY=37)
  # ABI 2 proofs over the same stand-in (OP_STUB_PARAMS=1, no latency), whose layouts, kind and note
  # ports a proof sets at run time. AAXEventsProof drives the process callback with a fake MIDI node
  # and transport: MIDI in and out at sample offsets, parameter events, transport, sidechain, stems
  # in Pro Tools' order. AAXDescribeProof runs GetEffectDescriptions against a recording collection
  # (components, stems, MIDI nodes, sidechain, categories, ABI check) and the offline notification.
  add_executable(AAXEventsProof aax/host_events.cpp aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXEventsProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXEventsProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=0)
  target_link_libraries(AAXEventsProof PRIVATE openplugin_aax_sdk)
  add_executable(AAXDescribeProof aax/host_describe.cpp aax/OpenPluginAAXDescribe.cpp aax/OpenPluginAAXParameters.cpp
    aax/OpenPluginAAXState.cpp aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXDescribeProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXDescribeProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=0)
  target_link_libraries(AAXDescribeProof PRIVATE openplugin_aax_sdk)
  # AAXTextProof: the plugin's own parameter text through the real AAX_CEffectParameters, over the
  # example (Gain, Warm). AAXTextStubProof: the same over a stand-in ABI with a choice, a toggle and
  # multi-byte text (OP_STUB_TEXT in aax/proof_stub.cpp).
  add_executable(AAXTextProof aax/host_text.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp aax/OpenPluginAAXProcess.cpp)
  target_include_directories(AAXTextProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_link_libraries(AAXTextProof PRIVATE openplugin_aax_sdk)
  op_link_rust(AAXTextProof)
  add_executable(AAXTextStubProof aax/host_text.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp
    aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXTextStubProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXTextStubProof PRIVATE OP_STUB_PARAMS=4 OP_STUB_LATENCY=0 OP_STUB_TEXT=1 OP_TEXT_STUB=1)
  target_link_libraries(AAXTextStubProof PRIVATE openplugin_aax_sdk)
  # Channel order proof: the process callback over Role Tags, every layout from mono to 9.1.6.
  if(OP_ROLE_PROOFS)
    add_executable(AAXRolesProof aax/host_roles.cpp aax/OpenPluginAAXProcess.cpp)
    target_include_directories(AAXRolesProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
    target_link_libraries(AAXRolesProof PRIVATE openplugin_aax_sdk)
    op_link_rust(AAXRolesProof "${OP_ROLE_LIB}")
    add_test(NAME aax_roles_proof COMMAND AAXRolesProof)
  endif()
  # AAXChangesProof: telling the host about changes (shell ABI 6) through the real data model's
  # TimerWakeup, with a fake AAX_IController recording SetSignalLatency and ParameterNameChanged.
  add_executable(AAXChangesProof aax/host_changes.cpp aax/OpenPluginAAXParameters.cpp aax/OpenPluginAAXState.cpp
    aax/OpenPluginAAXProcess.cpp aax/proof_stub.cpp)
  target_include_directories(AAXChangesProof PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(AAXChangesProof PRIVATE OP_STUB_PARAMS=3 OP_STUB_LATENCY=0)
  target_link_libraries(AAXChangesProof PRIVATE openplugin_aax_sdk)
  add_executable(AAXBundleProof aax/host_bundle.cpp)
  target_link_libraries(AAXBundleProof PRIVATE "-framework CoreFoundation")
  add_dependencies(AAXBundleProof OpenPluginAAX)
  # Parameter groups in the page table: the page table writer (aax/pages.cpp, without its main)
  # over the real plugin, read back. Built whether or not OP_AAX_PAGES writes a table into the bundle.
  add_executable(AAXPagesProof aax/host_pages.cpp aax/pages.cpp)
  target_include_directories(AAXPagesProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/aax")
  target_compile_definitions(AAXPagesProof PRIVATE OP_AAX_PAGES_NO_MAIN=1)
  op_link_rust(AAXPagesProof)
  add_test(NAME aax_pages_proof COMMAND AAXPagesProof "${CMAKE_BINARY_DIR}/aax-pages-proof.xml")
  add_test(NAME aax_text_proof COMMAND AAXTextProof)
  add_test(NAME aax_text_stub_proof COMMAND AAXTextStubProof)
  add_test(NAME aax_process_proof COMMAND AAXProof)
  add_test(NAME aax_state_proof COMMAND AAXStateProof)
  add_test(NAME aax_alloc_proof COMMAND AAXAllocProof)
  add_test(NAME aax_bypass_proof COMMAND AAXBypassProof)
  add_test(NAME aax_params_proof COMMAND AAXParamsProof)
  add_test(NAME aax_state_blob_proof COMMAND AAXStateBlobProof)
  add_test(NAME aax_alloc_latency_proof COMMAND AAXAllocLatencyProof)
  add_test(NAME aax_events_proof COMMAND AAXEventsProof)
  add_test(NAME aax_describe_proof COMMAND AAXDescribeProof)
  add_test(NAME aax_changes_proof COMMAND AAXChangesProof)
  add_test(NAME aax_bundle_proof COMMAND AAXBundleProof "$<TARGET_BUNDLE_DIR:OpenPluginAAX>")
endif()

# Format parity hosts and negative-control shells (docs/FORMAT_PARITY.md). Test builds only.
option(OP_FORMAT_PARITY "Build the format parity hosts from scripts/format_parity" OFF)
if(OP_FORMAT_PARITY)
  include("${CMAKE_CURRENT_LIST_DIR}/../scripts/format_parity/parity.cmake")
endif()
