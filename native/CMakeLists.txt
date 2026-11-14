# SPDX-License-Identifier: MIT
# Thin VST3, Audio Unit, and AAX shells over a Rust staticlib that exports op_shell_*.
# Does not sign, install, or copy an SDK into this tree.
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
    COMMAND ${CMAKE_COMMAND} -E copy "${CMAKE_CURRENT_SOURCE_DIR}/vst3/PkgInfo" "$<TARGET_BUNDLE_DIR:OpenPluginVST3>/Contents/PkgInfo")
  add_executable(VST3Proof vst3/host.cpp
    "${OP_VST3_SDK}/public.sdk/source/common/memorystream.cpp"
    "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
  target_include_directories(VST3Proof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_link_libraries(VST3Proof PRIVATE sdk "-framework CoreFoundation")
  add_dependencies(VST3Proof OpenPluginVST3)
  add_test(NAME vst3_proof COMMAND VST3Proof "$<TARGET_BUNDLE_DIR:OpenPluginVST3>")
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
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_AU_SDK}/LICENSE.txt" "$<TARGET_BUNDLE_DIR:OpenPluginAU>/Contents/Resources/AudioUnitSDK-LICENSE.txt")
  add_executable(AUProof au/host.cpp)
  target_include_directories(AUProof PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_link_libraries(AUProof PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
  add_dependencies(AUProof OpenPluginAU)
  add_test(NAME au_proof COMMAND AUProof "$<TARGET_BUNDLE_DIR:OpenPluginAU>")
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
  add_executable(AAXProof aax/host_process.cpp aax/OpenPluginAAXProcess.cpp)
  target_include_directories(AAXProof PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF")
  target_link_libraries(AAXProof PRIVATE openplugin_aax_sdk)
  op_link_rust(AAXProof)
  add_executable(AAXBundleProof aax/host_bundle.cpp)
  target_link_libraries(AAXBundleProof PRIVATE "-framework CoreFoundation")
  add_dependencies(AAXBundleProof OpenPluginAAX)
  add_test(NAME aax_process_proof COMMAND AAXProof)
  add_test(NAME aax_bundle_proof COMMAND AAXBundleProof "$<TARGET_BUNDLE_DIR:OpenPluginAAX>")
endif()
