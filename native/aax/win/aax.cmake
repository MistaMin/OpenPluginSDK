# SPDX-License-Identifier: MIT
# The AAX shell and its proofs on Windows (MSVC, x64). Included by the Windows
# configuration (native/cmake/windows.cmake) after project(); it needs OP_NATIVE_DIR,
# OP_RUST_LIB, op_link_rust, OP_ROLE_PROOFS and OP_ROLE_LIB from there, and sets its
# compiler flags on its own targets only.
#
# The same sources and proofs as on macOS (the `if(OP_BUILD_AAX)` block of
# native/CMakeLists.txt). What differs:
#   - The plug-in is `<Name>.aaxplugin\Contents\x64\<Name>.aaxplugin`, a DLL, in the
#     build folder, with the AAX SDK licence (and the page table, with OP_AAX_PAGES)
#     in `Contents\Resources`. There is no Info.plist and no code signature; Pro
#     Tools loads it only after PACE signing, which this build does not do.
#   - The AAX library is built from the SDK's sources with its Windows files
#     (AAX_CAutoreleasePool.Win.cpp) in place of the macOS `.mm`.
#   - The seven ACF entry points are exported by the SDK's AAX_Exports.cpp
#     (`__declspec(dllexport)`, `__stdcall`, which x64 does not decorate), so there
#     is no .def file.
#   - With OP_EDITOR the view is aax/win/OpenPluginAAXGUIWin.cpp (an HWND).
#   - win/alloc_hook.cpp stands in for libmalloc's malloc_logger in the
#     allocation proofs, and aax/win/host_bundle.cpp for the CoreFoundation loader.
if(NOT OP_BUILD_AAX)
  return()
endif()
if(NOT EXISTS "${OP_AAX_SDK}/Interfaces/AAX.h")
  message(FATAL_ERROR "OP_AAX_SDK must point at an existing AAX SDK")
endif()
if(NOT CMAKE_CXX_COMPILER_ARCHITECTURE_ID STREQUAL "x64")
  message(FATAL_ERROR "AAX on Windows is x64 only, not ${CMAKE_CXX_COMPILER_ARCHITECTURE_ID}")
endif()

set(OP_AAX_DIR "${OP_NATIVE_DIR}/aax")
set(OP_AAX_WIN_DIR "${CMAKE_CURRENT_LIST_DIR}")
set(OP_AAX_PROOF_INCLUDES
  "${OP_NATIVE_DIR}/include"
  "${OP_AAX_DIR}"
  "${OP_AAX_SDK}/Interfaces"
  "${OP_AAX_SDK}/Interfaces/ACF"
  "${OP_AAX_SDK}/Libs/AAXLibrary/include")
set(OP_AAX_DEFINITIONS UNICODE _UNICODE NOMINMAX _USE_MATH_DEFINES _CRT_SECURE_NO_WARNINGS)
if(OP_TAIL)
  list(APPEND OP_AAX_DEFINITIONS OP_HAS_TAIL=1)
else()
  list(APPEND OP_AAX_DEFINITIONS OP_HAS_TAIL=0)
endif()
if(OP_EDITOR)
  # The shell uses the size range and resize calls of op_editor.h.
  list(APPEND OP_AAX_DEFINITIONS OP_HAS_EDITOR=1 OP_EDITOR_RESIZE=1)
else()
  list(APPEND OP_AAX_DEFINITIONS OP_HAS_EDITOR=0)
endif()

# Includes, definitions and flags for a target built from the shell or proof sources.
# The sources are UTF-8 and carry text outside ASCII.
function(op_aax_settings target_name)
  target_include_directories(${target_name} PRIVATE ${OP_AAX_PROOF_INCLUDES})
  target_compile_definitions(${target_name} PRIVATE ${OP_AAX_DEFINITIONS})
  target_compile_options(${target_name} PRIVATE /utf-8 /W3 /MP)
endfunction()

# The AAX library, from the SDK's sources where they are.
file(GLOB OP_AAX_SDK_CPP CONFIGURE_DEPENDS "${OP_AAX_SDK}/Libs/AAXLibrary/source/*.cpp")
add_library(openplugin_aax_sdk STATIC ${OP_AAX_SDK_CPP})
target_include_directories(openplugin_aax_sdk PUBLIC
  "${OP_AAX_SDK}/Interfaces"
  "${OP_AAX_SDK}/Interfaces/ACF"
  "${OP_AAX_SDK}/Libs/AAXLibrary/include")
# What the SDK's own Visual Studio project defines for the library.
target_compile_definitions(openplugin_aax_sdk PRIVATE
  UNICODE _UNICODE NOMINMAX _WINDOWS I18N_LIB PLUGIN_SDK_BUILD AAX_PLUGIN_BUILD
  _CRT_SECURE_NO_DEPRECATE _SCL_SECURE_NO_DEPRECATE)
target_compile_options(openplugin_aax_sdk PRIVATE /utf-8 /MP)

# The plug-in: <Name>.aaxplugin\Contents\x64\<Name>.aaxplugin.
set(OP_AAX_BUNDLE_DIR "${CMAKE_BINARY_DIR}/${OP_PRODUCT}.aaxplugin")
if(OP_EDITOR)
  set(OP_AAX_GUI_SOURCE "${OP_AAX_WIN_DIR}/OpenPluginAAXGUIWin.cpp")
else()
  set(OP_AAX_GUI_SOURCE "${OP_AAX_DIR}/OpenPluginAAXGUI.cpp")
endif()
add_library(OpenPluginAAX MODULE
  "${OP_AAX_DIR}/OpenPluginAAXDescribe.cpp"
  "${OP_AAX_DIR}/OpenPluginAAXParameters.cpp"
  "${OP_AAX_DIR}/OpenPluginAAXState.cpp"
  "${OP_AAX_DIR}/OpenPluginAAXProcess.cpp"
  "${OP_AAX_GUI_SOURCE}"
  "${OP_AAX_SDK}/Interfaces/AAX_Exports.cpp")
op_version_resource(OpenPluginAAX "${OP_PRODUCT}" "${OP_PRODUCT} (AAX)" "${OP_PRODUCT}.aaxplugin")
op_aax_settings(OpenPluginAAX)
# $<1:...> keeps a Visual Studio build from adding a folder per configuration.
set_target_properties(OpenPluginAAX PROPERTIES
  OUTPUT_NAME "${OP_PRODUCT}"
  PREFIX ""
  SUFFIX ".aaxplugin"
  LIBRARY_OUTPUT_DIRECTORY "$<1:${OP_AAX_BUNDLE_DIR}/Contents/x64>"
  PDB_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/pdb/OpenPluginAAX>"
  ARCHIVE_OUTPUT_DIRECTORY "$<1:${CMAKE_BINARY_DIR}/implib/OpenPluginAAX>")
target_link_libraries(OpenPluginAAX PRIVATE openplugin_aax_sdk)
op_link_rust(OpenPluginAAX)
add_custom_command(TARGET OpenPluginAAX POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "${OP_AAX_BUNDLE_DIR}/Contents/Resources"
  COMMAND ${CMAKE_COMMAND} -E copy "${OP_AAX_SDK}/LICENSE.txt" "${OP_AAX_BUNDLE_DIR}/Contents/Resources/AAX-SDK-LICENSE.txt")
if(OP_AAX_PAGES)
  # The tool links the plugin, asks it for its parameters and writes the table.
  add_executable(OpenPluginAAXPages "${OP_AAX_DIR}/pages.cpp")
  op_aax_settings(OpenPluginAAXPages)
  op_link_rust(OpenPluginAAXPages)
  add_dependencies(OpenPluginAAX OpenPluginAAXPages)
  target_compile_definitions(OpenPluginAAX PRIVATE "OP_AAX_PAGE_TABLE=\"${OP_PRODUCT}Pages.xml\"")
  add_custom_command(TARGET OpenPluginAAX POST_BUILD
    COMMAND OpenPluginAAXPages "${OP_AAX_BUNDLE_DIR}/Contents/Resources/${OP_PRODUCT}Pages.xml")
endif()

# A proof executable from sources in native/aax (or full paths).
function(op_aax_proof target_name)
  set(sources "")
  foreach(source IN LISTS ARGN)
    if(IS_ABSOLUTE "${source}")
      list(APPEND sources "${source}")
    else()
      list(APPEND sources "${OP_AAX_DIR}/${source}")
    endif()
  endforeach()
  add_executable(${target_name} ${sources})
  op_aax_settings(${target_name})
  # <pthread.h> for the allocation proofs (native/win/compat).
  target_include_directories(${target_name} PRIVATE "${OP_NATIVE_DIR}/win/compat")
endfunction()

op_aax_proof(AAXProof host_process.cpp OpenPluginAAXProcess.cpp)
target_link_libraries(AAXProof PRIVATE openplugin_aax_sdk)
op_link_rust(AAXProof)
# State chunk proof: the shell's real AAX_CEffectParameters, no controller.
op_aax_proof(AAXStateProof host_state.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp OpenPluginAAXProcess.cpp)
target_link_libraries(AAXStateProof PRIVATE openplugin_aax_sdk)
op_link_rust(AAXStateProof)
# Allocation proof: replaces operator new and counts the heap calls of the whole
# process (win/alloc_hook.cpp), so it stays a separate binary.
op_aax_proof(AAXAllocProof host_alloc.cpp OpenPluginAAXProcess.cpp "${OP_NATIVE_DIR}/win/alloc_hook.cpp")
op_link_rust(AAXAllocProof)
# Stand-in ABI proofs (aax/proof_stub.cpp, no Rust library), as on macOS.
op_aax_proof(AAXBypassProof host_bypass.cpp OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXBypassProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=37)
op_aax_proof(AAXParamsProof host_params.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp
  OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXParamsProof PRIVATE OP_STUB_PARAMS=200 OP_STUB_LATENCY=37)
target_link_libraries(AAXParamsProof PRIVATE openplugin_aax_sdk)
op_aax_proof(AAXStateBlobProof host_state_blob.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp
  OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXStateBlobProof PRIVATE OP_STUB_PARAMS=3 OP_STUB_LATENCY=37)
target_link_libraries(AAXStateBlobProof PRIVATE openplugin_aax_sdk)
op_aax_proof(AAXAllocLatencyProof host_alloc.cpp OpenPluginAAXProcess.cpp proof_stub.cpp "${OP_NATIVE_DIR}/win/alloc_hook.cpp")
target_compile_definitions(AAXAllocLatencyProof PRIVATE OP_STUB_PARAMS=200 OP_STUB_LATENCY=37)
op_aax_proof(AAXEventsProof host_events.cpp OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXEventsProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=0)
target_link_libraries(AAXEventsProof PRIVATE openplugin_aax_sdk)
op_aax_proof(AAXDescribeProof host_describe.cpp OpenPluginAAXDescribe.cpp OpenPluginAAXParameters.cpp
  OpenPluginAAXState.cpp OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXDescribeProof PRIVATE OP_STUB_PARAMS=1 OP_STUB_LATENCY=0)
target_link_libraries(AAXDescribeProof PRIVATE openplugin_aax_sdk)
op_aax_proof(AAXTextProof host_text.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp OpenPluginAAXProcess.cpp)
target_link_libraries(AAXTextProof PRIVATE openplugin_aax_sdk)
op_link_rust(AAXTextProof)
op_aax_proof(AAXTextStubProof host_text.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp
  OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXTextStubProof PRIVATE OP_STUB_PARAMS=4 OP_STUB_LATENCY=0 OP_STUB_TEXT=1 OP_TEXT_STUB=1)
target_link_libraries(AAXTextStubProof PRIVATE openplugin_aax_sdk)
# Channel order proof: the process callback over Role Tags, every layout from mono to 9.1.6.
if(OP_ROLE_PROOFS)
  op_aax_proof(AAXRolesProof host_roles.cpp OpenPluginAAXProcess.cpp)
  target_link_libraries(AAXRolesProof PRIVATE openplugin_aax_sdk)
  op_link_rust(AAXRolesProof "${OP_ROLE_LIB}")
  add_test(NAME aax_roles_proof COMMAND AAXRolesProof)
endif()
op_aax_proof(AAXChangesProof host_changes.cpp OpenPluginAAXParameters.cpp OpenPluginAAXState.cpp
  OpenPluginAAXProcess.cpp proof_stub.cpp)
target_compile_definitions(AAXChangesProof PRIVATE OP_STUB_PARAMS=3 OP_STUB_LATENCY=0)
target_link_libraries(AAXChangesProof PRIVATE openplugin_aax_sdk)
# Loads the built plug-in as Pro Tools would find it and looks up the ACF exports.
add_executable(AAXBundleProof "${OP_AAX_WIN_DIR}/host_bundle.cpp")
target_compile_options(AAXBundleProof PRIVATE /utf-8 /W3)
add_dependencies(AAXBundleProof OpenPluginAAX)
op_aax_proof(AAXPagesProof host_pages.cpp pages.cpp)
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
add_test(NAME aax_bundle_proof COMMAND AAXBundleProof "${OP_AAX_BUNDLE_DIR}")
