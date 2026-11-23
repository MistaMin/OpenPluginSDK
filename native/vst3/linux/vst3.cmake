# SPDX-License-Identifier: MIT
# The VST3 shell on Linux. Included by native/cmake/linux.cmake, which holds the options,
# the Rust library and op_link_rust.
#
#   - A module is `<Name>.vst3/Contents/<arch>-linux/<Name>.so` (x86_64-linux,
#     aarch64-linux) in the build folder, with the VST3 SDK licence in
#     `Contents/Resources`.
#   - The entry points are ModuleEntry and ModuleExit from the SDK's linuxmain.cpp,
#     and GetPluginFactory; nothing else is exported (exports.map).
set(OP_VST3_LINUX_DIR "${CMAKE_CURRENT_LIST_DIR}")

if(CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|AMD64|amd64)$")
  set(OP_VST3_ARCH "x86_64-linux")
elseif(CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
  set(OP_VST3_ARCH "aarch64-linux")
else()
  message(FATAL_ERROR "The Linux shell is built for x86_64 or aarch64, not ${CMAKE_SYSTEM_PROCESSOR}")
endif()

if(NOT EXISTS "${OP_VST3_SDK}/public.sdk/source/main/linuxmain.cpp")
  message(FATAL_ERROR "OP_VST3_SDK must point at a VST3 SDK checkout")
endif()
set(SMTG_ENABLE_VST3_PLUGIN_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SMTG_ENABLE_VST3_HOSTING_EXAMPLES OFF CACHE BOOL "" FORCE)
set(SMTG_ENABLE_VSTGUI_SUPPORT OFF CACHE BOOL "" FORCE)
set(SMTG_ADD_VST3_UTILITIES OFF CACHE BOOL "" FORCE)
set(SMTG_CREATE_PLUGIN_LINK OFF CACHE BOOL "" FORCE)
set(SMTG_RUN_VST_VALIDATOR OFF CACHE BOOL "" FORCE)
set(SMTG_CREATE_MODULE_INFO OFF CACHE BOOL "" FORCE)
add_subdirectory("${OP_VST3_SDK}" vst3-sdk EXCLUDE_FROM_ALL)

set(OP_VST3_DEFINITIONS)
if(OP_TAIL)
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_TAIL=1)
else()
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_TAIL=0)
endif()
if(OP_EDITOR)
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_EDITOR=1 OP_EDITOR_RESIZE=1)
else()
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_EDITOR=0)
endif()

set(OP_VST3_BUNDLE "${CMAKE_BINARY_DIR}/${OP_PRODUCT}.vst3")
add_library(OpenPluginVST3 MODULE
  "${OP_VST3_LINUX_DIR}/OpenPluginVST3Linux.cpp"
  "${OP_VST3_SDK}/public.sdk/source/main/linuxmain.cpp")
if(OP_EDITOR)
  # The view's sizes in pixels, shared with Windows (vst3/win/editor_pixels.h).
  target_sources(OpenPluginVST3 PRIVATE "${OP_NATIVE_DIR}/vst3/win/editor_pixels.cpp")
endif()
target_compile_definitions(OpenPluginVST3 PRIVATE ${OP_VST3_DEFINITIONS})
target_compile_options(OpenPluginVST3 PRIVATE -fvisibility=hidden -fvisibility-inlines-hidden)
target_include_directories(OpenPluginVST3 PRIVATE "${OP_NATIVE_DIR}/include" "${OP_NATIVE_DIR}/vst3")
target_link_libraries(OpenPluginVST3 PRIVATE sdk)
op_link_rust(OpenPluginVST3)
target_link_options(OpenPluginVST3 PRIVATE
  "-Wl,--version-script=${OP_VST3_LINUX_DIR}/exports.map" -Wl,--no-undefined -Wl,-z,defs -Wl,--as-needed
  # The C++ runtime inside the module: it loads on distributions older than the build machine.
  -static-libstdc++)
set_target_properties(OpenPluginVST3 PROPERTIES
  OUTPUT_NAME "${OP_PRODUCT}"
  PREFIX ""
  SUFFIX ".so"
  LIBRARY_OUTPUT_DIRECTORY "${OP_VST3_BUNDLE}/Contents/${OP_VST3_ARCH}"
  LINK_DEPENDS "${OP_VST3_LINUX_DIR}/exports.map")
add_custom_command(TARGET OpenPluginVST3 POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "${OP_VST3_BUNDLE}/Contents/Resources"
  COMMAND ${CMAKE_COMMAND} -E copy "${OP_VST3_SDK}/LICENSE.txt" "${OP_VST3_BUNDLE}/Contents/Resources/VST3-SDK-LICENSE.txt")
