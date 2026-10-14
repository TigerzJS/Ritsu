// The translation pipeline (docs/subsystems/shader-compiler.md §2). Stateless after
// construction, so one instance can serve concurrent compilations.
#pragma once

#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"
#include "options.hpp"

#include <cstdint>
#include <vector>

namespace air2spirv {

struct CompileResult {
    std::vector<std::uint32_t> spirv;
    a2s_reflection reflection{};
    std::vector<a2s_binding> bindings;
};

class Compiler {
public:
    explicit Compiler(const ContextConfig& config) : config_(config) {}

    // Runs the pipeline. On success `result` holds a module that passed validation; on failure
    // the reason is in `diag` and `result` must be ignored.
    [[nodiscard]] a2s_status compile(const CompileRequest& request, CompileResult& result,
                                     Diagnostics& diag) const;

private:
    ContextConfig config_;
};

} // namespace air2spirv
