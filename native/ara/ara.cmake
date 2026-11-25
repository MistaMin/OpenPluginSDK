# ARA 2 for a VST3 module on Windows and Linux (macOS sets it up in native/CMakeLists.txt):
# the shared glue and the ARA Library compiled in, the licence and notice into the bundle.
# op_ara_vst3(<target> <bundle dir>) does nothing unless OP_ARA_SDK names an ARA SDK checkout.
function(op_ara_vst3 target_name bundle_dir)
  if(NOT OP_ARA_SDK)
    return()
  endif()
  if(NOT EXISTS "${OP_ARA_SDK}/ARA_Library/PlugIn/ARAPlug.cpp")
    message(FATAL_ERROR "OP_ARA_SDK must point at an ARA SDK checkout (ARA_API, ARA_Library)")
  endif()
  target_sources(${target_name} PRIVATE
    "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ara_glue.cpp"
    "${OP_ARA_SDK}/ARA_Library/PlugIn/ARAPlug.cpp"
    "${OP_ARA_SDK}/ARA_Library/Dispatch/ARAPlugInDispatch.cpp"
    "${OP_ARA_SDK}/ARA_Library/Utilities/ARAChannelFormat.cpp"
    "${OP_ARA_SDK}/ARA_Library/Utilities/ARAPitchInterpretation.cpp"
    "${OP_ARA_SDK}/ARA_Library/Debug/ARADebug.c")
  target_include_directories(${target_name} PRIVATE "${OP_ARA_SDK}")
  target_compile_definitions(${target_name} PRIVATE OP_HAS_ARA=1 OP_ARA_VST3=1 ARA_VALIDATE_API_CALLS=0)
  if(WIN32)
    target_compile_definitions(${target_name} PRIVATE NOMINMAX=1)
  endif()
  add_custom_command(TARGET ${target_name} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E make_directory "${bundle_dir}/Contents/Resources"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_ARA_SDK}/ARA_Library/LICENSE.txt" "${bundle_dir}/Contents/Resources/ARA-SDK-LICENSE.txt"
    COMMAND ${CMAKE_COMMAND} -E copy "${OP_ARA_SDK}/NOTICE.txt" "${bundle_dir}/Contents/Resources/ARA-SDK-NOTICE.txt")
endfunction()
