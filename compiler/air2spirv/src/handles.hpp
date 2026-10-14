// Definitions of the opaque handle types declared in the public header. They must live in the
// global namespace because the C header forward-declares them there.
#pragma once

#include "air2spirv/air2spirv.h"

#include "compiler.hpp"

#include <cstdint>
#include <string>
#include <vector>

struct a2s_context {
    explicit a2s_context(const air2spirv::ContextConfig& config) : compiler(config) {}

    const air2spirv::Compiler compiler;
};

struct a2s_output {
    a2s_status status = A2S_ERROR_INTERNAL;
    std::string diagnostics;
    std::vector<std::uint32_t> spirv;
    a2s_reflection reflection{};
    std::vector<a2s_binding> bindings;
};
