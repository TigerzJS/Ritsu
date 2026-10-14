// Validation and normalization of caller-provided C structs into internal C++ types.
// Everything that comes through the C ABI is checked here before any other code sees it.
#pragma once

#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <string>
#include <vector>

namespace air2spirv {

struct ContextConfig {
    std::uint64_t max_input_size = A2S_DEFAULT_MAX_INPUT_SIZE;
};

struct CompileOptions {
    std::uint32_t vulkan_major = 1;
    std::uint32_t vulkan_minor = 3;
    std::uint64_t features = 0;
    std::uint32_t min_subgroup_size = 0;
    std::uint32_t max_subgroup_size = 0;
    std::uint32_t required_subgroup_size_stages = 0;
    std::uint32_t flags = 0;
};

struct FunctionConstantValue {
    std::uint32_t index = 0;
    a2s_constant_type type = A2S_CONSTANT_TYPE_INVALID;
    std::uint32_t component_count = 0;
    std::array<std::byte, 32> data{};
};

struct CompileRequest {
    std::span<const std::byte> data;
    std::optional<std::string> entry_point;
    CompileOptions options;
    std::vector<FunctionConstantValue> function_constants; // sorted by index, unique
};

// All A2S_FEATURE_* bits this library knows; other bits are ignored.
inline constexpr std::uint64_t kKnownFeatureMask = (std::uint64_t{1} << 24U) - 1U;
// All A2S_COMPILE_FLAG_* bits this library knows; other bits are rejected.
inline constexpr std::uint32_t kKnownCompileFlags =
    A2S_COMPILE_FLAG_OPTIMIZE | A2S_COMPILE_FLAG_DEBUG_INFO | A2S_COMPILE_FLAG_SKIP_VALIDATION;
// Mask of all valid A2S_STAGE_BIT values.
inline constexpr std::uint32_t kKnownStageMask = (1U << (A2S_STAGE_INTERSECTION + 1U)) - 2U;

// Returns the byte size of one component of a function constant type, 0 if invalid.
[[nodiscard]] std::size_t constant_component_size(std::uint32_t type) noexcept;

[[nodiscard]] a2s_status load_context_desc(const a2s_context_desc* desc, ContextConfig& out,
                                           Diagnostics& diag);

[[nodiscard]] a2s_status load_compile_options(const a2s_compile_options* options,
                                              CompileOptions& out, Diagnostics& diag);

[[nodiscard]] a2s_status load_compile_request(const a2s_input* input, CompileRequest& out,
                                              Diagnostics& diag);

} // namespace air2spirv
