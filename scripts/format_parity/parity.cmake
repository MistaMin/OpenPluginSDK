# SPDX-License-Identifier: MIT
# Format parity hosts (docs/FORMAT_PARITY.md). Included by native/CMakeLists.txt
# when -DOP_FORMAT_PARITY=ON, after the shells are defined, so it can reuse
# their SDK targets and op_link_rust. Test builds only; nothing here is part of
# a shipped bundle.
#
# The *Fault targets are the negative control: the shell's own sources,
# unmodified, compiled with op_shell_process renamed to parity_faulty_process
# (native/parity_fault.cpp), which scales the output by 1 + OP_PARITY_FAULT_SCALE.
set(OP_PARITY_DIR "${CMAKE_CURRENT_LIST_DIR}")
set(OP_PARITY_FAULT_SCALE "1e-4" CACHE STRING "Output error of the negative-control shells (relative)")
set(OP_PARITY_FAULT_DEFS "op_shell_process=parity_faulty_process")
set_source_files_properties("${OP_PARITY_DIR}/native/parity_fault.cpp" PROPERTIES
  COMPILE_DEFINITIONS "PARITY_FAULT_SCALE=${OP_PARITY_FAULT_SCALE}f")
set_source_files_properties("${OP_PARITY_DIR}/native/parity_fault_events.cpp" PROPERTIES
  COMPILE_DEFINITIONS "PARITY_FAULT_SCALE=${OP_PARITY_FAULT_SCALE}f")

if(OP_BUILD_VST3)
  add_executable(ParityVST3 "${OP_PARITY_DIR}/native/vst3_parity.cpp"
    "${OP_VST3_SDK}/public.sdk/source/vst/hosting/parameterchanges.cpp")
  target_include_directories(ParityVST3 PRIVATE "${OP_PARITY_DIR}/native")
  target_link_libraries(ParityVST3 PRIVATE sdk "-framework CoreFoundation")
  add_dependencies(ParityVST3 OpenPluginVST3)

  set(OP_PRODUCT_SAVED "${OP_PRODUCT}")
  set(OP_PRODUCT "Parity Fault")
  configure_file(vst3/Info.plist.in "${CMAKE_BINARY_DIR}/vst3-parity-fault-Info.plist" @ONLY)
  set(OP_PRODUCT "${OP_PRODUCT_SAVED}")
  add_library(OpenPluginVST3Fault MODULE
    vst3/OpenPluginVST3.cpp
    "${OP_PARITY_DIR}/native/parity_fault_events.cpp"
    "${OP_VST3_SDK}/public.sdk/source/main/macmain.cpp")
  target_include_directories(OpenPluginVST3Fault PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include")
  target_compile_definitions(OpenPluginVST3Fault PRIVATE "op_shell_process_events=parity_faulty_process_events")
  set_target_properties(OpenPluginVST3Fault PROPERTIES
    OUTPUT_NAME "Parity Fault"
    BUNDLE TRUE
    BUNDLE_EXTENSION vst3
    PREFIX ""
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/vst3-parity-fault-Info.plist")
  target_link_libraries(OpenPluginVST3Fault PRIVATE sdk)
  op_link_rust(OpenPluginVST3Fault)
  add_custom_command(TARGET OpenPluginVST3Fault POST_BUILD
    COMMAND codesign --force --sign - "$<TARGET_BUNDLE_DIR:OpenPluginVST3Fault>")
endif()

if(OP_BUILD_AU)
  add_executable(ParityAU "${OP_PARITY_DIR}/native/au_parity.cpp")
  target_include_directories(ParityAU PRIVATE "${OP_PARITY_DIR}/native" "${CMAKE_CURRENT_SOURCE_DIR}/include" "${CMAKE_CURRENT_SOURCE_DIR}/au")
  target_link_libraries(ParityAU PRIVATE "-framework AudioToolbox" "-framework CoreFoundation")
  add_dependencies(ParityAU OpenPluginAU)

  set(OP_PARITY_SAVED_PRODUCT "${OP_PRODUCT}")
  set(OP_PARITY_SAVED_ID "${OP_BUNDLE_ID}")
  set(OP_PARITY_SAVED_SUB "${OP_AU_SUBTYPE}")
  set(OP_PARITY_SAVED_MAN "${OP_AU_MANUFACTURER}")
  set(OP_PRODUCT "ParityFault")
  set(OP_BUNDLE_ID "com.opengrid.test.parityfault")
  set(OP_AU_SUBTYPE "PfFt")
  set(OP_AU_MANUFACTURER "OpTs")
  configure_file(au/Info.plist.in "${CMAKE_BINARY_DIR}/au-parity-fault-Info.plist" @ONLY)
  set(OP_PRODUCT "${OP_PARITY_SAVED_PRODUCT}")
  set(OP_BUNDLE_ID "${OP_PARITY_SAVED_ID}")
  set(OP_AU_SUBTYPE "${OP_PARITY_SAVED_SUB}")
  set(OP_AU_MANUFACTURER "${OP_PARITY_SAVED_MAN}")
  add_library(OpenPluginAUFault MODULE au/OpenPluginAU.cpp "${OP_PARITY_DIR}/native/parity_fault.cpp")
  target_compile_features(OpenPluginAUFault PRIVATE cxx_std_23)
  target_include_directories(OpenPluginAUFault PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/include" "${OP_AU_SDK}/include")
  target_compile_definitions(OpenPluginAUFault PRIVATE ${OP_PARITY_FAULT_DEFS})
  set_target_properties(OpenPluginAUFault PROPERTIES
    OUTPUT_NAME "ParityFault"
    BUNDLE TRUE
    BUNDLE_EXTENSION component
    PREFIX ""
    MACOSX_BUNDLE_INFO_PLIST "${CMAKE_BINARY_DIR}/au-parity-fault-Info.plist")
  target_link_libraries(OpenPluginAUFault PRIVATE openplugin_au_sdk)
  op_link_rust(OpenPluginAUFault)
  add_custom_command(TARGET OpenPluginAUFault POST_BUILD
    COMMAND codesign --force --sign - "$<TARGET_BUNDLE_DIR:OpenPluginAUFault>")
endif()

if(OP_BUILD_AAX)
  set(OP_PARITY_AAX_INCLUDES
    "${OP_PARITY_DIR}/native"
    "${CMAKE_CURRENT_SOURCE_DIR}/include"
    "${CMAKE_CURRENT_SOURCE_DIR}/aax"
    "${OP_AAX_SDK}/Interfaces"
    "${OP_AAX_SDK}/Interfaces/ACF")
  add_executable(ParityAAX "${OP_PARITY_DIR}/native/aax_parity.cpp" aax/OpenPluginAAXProcess.cpp)
  target_include_directories(ParityAAX PRIVATE ${OP_PARITY_AAX_INCLUDES})
  target_link_libraries(ParityAAX PRIVATE openplugin_aax_sdk)
  op_link_rust(ParityAAX)
  # The AAX shell's process callback is compiled straight into the host, so the
  # fault build is the same host over the same source with the rename.
  add_executable(ParityAAXFault "${OP_PARITY_DIR}/native/aax_parity.cpp" aax/OpenPluginAAXProcess.cpp
    "${OP_PARITY_DIR}/native/parity_fault.cpp")
  target_include_directories(ParityAAXFault PRIVATE ${OP_PARITY_AAX_INCLUDES})
  target_compile_definitions(ParityAAXFault PRIVATE ${OP_PARITY_FAULT_DEFS})
  target_link_libraries(ParityAAXFault PRIVATE openplugin_aax_sdk)
  op_link_rust(ParityAAXFault)
endif()
