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
