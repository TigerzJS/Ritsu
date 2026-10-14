#include "compiler.hpp"

#include "input_sniffer.hpp"

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

    // Stages 3-8 (LLVM parse, AIR analysis, lowering, SPIR-V emission, optimization,
    // validation) do not exist yet. Report honestly instead of producing anything.
    if (request.entry_point.has_value()) {
        diag.note("requested entry point: '" + escape_untrusted(*request.entry_point) + "'");
    }
    diag.error("AIR to SPIR-V translation is not implemented yet (planned from WP-SC-02, see "
               "docs/subsystems/shader-compiler.md)");
    return A2S_ERROR_UNIMPLEMENTED;
}

} // namespace air2spirv
