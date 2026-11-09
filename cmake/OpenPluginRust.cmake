# Build a Rust static library the way RadarVU and Seven Halls do, then let
# a JUCE target link it. One crate, one plugin, the symbols in openplugin.h.
#
#   include(OpenPluginRust.cmake)
#   openplugin_rust_staticlib(
#       TARGET my_plugin_dsp
#       MANIFEST ${CMAKE_SOURCE_DIR}/Cargo.toml
#       CRATE my_plugin_ffi)
#   add_dependencies(MyPlugin my_plugin_dsp)
#   target_link_libraries(MyPlugin PRIVATE ${MY_PLUGIN_DSP_LIB})
#   target_include_directories(MyPlugin PRIVATE path/to/openplugin-ffi/include)

function(openplugin_rust_staticlib)
    cmake_parse_arguments(ARG "" "TARGET;MANIFEST;CRATE" "" ${ARGN})
    if(NOT ARG_TARGET OR NOT ARG_MANIFEST OR NOT ARG_CRATE)
        message(FATAL_ERROR "openplugin_rust_staticlib needs TARGET, MANIFEST and CRATE")
    endif()
    find_program(OPENPLUGIN_CARGO cargo HINTS "$ENV{HOME}/.cargo/bin" "/opt/homebrew/bin" "/usr/local/bin")
    if(NOT OPENPLUGIN_CARGO)
        message(FATAL_ERROR "cargo is required to build ${ARG_CRATE}")
    endif()
    if(WIN32)
        set(_lib_name ${ARG_CRATE}.lib)
    else()
        set(_lib_name lib${ARG_CRATE}.a)
    endif()
    set(_out ${CMAKE_CURRENT_BINARY_DIR}/${_lib_name})
    set(_target_dir ${CMAKE_CURRENT_BINARY_DIR}/${ARG_TARGET}-target)
    if(APPLE AND CMAKE_OSX_ARCHITECTURES MATCHES "arm64" AND CMAKE_OSX_ARCHITECTURES MATCHES "x86_64")
        add_custom_command(
            OUTPUT ${_out}
            COMMAND ${OPENPLUGIN_CARGO} build --release -p ${ARG_CRATE}
                    --target aarch64-apple-darwin --manifest-path ${ARG_MANIFEST}
                    --target-dir ${_target_dir}
            COMMAND ${OPENPLUGIN_CARGO} build --release -p ${ARG_CRATE}
                    --target x86_64-apple-darwin --manifest-path ${ARG_MANIFEST}
                    --target-dir ${_target_dir}
            COMMAND lipo -create -output ${_out}
                    ${_target_dir}/aarch64-apple-darwin/release/${_lib_name}
                    ${_target_dir}/x86_64-apple-darwin/release/${_lib_name}
            DEPENDS ${ARG_MANIFEST}
            COMMENT "Building ${ARG_CRATE} (arm64 + x86_64)"
            VERBATIM)
    else()
        add_custom_command(
            OUTPUT ${_out}
            COMMAND ${OPENPLUGIN_CARGO} build --release -p ${ARG_CRATE}
                    --manifest-path ${ARG_MANIFEST} --target-dir ${_target_dir}
            COMMAND ${CMAKE_COMMAND} -E copy ${_target_dir}/release/${_lib_name} ${_out}
            DEPENDS ${ARG_MANIFEST}
            COMMENT "Building ${ARG_CRATE}"
            VERBATIM)
    endif()
    add_custom_target(${ARG_TARGET} DEPENDS ${_out})
    string(TOUPPER ${ARG_TARGET} _upper)
    set(${_upper}_LIB ${_out} PARENT_SCOPE)
endfunction()
