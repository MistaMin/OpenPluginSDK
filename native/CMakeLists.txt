# SPDX-License-Identifier: MIT
# Thin VST3, Audio Unit, and AAX shells over a Rust staticlib that exports op_shell_*.
# Does not sign, install, or copy an SDK into this tree.
#
# The proofs load the SDK example (examples/editor, "Editor Gain"). Build and
# run them from the repository root; the SDK checkouts stay outside this tree:
#
#   CARGO_BUILD_JOBS=2 cargo build --release --lib -p example-editor
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
cmake_minimum_required(VERSION 3.20)
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

function(op_link_rust target_name)
  target_link_libraries(${target_name} PRIVATE
    "${OP_RUST_LIB}"
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
  target_link_libraries(openplugin_au_sdk PUBLIC "-framework AudioToolbox" "-framework CoreAudio" "-framework CoreFoundation")
  configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-Info.plist" @ONLY)
  add_library(OpenPluginAU MODULE au/OpenPluginAU.cpp)
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
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-latency-Info.plist" @ONLY)
    endfunction()
    op_au_latency_plist()
    add_library(OpenPluginAULatency MODULE au/OpenPluginAU.cpp au/latency_shell.cpp)
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
      configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-state-Info.plist" @ONLY)
    endfunction()
    op_au_state_plist()
    add_library(OpenPluginAUState MODULE au/OpenPluginAU.cpp au/state_shell.cpp)
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
    target_include_directories(OpenPluginAAXPages PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
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
  add_executable(AAXBundleProof aax/host_bundle.cpp)
  target_link_libraries(AAXBundleProof PRIVATE "-framework CoreFoundation")
  add_dependencies(AAXBundleProof OpenPluginAAX)
  add_test(NAME aax_process_proof COMMAND AAXProof)
  add_test(NAME aax_state_proof COMMAND AAXStateProof)
  add_test(NAME aax_alloc_proof COMMAND AAXAllocProof)
  add_test(NAME aax_bypass_proof COMMAND AAXBypassProof)
  add_test(NAME aax_params_proof COMMAND AAXParamsProof)
  add_test(NAME aax_state_blob_proof COMMAND AAXStateBlobProof)
  add_test(NAME aax_alloc_latency_proof COMMAND AAXAllocLatencyProof)
  add_test(NAME aax_bundle_proof COMMAND AAXBundleProof "$<TARGET_BUNDLE_DIR:OpenPluginAAX>")
endif()

# Format parity hosts and negative-control shells (docs/FORMAT_PARITY.md). Test builds only.
option(OP_FORMAT_PARITY "Build the format parity hosts from scripts/format_parity" OFF)
if(OP_FORMAT_PARITY)
  include("${CMAKE_CURRENT_LIST_DIR}/../scripts/format_parity/parity.cmake")
endif()
