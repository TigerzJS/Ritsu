# LLVM for stage 3 (bitcode parse + verifier), only with AIR2SPIRV_WITH_LLVM=ON.
# docs/subsystems/shader-compiler.md §2.3 and §11.4 (WP-SC-02).
#
# Pinned (provisionally, S-AIR-1 part 1): LLVM 18.1 as shipped by Ubuntu 24.04 (llvm-18-dev,
# plus libzstd-dev for LLVMExports.cmake). Another major/minor version is rejected: the
# bitcode reader's error texts, the record tables of the identification pre-check and the
# hardening in src/bitcode/llvm_runtime.cpp are written against this version.
#
# Provides:
#   target air2spirv_llvm_deps       INTERFACE: include dirs (SYSTEM), definitions, libraries
#   A2S_LLVM_PACKAGE                 identity of the linked LLVM for the version string
#
# AIR2SPIRV_LLVM_ID overrides the automatic package detection (self-built LLVM).

set(AIR2SPIRV_LLVM_VERSION "18.1" CACHE STRING "Required LLVM major.minor (pinned, S-AIR-1)")
mark_as_advanced(AIR2SPIRV_LLVM_VERSION)

find_package(LLVM ${AIR2SPIRV_LLVM_VERSION} CONFIG
    HINTS /usr/lib/llvm-18/lib/cmake/llvm)
if(NOT LLVM_FOUND)
    message(FATAL_ERROR "AIR2SPIRV_WITH_LLVM=ON needs LLVM ${AIR2SPIRV_LLVM_VERSION}.x "
        "(found: '${LLVM_PACKAGE_VERSION}').\n"
        "  Ubuntu 24.04:  sudo apt-get install llvm-18-dev libzstd-dev\n"
        "  Other prefix:  -DLLVM_DIR=<prefix>/lib/cmake/llvm")
endif()
message(STATUS "air2spirv: using LLVM ${LLVM_PACKAGE_VERSION} from ${LLVM_DIR}")

add_library(air2spirv_llvm_deps INTERFACE)
separate_arguments(_a2s_llvm_definitions NATIVE_COMMAND "${LLVM_DEFINITIONS}")
# SYSTEM: LLVM headers must not trip our -Werror warning set.
target_include_directories(air2spirv_llvm_deps SYSTEM INTERFACE ${LLVM_INCLUDE_DIRS})
target_compile_definitions(air2spirv_llvm_deps INTERFACE ${_a2s_llvm_definitions})
# Static component libraries (not libLLVM.so): the host process may also load Mesa's libLLVM,
# and air2spirv's LLVM state (error handlers, options) must stay its own.
llvm_map_components_to_libnames(_a2s_llvm_libs bitreader bitstreamreader core support)
target_link_libraries(air2spirv_llvm_deps INTERFACE ${_a2s_llvm_libs})

set(AIR2SPIRV_LLVM_ID "" CACHE STRING
    "Identity of the linked LLVM for the version string (empty: detect via dpkg)")
if(AIR2SPIRV_LLVM_ID)
    if(NOT AIR2SPIRV_LLVM_ID MATCHES "^[A-Za-z0-9 ._+~:-]+$")
        message(FATAL_ERROR "AIR2SPIRV_LLVM_ID may only contain [A-Za-z0-9 ._+~:-]")
    endif()
    set(A2S_LLVM_PACKAGE "${AIR2SPIRV_LLVM_ID}")
else()
    # _a2s_dpkg_identity comes from Air2SpirvSpirvDeps.cmake.
    find_library(_a2s_llvm_core_lib NAMES LLVMCore PATHS ${LLVM_LIBRARY_DIRS} NO_DEFAULT_PATH)
    set(A2S_LLVM_PACKAGE "")
    if(_a2s_llvm_core_lib)
        _a2s_dpkg_identity("${_a2s_llvm_core_lib}" A2S_LLVM_PACKAGE)
    endif()
    if(NOT A2S_LLVM_PACKAGE)
        message(WARNING "air2spirv: cannot identify the LLVM package; set AIR2SPIRV_LLVM_ID so "
            "that the version string (shader cache key) tells LLVM builds apart")
        set(A2S_LLVM_PACKAGE "unidentified")
    endif()
endif()
message(STATUS "air2spirv: LLVM identity '${A2S_LLVM_PACKAGE}'")
