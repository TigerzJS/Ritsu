// libFuzzer target for stage 3: bitcode pre-scan, LLVM bitcode reader, verifier
// (docs/subsystems/shader-compiler.md §10, WP-SC-02). Built only with AIR2SPIRV_WITH_LLVM=ON.
//
// Seeds: fuzz/corpus/fuzz_air_bitcode (copies of tests/fixtures/bitcode, own sources only).
// Every input goes through the public entry point a2s_compile() exactly like production code.
// Invariants:
//   - the status is MALFORMED, UNSUPPORTED or UNIMPLEMENTED (OK is impossible until translation
//     exists; INTERNAL / OUT_OF_MEMORY / INVALID_ARGUMENT would be bugs),
//   - the output object is consistent and explains every failure,
//   - an UNIMPLEMENTED result for bitcode means LLVM parsed and verified a module,
//   - the result is deterministic (second run on the same bytes, same status and diagnostics).
// Crashes, aborts (including LLVM fatal errors, which abort, see src/bitcode/llvm_runtime.hpp),
// sanitizer reports, timeouts and out-of-memory are findings.
#include "air2spirv/air2spirv.h"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

a2s_context* g_context = nullptr;

[[noreturn]] void invariant_failed(const char* what) {
    __builtin_trap(); // keeps `what` in the crash report's frame for triage
    (void)what;
}

struct Outcome {
    a2s_status status;
    std::string diagnostics;
};

Outcome compile(const std::uint8_t* data, std::size_t size) {
    a2s_input input;
    a2s_input_init(&input);
    input.data = data;
    input.size = size;
    a2s_output* output = nullptr;
    const a2s_status status = a2s_compile(g_context, &input, &output);
    if (output == nullptr || a2s_output_get_status(output) != status) {
        invariant_failed("output object missing or inconsistent");
    }
    Outcome outcome{status, a2s_output_get_diagnostics(output)};
    std::size_t words = 1;
    if (a2s_output_get_spirv(output, &words) != nullptr || words != 0) {
        invariant_failed("SPIR-V on a failed compilation");
    }
    a2s_output_free(output);
    return outcome;
}

} // namespace

extern "C" int LLVMFuzzerInitialize(int* /*argc*/, char*** /*argv*/) {
    if (a2s_context_create(nullptr, &g_context) != A2S_OK) {
        std::abort();
    }
    return 0;
}

extern "C" int LLVMFuzzerTestOneInput(const std::uint8_t* data, std::size_t size) {
    const Outcome first = compile(data, size);
    switch (first.status) {
    case A2S_ERROR_MALFORMED_INPUT:
    case A2S_ERROR_UNSUPPORTED:
        break;
    case A2S_ERROR_UNIMPLEMENTED:
        if (first.diagnostics.find("note: bitcode: parsed ") == std::string::npos) {
            invariant_failed("UNIMPLEMENTED without a parsed module");
        }
        break;
    default:
        invariant_failed("unexpected status");
    }
    if (first.diagnostics.find("error: ") == std::string::npos) {
        invariant_failed("failure without an error diagnostic");
    }

    const Outcome second = compile(data, size);
    if (second.status != first.status || second.diagnostics != first.diagnostics) {
        invariant_failed("nondeterministic result");
    }
    return 0;
}
