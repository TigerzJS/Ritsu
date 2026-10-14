#include "compiler.hpp"

#include "input_sniffer.hpp"

#if A2S_HAVE_LLVM
#include "bitcode/bitcode_module.hpp"
#endif

#include <string>

namespace air2spirv {

a2s_status Compiler::compile(const CompileRequest& request, CompileResult& result,
                             Diagnostics& diag) const {
    result = CompileResult{};

    // Stage 0: resource limits on untrusted input.
    if (request.data.size() > config_.max_input_size) {
        diag.error("input size " + std::to_string(request.data.size()) +
                   " bytes exceeds the context limit of " + std::to_string(config_.max_input_size) +
                   " bytes");
        return A2S_ERROR_UNSUPPORTED;
    }

    // Stage 1/2: container recognition and bitcode extraction.
    SniffedInput sniffed;
    if (const a2s_status s = sniff_input(request.data, sniffed, diag); s != A2S_OK) {
        return s;
    }
    diag.note(describe(sniffed));

    // Stage 3: LLVM parse + verifier (only in builds with LLVM).
#if A2S_HAVE_LLVM
    bitcode::ParsedModule module;
    if (const a2s_status s = bitcode::parse_bitcode_module(sniffed.bitcode, module, diag);
        s != A2S_OK) {
        return s;
    }
    diag.note("bitcode: " + bitcode::describe(module.stats()) + " (LLVM " +
              bitcode::llvm_version() + ")");
#else
    diag.note("bitcode not parsed: air2spirv was built without LLVM (AIR2SPIRV_WITH_LLVM=OFF)");
#endif

    // Stages 4-8 (AIR analysis, lowering, SPIR-V emission, optimization, validation) do not
    // exist yet. Report honestly instead of producing anything.
    if (request.entry_point.has_value()) {
        diag.note("requested entry point: '" + escape_untrusted(*request.entry_point) + "'");
    }
    diag.error("AIR to SPIR-V translation is not implemented yet (AIR analysis follows in "
               "WP-SC-08, see docs/subsystems/shader-compiler.md)");
    return A2S_ERROR_UNIMPLEMENTED;
}

} // namespace air2spirv
