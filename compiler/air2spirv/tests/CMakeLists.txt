# Plain C++ tests without external frameworks (no network access needed).
# Every A2S_TEST(<case>) in a test source becomes its own ctest test named
# air2spirv.<area>.<case> (docs/process/testing.md §3.1). Cases are discovered by scanning the
# source at configure time; CMAKE_CONFIGURE_DEPENDS re-runs the scan when a source changes.

add_library(a2s_test_harness STATIC test_harness.cpp)
target_include_directories(a2s_test_harness PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(a2s_test_harness PUBLIC air2spirv_warnings air2spirv_sanitizers)

# FileCheck-style matcher (no dependencies) and SPIR-V test helpers (internal headers, SPIRV-Tools).
add_library(a2s_filecheck STATIC support/filecheck.cpp)
target_include_directories(a2s_filecheck PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/support)
target_link_libraries(a2s_filecheck PUBLIC air2spirv_warnings air2spirv_sanitizers)

add_library(a2s_spirv_test_support STATIC support/spirv_test_modules.cpp)
target_include_directories(a2s_spirv_test_support PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/support ${PROJECT_SOURCE_DIR}/src)
target_link_libraries(a2s_spirv_test_support PUBLIC
    air2spirv::air2spirv a2s_filecheck air2spirv_spirv_deps air2spirv_warnings
    air2spirv_sanitizers)

set(_a2s_test_env
    "ASAN_OPTIONS=detect_leaks=1:abort_on_error=0:strict_string_checks=1"
    "UBSAN_OPTIONS=print_stacktrace=1:halt_on_error=1")

# a2s_add_test_executable(<area> <source> [INTERNAL] [LIBS ...])
#   INTERNAL: the test may include private headers from src/.
function(a2s_add_test_executable area source)
    cmake_parse_arguments(ARG "INTERNAL" "" "LIBS" ${ARGN})
    set(target test_${area})
    add_executable(${target} ${source})
    target_link_libraries(${target} PRIVATE air2spirv::air2spirv a2s_test_harness ${ARG_LIBS})
    if(ARG_INTERNAL)
        target_include_directories(${target} PRIVATE ${PROJECT_SOURCE_DIR}/src)
        target_link_libraries(${target} PRIVATE air2spirv_spirv_deps)
    endif()

    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
        ${CMAKE_CURRENT_SOURCE_DIR}/${source})
    file(STRINGS ${source} _lines REGEX "^A2S_TEST\\(([A-Za-z0-9_]+)\\)")
    if(NOT _lines)
        message(FATAL_ERROR "${source}: no A2S_TEST cases found")
    endif()
    foreach(_line IN LISTS _lines)
        string(REGEX REPLACE "^A2S_TEST\\(([A-Za-z0-9_]+)\\).*" "\\1" _case "${_line}")
        add_test(NAME air2spirv.${area}.${_case} COMMAND ${target} ${_case})
        set_tests_properties(air2spirv.${area}.${_case} PROPERTIES
            ENVIRONMENT "${_a2s_test_env}" TIMEOUT 60)
    endforeach()
endfunction()

find_package(Threads REQUIRED)

a2s_add_test_executable(container test_container.cpp INTERNAL)
a2s_add_test_executable(c_api test_c_api.cpp LIBS Threads::Threads)
a2s_add_test_executable(filecheck test_filecheck.cpp LIBS a2s_filecheck)
a2s_add_test_executable(spirv_builder test_spirv_builder.cpp INTERNAL LIBS a2s_spirv_test_support)
a2s_add_test_executable(spirv_tools test_spirv_tools.cpp INTERNAL
    LIBS a2s_spirv_test_support Threads::Threads)

if(AIR2SPIRV_BUILD_TOOLS AND UNIX)
    a2s_add_test_executable(cli test_cli.cpp)
    target_compile_definitions(test_cli PRIVATE
        A2S_CLI_PATH="$<TARGET_FILE:air2spirv-cli>"
        A2S_TEST_TMPDIR="${CMAKE_CURRENT_BINARY_DIR}/cli-scratch")
    add_dependencies(test_cli air2spirv-cli)
endif()

# The public header must compile as strict C99 and be usable from a C translation unit.
add_executable(test_c_header c_header_test.c)
target_link_libraries(test_c_header PRIVATE air2spirv::air2spirv air2spirv_warnings)
set_target_properties(test_c_header PROPERTIES C_STANDARD 99 C_EXTENSIONS OFF
    LINKER_LANGUAGE CXX)
add_test(NAME air2spirv.c_header.compiles_and_links_as_c99 COMMAND test_c_header)
set_tests_properties(air2spirv.c_header.compiles_and_links_as_c99 PROPERTIES
    ENVIRONMENT "${_a2s_test_env}" TIMEOUT 60)
