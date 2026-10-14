# air2spirv - Apple AIR -> Vulkan SPIR-V (docs/subsystems/shader-compiler.md)
cmake_minimum_required(VERSION 3.25)

project(air2spirv
    VERSION 0.1.0
    DESCRIPTION "Apple AIR (LLVM bitcode) to Vulkan SPIR-V translator"
    LANGUAGES C CXX)

# ---- Options --------------------------------------------------------------------------------------
option(AIR2SPIRV_WERROR "Treat compiler warnings as errors" ON)
option(AIR2SPIRV_WITH_LLVM "Build against LLVM (find_package(LLVM CONFIG))" OFF)
option(AIR2SPIRV_SANITIZE "Build everything with AddressSanitizer + UndefinedBehaviorSanitizer" OFF)
option(AIR2SPIRV_BUILD_TESTS "Build the ctest test suite" ${PROJECT_IS_TOP_LEVEL})
option(AIR2SPIRV_BUILD_TOOLS "Build air2spirv-cli" ON)
option(AIR2SPIRV_BUILD_FUZZERS "Build libFuzzer targets (Clang only)" OFF)
set(AIR2SPIRV_BUILD_ID "dev" CACHE STRING
    "Build identity appended to the version string (part of every shader cache key)")

# The build id goes verbatim into the version string (cache key) and a C string literal.
string(LENGTH "${AIR2SPIRV_BUILD_ID}" _a2s_build_id_length)
if(_a2s_build_id_length GREATER 64 OR NOT AIR2SPIRV_BUILD_ID MATCHES "^[A-Za-z0-9._+-]+$")
    message(FATAL_ERROR "AIR2SPIRV_BUILD_ID must be 1-64 characters from [A-Za-z0-9._+-], "
        "got '${AIR2SPIRV_BUILD_ID}'")
endif()

if(PROJECT_IS_TOP_LEVEL AND NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE Debug CACHE STRING "Build type" FORCE)
endif()

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

if(AIR2SPIRV_BUILD_FUZZERS)
    if(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang")
        message(FATAL_ERROR "AIR2SPIRV_BUILD_FUZZERS requires Clang (libFuzzer)")
    endif()
    # The library gets coverage instrumentation for the fuzzer, so every binary that links it
    # needs the sanitizer runtimes as well.
    set(AIR2SPIRV_SANITIZE ON CACHE BOOL "" FORCE)
endif()

# ---- Shared compile settings (interface targets) --------------------------------------------------
add_library(air2spirv_warnings INTERFACE)
if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
    target_compile_options(air2spirv_warnings INTERFACE
        -Wall -Wextra -Wpedantic
        -Wconversion -Wsign-conversion -Wshadow -Wcast-align -Wformat=2
        -Wimplicit-fallthrough -Wnull-dereference -Wundef
        $<$<COMPILE_LANGUAGE:CXX>:-Wnon-virtual-dtor -Wold-style-cast -Woverloaded-virtual>
        $<$<COMPILE_LANGUAGE:C>:-Wstrict-prototypes -Wmissing-prototypes>
        $<$<BOOL:${AIR2SPIRV_WERROR}>:-Werror>)
endif()

add_library(air2spirv_sanitizers INTERFACE)
if(AIR2SPIRV_SANITIZE)
    set(_a2s_san_flags -fsanitize=address,undefined -fno-sanitize-recover=all
        -fno-omit-frame-pointer)
    target_compile_options(air2spirv_sanitizers INTERFACE ${_a2s_san_flags})
    target_link_options(air2spirv_sanitizers INTERFACE ${_a2s_san_flags})
endif()

# ---- External dependencies ----------------------------------------------------------------------
list(APPEND CMAKE_MODULE_PATH ${CMAKE_CURRENT_SOURCE_DIR}/cmake)
include(Air2SpirvSpirvDeps) # required: SPIRV-Headers + SPIRV-Tools, target air2spirv_spirv_deps

# ---- Library --------------------------------------------------------------------------------------
add_library(air2spirv
    src/c_api.cpp
    src/compiler.cpp
    src/diagnostics.cpp
    src/input_sniffer.cpp
    src/options.cpp
    src/version.cpp
    src/spirv/module_builder.cpp
    src/spirv/tools.cpp)
add_library(air2spirv::air2spirv ALIAS air2spirv)

target_include_directories(air2spirv
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>
    PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/src)
target_compile_features(air2spirv PRIVATE cxx_std_20)
target_compile_definitions(air2spirv PRIVATE
    A2S_VERSION="${PROJECT_VERSION}+${AIR2SPIRV_BUILD_ID}"
    A2S_SPIRV_TOOLS_PACKAGE="${A2S_SPIRV_TOOLS_PACKAGE}")
target_link_libraries(air2spirv
    PRIVATE air2spirv_warnings air2spirv_spirv_deps
    PUBLIC air2spirv_sanitizers)
set_target_properties(air2spirv PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    VISIBILITY_INLINES_HIDDEN ON
    POSITION_INDEPENDENT_CODE ON)

# The library is C++ even though its interface is C: consumers (including the C test and the
# Rust build script) must link the C++ runtime.
set_property(TARGET air2spirv APPEND PROPERTY INTERFACE_LINK_LIBRARIES
    $<$<LINK_LANGUAGE:C>:stdc++>)

if(AIR2SPIRV_WITH_LLVM)
    find_package(LLVM CONFIG REQUIRED)
    message(STATUS "air2spirv: using LLVM ${LLVM_PACKAGE_VERSION} from ${LLVM_DIR}")
    separate_arguments(_a2s_llvm_definitions NATIVE_COMMAND "${LLVM_DEFINITIONS}")
    # SYSTEM: LLVM headers must not trip our -Werror warning set.
    target_include_directories(air2spirv SYSTEM PRIVATE ${LLVM_INCLUDE_DIRS})
    target_compile_definitions(air2spirv PRIVATE ${_a2s_llvm_definitions} A2S_HAVE_LLVM=1)
    llvm_map_components_to_libnames(_a2s_llvm_libs bitreader bitstreamreader core support)
    target_link_libraries(air2spirv PRIVATE ${_a2s_llvm_libs})
    # NOTE: LLVM is usually built without RTTI. As long as air2spirv does not derive from LLVM
    # classes this does not matter; once it does (custom passes), add -fno-rtti for the files
    # that do, see docs/subsystems/shader-compiler.md (WP-SC-02).
else()
    target_compile_definitions(air2spirv PRIVATE A2S_HAVE_LLVM=0)
endif()

# ---- Tools, tests, fuzzers ------------------------------------------------------------------------
if(AIR2SPIRV_BUILD_TOOLS)
    add_subdirectory(tools/air2spirv-cli)
endif()

if(AIR2SPIRV_BUILD_TESTS)
    enable_testing()
    add_subdirectory(tests)
endif()

if(AIR2SPIRV_BUILD_FUZZERS)
    add_subdirectory(fuzz)
endif()

message(STATUS "air2spirv ${PROJECT_VERSION}+${AIR2SPIRV_BUILD_ID}: "
    "llvm=${AIR2SPIRV_WITH_LLVM} werror=${AIR2SPIRV_WERROR} sanitize=${AIR2SPIRV_SANITIZE} "
    "tests=${AIR2SPIRV_BUILD_TESTS} tools=${AIR2SPIRV_BUILD_TOOLS} fuzzers=${AIR2SPIRV_BUILD_FUZZERS}")
