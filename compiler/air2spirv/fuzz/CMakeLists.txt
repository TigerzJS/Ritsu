# libFuzzer targets (docs/process/testing.md §5). Clang only; enabled by AIR2SPIRV_BUILD_FUZZERS.
#   ./fuzz_air_sniff -timeout=5 -rss_limit_mb=2048 corpus/fuzz_air_sniff
add_executable(fuzz_air_sniff fuzz_air_sniff.cpp)
target_include_directories(fuzz_air_sniff PRIVATE ${PROJECT_SOURCE_DIR}/src)
target_link_libraries(fuzz_air_sniff PRIVATE air2spirv::air2spirv air2spirv_warnings)
# ASan/UBSan come from air2spirv_sanitizers (AIR2SPIRV_SANITIZE is forced ON). The library is
# also instrumented for coverage so the fuzzer sees its branches.
target_compile_options(air2spirv PRIVATE -fsanitize=fuzzer-no-link)
target_compile_options(fuzz_air_sniff PRIVATE -fsanitize=fuzzer)
target_link_options(fuzz_air_sniff PRIVATE -fsanitize=fuzzer)

# Stage 3 (pre-scan, LLVM bitcode reader, verifier). Needs AIR2SPIRV_WITH_LLVM=ON.
#   ./fuzz_air_bitcode -timeout=10 -rss_limit_mb=2048 <work-dir> corpus/fuzz_air_bitcode
# See run_fuzz_air_bitcode.sh. Coverage feedback comes from air2spirv's own code only: the LLVM
# libraries from the distribution are not instrumented (docs/subsystems/shader-compiler.md §11.4).
if(AIR2SPIRV_WITH_LLVM)
    add_executable(fuzz_air_bitcode fuzz_air_bitcode.cpp)
    target_link_libraries(fuzz_air_bitcode PRIVATE air2spirv::air2spirv air2spirv_warnings)
    target_compile_options(fuzz_air_bitcode PRIVATE -fsanitize=fuzzer)
    target_link_options(fuzz_air_bitcode PRIVATE -fsanitize=fuzzer)
endif()
