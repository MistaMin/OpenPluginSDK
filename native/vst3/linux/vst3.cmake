# SPDX-License-Identifier: MIT
# The VST3 shell and its proofs on Linux. Included by native/cmake/linux.cmake, which
# holds the options, the Rust library and op_link_rust, and explains how to build.
#
# What differs from the macOS build in native/CMakeLists.txt:
#   - A module is `<Name>.vst3/Contents/<machine>-linux/<Name>.so` (x86_64-linux,
#     aarch64-linux) in the build folder, with the
#     VST3 SDK licence in `Contents/Resources`. There is no Info.plist, PkgInfo or code
#     signature.
#   - The entry points are ModuleEntry and ModuleExit from the SDK's linuxmain.cpp.
#   - A module exports those two, GetPluginFactory, and nothing else (exports.map).
#   - Timers. Linux has no run loop of its own for a plug-in: the host lends one
#     (Linux::IRunLoop), through IPluginFactory3::setHostContext for the shell's own
#     timer and through IPlugFrame for the editor.
#   - The proofs are the macOS sources unchanged. vst3/linux/compat stands in for the
#     CoreFoundation calls they make (and is the host run loop the module is given),
#     linux/compat for pthread_main_np, linux/alloc_hook.cpp for libmalloc's
#     malloc_logger.
#   - The settings are given to each target here, so they do not reach the VST3 SDK's
#     own targets.
set(OP_VST3_LINUX_DIR "${CMAKE_CURRENT_LIST_DIR}")

# The folder a VST3 module sits in, named after the machine it runs on.
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

set(OP_VST3_DEFINITIONS "")
if(OP_TAIL)
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_TAIL=1)
else()
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_TAIL=0)
endif()
if(OP_EDITOR)
  # The shell uses the size range and resize calls of op_editor.h.
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_EDITOR=1 OP_EDITOR_RESIZE=1)
else()
  list(APPEND OP_VST3_DEFINITIONS OP_HAS_EDITOR=0)
endif()
function(op_vst3_settings target_name)
  target_compile_definitions(${target_name} PRIVATE ${OP_VST3_DEFINITIONS})
endfunction()

# What the proofs use in place of CoreFoundation: a bundle loaded with dlopen, and
# the run loop a module is lent. linux/compat/main_thread.h (pthread_main_np) goes
# in front of every proof and stand-in plugin source.
add_library(OpenPluginLinuxCompat STATIC "${OP_VST3_LINUX_DIR}/cf_compat.cpp")
op_vst3_settings(OpenPluginLinuxCompat)
target_compile_definitions(OpenPluginLinuxCompat PRIVATE "OP_VST3_ARCH=\"${OP_VST3_ARCH}\"")
target_include_directories(OpenPluginLinuxCompat PUBLIC "${OP_VST3_LINUX_DIR}/compat")
target_link_libraries(OpenPluginLinuxCompat PUBLIC sdk ${CMAKE_DL_LIBS})
set(OP_MAIN_THREAD_HEADER "${OP_NATIVE_DIR}/linux/compat/main_thread.h")

# A VST3 module `<name>.vst3/Contents/<machine>-linux/<name>.so` in the build folder,
# from the shell and the extra sources given (a stand-in plugin). OP_VST3_DIR_<target>
# is the folder.
function(op_vst3_module target_name product_name)
  set(bundle_dir "${CMAKE_BINARY_DIR}/${product_name}.vst3")
  set(OP_VST3_DIR_${target_name} "${bundle_dir}" PARENT_SCOPE)
  add_library(${target_name} MODULE
    "${OP_VST3_LINUX_DIR}/OpenPluginVST3Linux.cpp"
    ${ARGN}
    "${OP_VST3_SDK}/public.sdk/source/main/linuxmain.cpp")
  op_vst3_settings(${target_name})
  target_compile_options(${target_name} PRIVATE -fvisibility=hidden -fvisibility-inlines-hidden -include "${OP_MAIN_THREAD_HEADER}")
  target_include_directories(${target_name} PRIVATE "${OP_NATIVE_DIR}/include" "${OP_NATIVE_DIR}/vst3")
  set_target_properties(${target_name} PROPERTIES
    OUTPUT_NAME "${product_name}"
    PREFIX ""
    SUFFIX ".so"
    LIBRARY_OUTPUT_DIRECTORY "${bundle_dir}/Contents/${OP_VST3_ARCH}"
    LINK_DEPENDS "${OP_VST3_LINUX_DIR}/exports.map")
  target_link_libraries(${target_name} PRIVATE sdk)
  if(OP_EDITOR)
    # The view's sizes in pixels, shared with Windows (vst3/win/editor_pixels.h).
    target_sources(${target_name} PRIVATE "${OP_NATIVE_DIR}/vst3/win/editor_pixels.cpp")
  endif()
  # Every symbol is resolved when the module is linked, not when a host loads it.
  target_link_options(${target_name} PRIVATE
    "-Wl,--version-script=${OP_VST3_LINUX_DIR}/exports.map" -Wl,--no-undefined -Wl,-z,defs -Wl,--as-needed
    # The C++ runtime inside the module: it loads on distributions older than the build machine.
    -static-libstdc++)
  add_custom_command(TARGET ${target_name} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "${bundle_dir}/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_VST3_SDK}/LICENSE.txt" "${bundle_dir}/Contents/Resources/VST3-SDK-LICENSE.txt")
endfunction()

# A proof executable from a macOS proof source and the SDK hosting sources given.
# Its symbols are exported, so a loaded module's calls of malloc and operator new
# reach the counting ones a proof defines.
function(op_vst3_proof target_name source)
  add_executable(${target_name} "${OP_NATIVE_DIR}/vst3/${source}" ${ARGN})
  op_vst3_settings(${target_name})
  target_compile_options(${target_name} PRIVATE -include "${OP_MAIN_THREAD_HEADER}")
  target_include_directories(${target_name} PRIVATE "${OP_NATIVE_DIR}/include" "${OP_NATIVE_DIR}/vst3")
  target_link_libraries(${target_name} PRIVATE sdk OpenPluginLinuxCompat)
  set_target_properties(${target_name} PROPERTIES ENABLE_EXPORTS ON)
endfunction()

set(OP_SDK_SOURCE "${OP_VST3_SDK}/public.sdk/source")

op_vst3_module(OpenPluginVST3 "${OP_PRODUCT}")
op_link_rust(OpenPluginVST3)
include("${OP_NATIVE_DIR}/ara/ara.cmake")
op_ara_vst3(OpenPluginVST3 "${OP_VST3_DIR_OpenPluginVST3}")
op_vst3_proof(VST3Proof host.cpp
  "${OP_SDK_SOURCE}/common/memorystream.cpp"
  "${OP_SDK_SOURCE}/vst/hosting/hostclasses.cpp"
  "${OP_SDK_SOURCE}/vst/hosting/pluginterfacesupport.cpp"
  "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp")
op_link_rust(VST3Proof)
add_dependencies(VST3Proof OpenPluginVST3)
add_test(NAME vst3_proof COMMAND VST3Proof "${OP_VST3_DIR_OpenPluginVST3}")
# process() must not allocate: counted through operator new and the C library's
# allocation functions (linux/alloc_hook.cpp).
op_vst3_proof(VST3NoAllocProof noalloc.cpp
  "${OP_NATIVE_DIR}/linux/alloc_hook.cpp"
  "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp")
add_dependencies(VST3NoAllocProof OpenPluginVST3)
add_test(NAME vst3_noalloc_proof COMMAND VST3NoAllocProof "${OP_VST3_DIR_OpenPluginVST3}")

if(NOT OP_EDITOR)
  # The stand-in plugins do not export the editor ABI, so these are built without
  # OP_EDITOR only, as on macOS.
  op_vst3_module(OpenPluginVST3Latency "Latency Stub" "${OP_NATIVE_DIR}/vst3/latency_stub.cpp")
  op_vst3_proof(VST3BypassLatencyProof bypass_latency.cpp
    "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp")
  add_dependencies(VST3BypassLatencyProof OpenPluginVST3Latency)
  add_test(NAME vst3_bypass_latency_proof COMMAND VST3BypassLatencyProof "${OP_VST3_DIR_OpenPluginVST3Latency}")
  add_dependencies(VST3NoAllocProof OpenPluginVST3Latency)
  add_test(NAME vst3_noalloc_latency_proof COMMAND VST3NoAllocProof "${OP_VST3_DIR_OpenPluginVST3Latency}")

  op_vst3_module(OpenPluginVST3State "State Stub" "${OP_NATIVE_DIR}/vst3/state_stub.cpp")
  op_vst3_proof(VST3StateProof state.cpp
    "${OP_SDK_SOURCE}/common/memorystream.cpp")
  add_dependencies(VST3StateProof OpenPluginVST3State)
  add_test(NAME vst3_state_proof COMMAND VST3StateProof "${OP_VST3_DIR_OpenPluginVST3State}")

  op_vst3_module(OpenPluginVST3Track "Track Stub" "${OP_NATIVE_DIR}/vst3/track_stub.cpp")
  op_vst3_proof(VST3TrackProof track.cpp
    "${OP_SDK_SOURCE}/vst/hosting/hostclasses.cpp"
    "${OP_SDK_SOURCE}/vst/hosting/pluginterfacesupport.cpp")
  add_dependencies(VST3TrackProof OpenPluginVST3Track)
  add_test(NAME vst3_track_proof COMMAND VST3TrackProof "${OP_VST3_DIR_OpenPluginVST3Track}")

  op_vst3_module(OpenPluginVST3Changes "Changes Stub" "${OP_NATIVE_DIR}/vst3/changes_stub.cpp")
  op_vst3_proof(VST3ChangesProof changes.cpp
    "${OP_SDK_SOURCE}/vst/hosting/hostclasses.cpp"
    "${OP_SDK_SOURCE}/vst/hosting/pluginterfacesupport.cpp"
    "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp")
  add_dependencies(VST3ChangesProof OpenPluginVST3Changes)
  add_test(NAME vst3_changes_proof COMMAND VST3ChangesProof "${OP_VST3_DIR_OpenPluginVST3Changes}")
  add_dependencies(VST3NoAllocProof OpenPluginVST3Changes)
  add_test(NAME vst3_noalloc_changes_proof COMMAND VST3NoAllocProof "${OP_VST3_DIR_OpenPluginVST3Changes}")

  # The three stand-ins of vst3/events_stub.cpp: effect with sidechain and surround,
  # instrument, note effect.
  function(op_vst3_events_stub stub_variant stub_name stub_target)
    op_vst3_module(${stub_target} "${stub_name}" "${OP_NATIVE_DIR}/vst3/events_stub.cpp")
    target_compile_definitions(${stub_target} PRIVATE STUB_VARIANT=${stub_variant})
    set(OP_VST3_DIR_${stub_target} "${OP_VST3_DIR_${stub_target}}" PARENT_SCOPE)
  endfunction()
  op_vst3_events_stub(0 "Events Stub" OpenPluginVST3Events)
  op_vst3_events_stub(1 "Synth Stub" OpenPluginVST3Synth)
  op_vst3_events_stub(2 "Note Stub" OpenPluginVST3Notes)
  # The proof counts with its own operator new, which a module with its own C++
  # runtime does not call: vst3/linux/new_bridge.cpp brings every allocation in the
  # process to it.
  op_vst3_proof(VST3EventsProof events_host.cpp
    "${OP_NATIVE_DIR}/linux/alloc_hook.cpp"
    "${OP_VST3_LINUX_DIR}/new_bridge.cpp"
    "${OP_SDK_SOURCE}/common/memorystream.cpp"
    "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp"
    "${OP_SDK_SOURCE}/vst/hosting/eventlist.cpp")
  add_dependencies(VST3EventsProof OpenPluginVST3Events OpenPluginVST3Synth OpenPluginVST3Notes)
  add_dependencies(VST3NoAllocProof OpenPluginVST3Events)
  add_test(NAME vst3_noalloc_events_proof COMMAND VST3NoAllocProof "${OP_VST3_DIR_OpenPluginVST3Events}")
  add_test(NAME vst3_events_proof COMMAND VST3EventsProof
    "${OP_VST3_DIR_OpenPluginVST3Events}" "${OP_VST3_DIR_OpenPluginVST3Synth}" "${OP_VST3_DIR_OpenPluginVST3Notes}")

  # Per-note expression proof (ABI 7): the shell over Expression Synth, note ids and
  # kNoteExpressionValueEvent in, INoteExpressionController read back. It counts with
  # its own operator new, like the events proof.
  if(OP_EXPRESSION_PROOFS)
    op_vst3_module(OpenPluginVST3Expression "Expression Synth")
    op_link_rust(OpenPluginVST3Expression "${OP_EXPRESSION_LIB}")
    op_vst3_proof(VST3ExpressionProof expression_host.cpp
      "${OP_NATIVE_DIR}/linux/alloc_hook.cpp"
      "${OP_VST3_LINUX_DIR}/new_bridge.cpp"
      "${OP_SDK_SOURCE}/vst/hosting/eventlist.cpp")
    add_dependencies(VST3ExpressionProof OpenPluginVST3Expression)
    add_test(NAME vst3_expression_proof COMMAND VST3ExpressionProof "${OP_VST3_DIR_OpenPluginVST3Expression}")
  endif()

  # Channel order proof: the shell over Role Tags, every layout from mono to 9.1.6.
  if(OP_ROLE_PROOFS)
    op_vst3_module(OpenPluginVST3Roles "Role Tags")
    op_link_rust(OpenPluginVST3Roles "${OP_ROLE_LIB}")
    op_vst3_proof(VST3RolesProof roles_host.cpp)
    add_dependencies(VST3RolesProof OpenPluginVST3Roles)
    add_test(NAME vst3_roles_proof COMMAND VST3RolesProof "${OP_VST3_DIR_OpenPluginVST3Roles}")
  endif()
endif()

if(OP_EDITOR)
  # The editor in the VST3 shell, in a real window on the X display, driven from the
  # proof's run loop. OP_EDITOR_PROOF_CLICK is "x;y;param": a point (editor points
  # from the top left) whose click edits that parameter. The proof is an X client of
  # its own, through libxcb (libxcb1-dev, libxcb on Arch).
  set(OP_EDITOR_PROOF_CLICK "" CACHE STRING "x;y;param for VST3EditorProof, or empty")
  find_package(PkgConfig REQUIRED)
  pkg_check_modules(OP_XCB REQUIRED IMPORTED_TARGET xcb)
  add_executable(VST3EditorProof "${OP_VST3_LINUX_DIR}/editor_host.cpp"
    "${OP_SDK_SOURCE}/vst/vstinitiids.cpp")
  op_vst3_settings(VST3EditorProof)
  target_link_libraries(VST3EditorProof PRIVATE sdk OpenPluginLinuxCompat PkgConfig::OP_XCB)
  add_dependencies(VST3EditorProof OpenPluginVST3)
  add_test(NAME vst3_editor_proof COMMAND VST3EditorProof "${OP_VST3_DIR_OpenPluginVST3}" ${OP_EDITOR_PROOF_CLICK})
endif()
