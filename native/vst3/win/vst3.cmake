# SPDX-License-Identifier: MIT
# The VST3 shell and its proofs on Windows (MSVC). Included by native/cmake/windows.cmake,
# which holds the options, the Rust library and op_link_rust, and explains how to build.
#
# What differs from the macOS build in native/CMakeLists.txt:
#   - A module is `<Name>.vst3\Contents\<arch>-win\<Name>.vst3` (x86_64-win,
#     arm64-win, arm64ec-win) in the build folder, with the VST3 SDK licence in
#     `Contents\Resources`. There is no Info.plist, PkgInfo or code signature.
#   - The entry points are InitDll and ExitDll from the SDK's dllmain.cpp.
#   - The proofs are the macOS sources unchanged. vst3/win/compat stands in for the
#     CoreFoundation and unistd calls they make, win/compat for the pthread ones,
#     win/alloc_hook.cpp for libmalloc's malloc_logger.
#   - The settings are given to each target here, so they reach neither the VST3
#     SDK's own targets nor another format's.
set(OP_VST3_WIN_DIR "${CMAKE_CURRENT_LIST_DIR}")

# The folder a VST3 module sits in, named after the machine it runs on.
if(CMAKE_CXX_COMPILER_ARCHITECTURE_ID STREQUAL "ARM64EC")
  set(OP_VST3_ARCH "arm64ec-win")
elseif(CMAKE_CXX_COMPILER_ARCHITECTURE_ID MATCHES "^ARM64")
  set(OP_VST3_ARCH "arm64-win")
elseif(CMAKE_CXX_COMPILER_ARCHITECTURE_ID STREQUAL "x64")
  set(OP_VST3_ARCH "x86_64-win")
else()
  message(FATAL_ERROR "The shells are built for x64 or ARM64, not ${CMAKE_CXX_COMPILER_ARCHITECTURE_ID}")
endif()

if(NOT EXISTS "${OP_VST3_SDK}/public.sdk/source/main/dllmain.cpp")
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
# The SDK's own CMake names the C runtime per configuration (/MDd for Debug, /MD
# otherwise). Its libraries are linked into the shells, which use the release static
# runtime (/MT) in every configuration (native/cmake/windows.cmake), so that choice
# is taken off them and they get the shells' runtime.
foreach(sdk_target IN ITEMS base pluginterfaces sdk sdk_common sdk_hosting)
  if(TARGET ${sdk_target})
    get_target_property(sdk_options ${sdk_target} COMPILE_OPTIONS)
    if(sdk_options)
      list(FILTER sdk_options EXCLUDE REGEX "/M[DT]d?>$")
      set_property(TARGET ${sdk_target} PROPERTY COMPILE_OPTIONS "${sdk_options}")
    endif()
    get_target_property(sdk_type ${sdk_target} TYPE)
    if(NOT sdk_type STREQUAL "INTERFACE_LIBRARY")
      set_property(TARGET ${sdk_target} PROPERTY MSVC_RUNTIME_LIBRARY "${OP_MSVC_RUNTIME}")
    endif()
  endif()
endforeach()

# UNICODE: dllmain.cpp reads the module path as wide characters. _USE_MATH_DEFINES:
# the proofs use M_PI. The sources are UTF-8 and carry text outside ASCII.
set(OP_VST3_DEFINITIONS UNICODE _UNICODE NOMINMAX _USE_MATH_DEFINES _CRT_SECURE_NO_WARNINGS _CRT_NONSTDC_NO_WARNINGS)
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
  target_compile_options(${target_name} PRIVATE /utf-8 /W3 /Zc:__cplusplus)
endfunction()

# What the proofs and the stand-in plugins use in place of CoreFoundation and pthread.
add_library(OpenPluginWinCompat STATIC "${OP_VST3_WIN_DIR}/cf_compat.cpp")
op_vst3_settings(OpenPluginWinCompat)
target_include_directories(OpenPluginWinCompat PUBLIC "${OP_VST3_WIN_DIR}/compat" "${OP_NATIVE_DIR}/win/compat")
target_link_libraries(OpenPluginWinCompat PUBLIC user32)

# The stand-in plugins export what a proof looks up with the GCC spelling.
set_source_files_properties(
  "${OP_NATIVE_DIR}/vst3/latency_stub.cpp" "${OP_NATIVE_DIR}/vst3/state_stub.cpp"
  "${OP_NATIVE_DIR}/vst3/track_stub.cpp" "${OP_NATIVE_DIR}/vst3/changes_stub.cpp"
  "${OP_NATIVE_DIR}/vst3/events_stub.cpp"
  PROPERTIES COMPILE_OPTIONS "/FI${OP_VST3_WIN_DIR}/compat/stub_export.h")

# A VST3 module `<name>.vst3\Contents\<arch>-win\<name>.vst3` in the build folder, from the
# shell and the extra sources given (a stand-in plugin). OP_VST3_DIR_<target> is the folder.
function(op_vst3_module target_name product_name)
  set(bundle_dir "${CMAKE_BINARY_DIR}/${product_name}.vst3")
  set(OP_VST3_DIR_${target_name} "${bundle_dir}" PARENT_SCOPE)
  add_library(${target_name} MODULE
    "${OP_VST3_WIN_DIR}/OpenPluginVST3Win.cpp"
    ${ARGN}
    "${OP_VST3_SDK}/public.sdk/source/main/dllmain.cpp")
  op_vst3_settings(${target_name})
  target_include_directories(${target_name} PRIVATE "${OP_NATIVE_DIR}/include" "${OP_NATIVE_DIR}/vst3")
  # $<1:...> keeps a Visual Studio build from adding a folder per configuration.
  set_target_properties(${target_name} PROPERTIES
    OUTPUT_NAME "${product_name}"
    PREFIX ""
    SUFFIX ".vst3"
    LIBRARY_OUTPUT_DIRECTORY "$<1:${bundle_dir}/Contents/${OP_VST3_ARCH}>"
    PDB_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/pdb/${target_name}>"
    ARCHIVE_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/implib/${target_name}>")
  target_link_libraries(${target_name} PRIVATE sdk)
  if(OP_EDITOR)
    # The view's sizes in pixels (vst3/win/editor_pixels.h).
    target_sources(${target_name} PRIVATE "${OP_VST3_WIN_DIR}/editor_pixels.cpp")
    target_link_libraries(${target_name} PRIVATE user32)
  endif()
  if(ARGN)
    target_link_libraries(${target_name} PRIVATE OpenPluginWinCompat)
  endif()
  op_version_resource(${target_name} "${product_name}" "${product_name} (VST3)" "${product_name}.vst3")
  add_custom_command(TARGET ${target_name} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "${bundle_dir}/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_VST3_SDK}/LICENSE.txt" "${bundle_dir}/Contents/Resources/VST3-SDK-LICENSE.txt")
endfunction()

# A proof executable from a macOS proof source and the SDK hosting sources given.
function(op_vst3_proof target_name source)
  add_executable(${target_name} "${OP_NATIVE_DIR}/vst3/${source}" ${ARGN})
  op_vst3_settings(${target_name})
  target_include_directories(${target_name} PRIVATE "${OP_NATIVE_DIR}/include" "${OP_NATIVE_DIR}/vst3")
  target_link_libraries(${target_name} PRIVATE sdk OpenPluginWinCompat)
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
# process() must not allocate: counted through operator new and the heap functions
# every allocation in the process ends in (win/alloc_hook.cpp).
op_vst3_proof(VST3NoAllocProof noalloc.cpp
  "${OP_NATIVE_DIR}/win/alloc_hook.cpp"
  "${OP_VST3_WIN_DIR}/hook_on_load.cpp"
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
  # The proof counts with its own operator new, which a Windows module does not share:
  # vst3/win/new_bridge.cpp brings every heap allocation in the process to it.
  op_vst3_proof(VST3EventsProof events_host.cpp
    "${OP_NATIVE_DIR}/win/alloc_hook.cpp"
    "${OP_VST3_WIN_DIR}/hook_on_load.cpp"
    "${OP_VST3_WIN_DIR}/new_bridge.cpp"
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
      "${OP_NATIVE_DIR}/win/alloc_hook.cpp"
    "${OP_VST3_WIN_DIR}/hook_on_load.cpp"
      "${OP_VST3_WIN_DIR}/new_bridge.cpp"
      "${OP_SDK_SOURCE}/vst/hosting/eventlist.cpp")
    add_dependencies(VST3ExpressionProof OpenPluginVST3Expression)
    add_test(NAME vst3_expression_proof COMMAND VST3ExpressionProof "${OP_VST3_DIR_OpenPluginVST3Expression}")
  endif()

  # Identity pinning proof (ABI 8): the shell over the four builds of Pinned. A project
  # saved with one pinned build reopens in the reordered one; the builds without pins are
  # the negative control.
  if(OP_IDENTITY_PROOFS)
    op_vst3_module(OpenPluginVST3PinnedA "Pinned A")
    op_link_rust(OpenPluginVST3PinnedA "${OP_IDENTITY_LIB_a}")
    op_vst3_module(OpenPluginVST3PinnedB "Pinned B")
    op_link_rust(OpenPluginVST3PinnedB "${OP_IDENTITY_LIB_b}")
    op_vst3_module(OpenPluginVST3PlainA "Plain A")
    op_link_rust(OpenPluginVST3PlainA "${OP_IDENTITY_LIB_plain_a}")
    op_vst3_module(OpenPluginVST3PlainB "Plain B")
    op_link_rust(OpenPluginVST3PlainB "${OP_IDENTITY_LIB_plain_b}")
    op_vst3_proof(VST3IdentityProof identity_host.cpp
      "${OP_SDK_SOURCE}/common/memorystream.cpp"
      "${OP_SDK_SOURCE}/vst/hosting/parameterchanges.cpp")
    add_dependencies(VST3IdentityProof OpenPluginVST3PinnedA OpenPluginVST3PinnedB OpenPluginVST3PlainA OpenPluginVST3PlainB)
    add_test(NAME vst3_identity_proof COMMAND VST3IdentityProof
      "${OP_VST3_DIR_OpenPluginVST3PinnedA}" "${OP_VST3_DIR_OpenPluginVST3PinnedB}"
      "${OP_VST3_DIR_OpenPluginVST3PlainA}" "${OP_VST3_DIR_OpenPluginVST3PlainB}")
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
  # The editor in the VST3 shell, in a real window. OP_EDITOR_PROOF_CLICK is "x;y;param":
  # a point (editor points from the top left) whose click edits that parameter.
  set(OP_EDITOR_PROOF_CLICK "" CACHE STRING "x;y;param for VST3EditorProof, or empty")
  add_executable(VST3EditorProof "${OP_VST3_WIN_DIR}/editor_host.cpp"
    "${OP_SDK_SOURCE}/vst/vstinitiids.cpp")
  op_vst3_settings(VST3EditorProof)
  target_link_libraries(VST3EditorProof PRIVATE sdk user32)
  add_dependencies(VST3EditorProof OpenPluginVST3)
  add_test(NAME vst3_editor_proof COMMAND VST3EditorProof "${OP_VST3_DIR_OpenPluginVST3}" ${OP_EDITOR_PROOF_CLICK})
endif()
