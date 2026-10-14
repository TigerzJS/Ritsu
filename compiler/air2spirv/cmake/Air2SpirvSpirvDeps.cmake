# SPIRV-Headers + SPIRV-Tools (docs/subsystems/shader-compiler.md §2.6, §11.2; WP-SC-03).
#
# Both are REQUIRED: ADR-0007 makes `spirv-val` mandatory for every module air2spirv emits, so
# a build without the validator would silently drop that guarantee. There is deliberately no
# option to build without them.
#
# Source: system packages via their CMake package configs. On Ubuntu 24.04:
#     sudo apt-get install spirv-headers spirv-tools
# (Debian/Ubuntu ship the static SPIRV-Tools libraries, headers and CMake configs in the
# `spirv-tools` package itself; there is no separate -dev package.) A self-built or pinned
# installation can be selected with -DCMAKE_PREFIX_PATH=<prefix> or -DSPIRV-Headers_DIR=... /
# -DSPIRV-Tools_DIR=... / -DSPIRV-Tools-opt_DIR=....
#
# Results:
#   air2spirv_spirv_deps        INTERFACE target (headers + SPIRV-Tools-opt + SPIRV-Tools core)
#   A2S_SPIRV_TOOLS_PACKAGE     package identity of the linked SPIRV-Tools ("spirv-tools
#                               2025.1~rc1-1~ubuntu0.24.04.2"), or "" if it cannot be determined.
#                               Goes into the version string (cache key): spvSoftwareVersionString()
#                               alone does not distinguish e.g. 2025.1~rc1 from 2025.1.
#
# AIR2SPIRV_SPIRV_TOOLS_ID overrides the automatic package detection (e.g. "git-<sha>" for a
# self-built SPIRV-Tools without dpkg metadata).

set(_a2s_spirv_hint
    "air2spirv needs SPIRV-Headers and SPIRV-Tools (ADR-0007: spirv-val is mandatory).\n"
    "  Ubuntu 24.04:  sudo apt-get install spirv-headers spirv-tools\n"
    "  Other prefix:  -DCMAKE_PREFIX_PATH=<install prefix of SPIRV-Headers/SPIRV-Tools>\n"
    "Tested with the Ubuntu 24.04 (noble-updates) packages spirv-tools 2025.1~rc1 and "
    "spirv-headers 1.6.1+1.4.309.")

find_package(SPIRV-Headers CONFIG QUIET)
if(NOT SPIRV-Headers_FOUND OR NOT TARGET SPIRV-Headers::SPIRV-Headers)
    message(FATAL_ERROR "SPIRV-Headers not found (no SPIRV-HeadersConfig.cmake).\n"
        ${_a2s_spirv_hint})
endif()

find_package(SPIRV-Tools CONFIG QUIET)
find_package(SPIRV-Tools-opt CONFIG QUIET)
if(TARGET SPIRV-Tools-static)
    # Debian/Ubuntu and upstream static installs.
    set(_a2s_spirv_tools_core SPIRV-Tools-static)
elseif(TARGET SPIRV-Tools)
    set(_a2s_spirv_tools_core SPIRV-Tools)
else()
    message(FATAL_ERROR "SPIRV-Tools not found (no SPIRV-ToolsConfig.cmake providing "
        "SPIRV-Tools-static or SPIRV-Tools).\n" ${_a2s_spirv_hint})
endif()
if(NOT TARGET SPIRV-Tools-opt)
    message(FATAL_ERROR "SPIRV-Tools-opt not found (no SPIRV-Tools-optConfig.cmake).\n"
        ${_a2s_spirv_hint})
endif()

add_library(air2spirv_spirv_deps INTERFACE)
target_link_libraries(air2spirv_spirv_deps INTERFACE
    SPIRV-Headers::SPIRV-Headers SPIRV-Tools-opt ${_a2s_spirv_tools_core})

# ---- package identity (dpkg), robust: empty when unavailable --------------------------------------
# _a2s_dpkg_identity(<file> <out-var>): "<package> <version>" of the Debian package owning <file>.
function(_a2s_dpkg_identity file out_var)
    set(${out_var} "" PARENT_SCOPE)
    find_program(A2S_DPKG_QUERY dpkg-query)
    mark_as_advanced(A2S_DPKG_QUERY)
    if(NOT A2S_DPKG_QUERY OR NOT EXISTS "${file}")
        return()
    endif()
    file(REAL_PATH "${file}" _real)
    foreach(_candidate IN ITEMS "${file}" "${_real}")
        execute_process(COMMAND ${A2S_DPKG_QUERY} -S "${_candidate}"
            OUTPUT_VARIABLE _owner ERROR_QUIET RESULT_VARIABLE _rc
            OUTPUT_STRIP_TRAILING_WHITESPACE)
        if(_rc EQUAL 0 AND _owner MATCHES "^([A-Za-z0-9.+-]+)(:[A-Za-z0-9_-]+)?: ")
            set(_package "${CMAKE_MATCH_1}")
            execute_process(COMMAND ${A2S_DPKG_QUERY} -W "-f=\${Version}" "${_package}"
                OUTPUT_VARIABLE _version ERROR_QUIET RESULT_VARIABLE _rc
                OUTPUT_STRIP_TRAILING_WHITESPACE)
            if(_rc EQUAL 0 AND _version MATCHES "^[A-Za-z0-9.+~:-]+$")
                set(${out_var} "${_package} ${_version}" PARENT_SCOPE)
            endif()
            return()
        endif()
    endforeach()
endfunction()

set(AIR2SPIRV_SPIRV_TOOLS_ID "" CACHE STRING
    "Identity of the linked SPIRV-Tools for the version string (empty: detect via dpkg)")
if(AIR2SPIRV_SPIRV_TOOLS_ID)
    if(NOT AIR2SPIRV_SPIRV_TOOLS_ID MATCHES "^[A-Za-z0-9 ._+~:-]+$")
        message(FATAL_ERROR "AIR2SPIRV_SPIRV_TOOLS_ID may only contain [A-Za-z0-9 ._+~:-]")
    endif()
    set(A2S_SPIRV_TOOLS_PACKAGE "${AIR2SPIRV_SPIRV_TOOLS_ID}")
else()
    get_target_property(_a2s_spirv_tools_lib ${_a2s_spirv_tools_core} LOCATION)
    _a2s_dpkg_identity("${_a2s_spirv_tools_lib}" A2S_SPIRV_TOOLS_PACKAGE)
endif()
_a2s_dpkg_identity("${SPIRV-Headers_DIR}/SPIRV-HeadersConfig.cmake" _a2s_spirv_headers_package)

# Fail at configure time with a clear message instead of deep inside a compile when the
# installed versions are too old for what src/spirv/ uses (SPV_ENV_VULKAN_1_4, the C++
# validator/optimizer API, the SPV_ENABLE_UTILITY_CODE helpers of spirv.hpp11).
include(CheckCXXSourceCompiles)
include(CMakePushCheckState)
# check_cxx_source_compiles caches its result: re-check whenever the dependency locations or
# the compiler change.
set(_a2s_spirv_check_key
    "${SPIRV-Headers_DIR}|${SPIRV-Tools_DIR}|${SPIRV-Tools-opt_DIR}|${CMAKE_CXX_COMPILER}")
if(NOT "${_a2s_spirv_check_key}" STREQUAL "${A2S_SPIRV_DEPS_CHECKED_FOR}")
    unset(A2S_SPIRV_DEPS_USABLE CACHE)
endif()
cmake_push_check_state(RESET)
set(CMAKE_REQUIRED_QUIET ON)
# try_compile only accepts imported targets, so list the package targets directly.
set(CMAKE_REQUIRED_LIBRARIES
    SPIRV-Headers::SPIRV-Headers SPIRV-Tools-opt ${_a2s_spirv_tools_core})
set(CMAKE_REQUIRED_FLAGS "-std=c++20")
check_cxx_source_compiles([=[
#define SPV_ENABLE_UTILITY_CODE
#include <spirv/unified1/spirv.hpp11>
#include <spirv-tools/libspirv.hpp>
#include <spirv-tools/optimizer.hpp>
int main() {
    bool has_result = false;
    bool has_type = false;
    spv::HasResultAndType(spv::Op::OpTypeForwardPointer, &has_result, &has_type);
    spvtools::SpirvTools tools(SPV_ENV_VULKAN_1_4);
    spvtools::ValidatorOptions validator_options;
    spvtools::OptimizerOptions optimizer_options;
    optimizer_options.set_validator_options(validator_options);
    spvtools::Optimizer optimizer(SPV_ENV_VULKAN_1_3);
    return tools.IsValid() && spvSoftwareVersionString() != nullptr ? 0 : 1;
}
]=] A2S_SPIRV_DEPS_USABLE)
cmake_pop_check_state()
set(A2S_SPIRV_DEPS_CHECKED_FOR "${_a2s_spirv_check_key}" CACHE INTERNAL
    "Dependency locations A2S_SPIRV_DEPS_USABLE was computed for")
if(NOT A2S_SPIRV_DEPS_USABLE)
    unset(A2S_SPIRV_DEPS_USABLE CACHE)
    message(FATAL_ERROR "The installed SPIRV-Headers/SPIRV-Tools are unusable or too old "
        "(need SPV_ENV_VULKAN_1_4 and the C++ validator/optimizer API; see "
        "CMakeFiles/CMakeConfigureLog.yaml for the compiler error).\n" ${_a2s_spirv_hint})
endif()

# The version in SPIRV-HeadersConfigVersion.cmake is not maintained by every packager (Ubuntu's
# says 1.5.5 for 1.4.309), so only the package identity is shown.
if(_a2s_spirv_headers_package)
    set(_a2s_headers_desc "package ${_a2s_spirv_headers_package}")
else()
    set(_a2s_headers_desc "version unknown")
endif()
if(A2S_SPIRV_TOOLS_PACKAGE)
    set(_a2s_tools_desc "${A2S_SPIRV_TOOLS_PACKAGE}")
else()
    set(_a2s_tools_desc "package unknown, version string uses the library's build details")
endif()
message(STATUS "air2spirv: SPIRV-Headers from ${SPIRV-Headers_DIR} (${_a2s_headers_desc})")
message(STATUS "air2spirv: SPIRV-Tools from ${SPIRV-Tools_DIR} "
    "(${_a2s_tools_desc}; core target ${_a2s_spirv_tools_core})")
