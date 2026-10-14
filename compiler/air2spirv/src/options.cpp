#include "options.hpp"

#include "versioned_struct.hpp"

#include <algorithm>
#include <cstring>
#include <string>

namespace air2spirv {
namespace {

bool is_valid_subgroup_size(std::uint32_t size) noexcept {
    // 0 = unknown, otherwise a power of two in [1, 128].
    return size == 0 || (size <= 128 && (size & (size - 1)) == 0);
}

bool api_version_compatible(std::uint32_t caller) noexcept {
    const std::uint32_t caller_major = A2S_API_VERSION_GET_MAJOR(caller);
    const std::uint32_t caller_minor = A2S_API_VERSION_GET_MINOR(caller);
    if (caller_major != A2S_API_VERSION_MAJOR) {
        return false;
    }
    if (caller_major == 0) {
        // Unstable API: every 0.x version is its own ABI.
        return caller_minor == A2S_API_VERSION_MINOR;
    }
    return caller_minor <= A2S_API_VERSION_MINOR;
}

} // namespace

std::size_t constant_component_size(std::uint32_t type) noexcept {
    switch (type) {
    case A2S_CONSTANT_TYPE_BOOL:
    case A2S_CONSTANT_TYPE_INT8:
    case A2S_CONSTANT_TYPE_UINT8:
        return 1;
    case A2S_CONSTANT_TYPE_INT16:
    case A2S_CONSTANT_TYPE_UINT16:
    case A2S_CONSTANT_TYPE_HALF:
    case A2S_CONSTANT_TYPE_BFLOAT:
        return 2;
    case A2S_CONSTANT_TYPE_INT32:
    case A2S_CONSTANT_TYPE_UINT32:
    case A2S_CONSTANT_TYPE_FLOAT:
        return 4;
    case A2S_CONSTANT_TYPE_INT64:
    case A2S_CONSTANT_TYPE_UINT64:
        return 8;
    default:
        return 0;
    }
}

a2s_status load_context_desc(const a2s_context_desc* desc, ContextConfig& out, Diagnostics& diag) {
    out = ContextConfig{};
    if (desc == nullptr) {
        return A2S_OK;
    }
    a2s_context_desc d{};
    if (load_versioned_struct(desc, d) != A2S_OK) {
        diag.error("a2s_context_desc: invalid struct_size or non-zero unknown trailing bytes");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (!api_version_compatible(d.api_version)) {
        diag.error("a2s_context_desc: API version " +
                   std::to_string(A2S_API_VERSION_GET_MAJOR(d.api_version)) + "." +
                   std::to_string(A2S_API_VERSION_GET_MINOR(d.api_version)) +
                   " is not compatible with library API version " +
                   std::to_string(A2S_API_VERSION_MAJOR) + "." +
                   std::to_string(A2S_API_VERSION_MINOR));
        return A2S_ERROR_UNSUPPORTED;
    }
    if (d.flags != 0 || d.reserved != 0) {
        diag.error("a2s_context_desc: flags and reserved must be 0");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (d.max_input_size != 0) {
        out.max_input_size = d.max_input_size;
    }
    return A2S_OK;
}

a2s_status load_compile_options(const a2s_compile_options* options, CompileOptions& out,
                                Diagnostics& diag) {
    out = CompileOptions{};
    if (options == nullptr) {
        return A2S_OK;
    }
    a2s_compile_options o{};
    if (load_versioned_struct(options, o) != A2S_OK) {
        diag.error("a2s_compile_options: invalid struct_size or non-zero unknown trailing bytes");
        return A2S_ERROR_INVALID_ARGUMENT;
    }

    if (o.target_vulkan_version != 0) {
        const std::uint32_t variant = o.target_vulkan_version >> 29U;
        const std::uint32_t major = (o.target_vulkan_version >> 22U) & 0x7FU;
        const std::uint32_t minor = (o.target_vulkan_version >> 12U) & 0x3FFU;
        if (variant != 0) {
            diag.error("a2s_compile_options: target_vulkan_version has a non-zero variant");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        // ADR-0007: output is Vulkan SPIR-V 1.6, which requires Vulkan 1.3.
        if (major != 1 || minor < 3) {
            diag.error("a2s_compile_options: target Vulkan " + std::to_string(major) + "." +
                       std::to_string(minor) + " is not supported (need 1.3 or newer 1.x)");
            return A2S_ERROR_UNSUPPORTED;
        }
        out.vulkan_major = major;
        out.vulkan_minor = minor;
    }

    if ((o.flags & ~kKnownCompileFlags) != 0) {
        diag.error("a2s_compile_options: unknown bits in flags");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (!is_valid_subgroup_size(o.min_subgroup_size) ||
        !is_valid_subgroup_size(o.max_subgroup_size)) {
        diag.error("a2s_compile_options: subgroup sizes must be 0 or a power of two <= 128");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (o.min_subgroup_size != 0 && o.max_subgroup_size != 0 &&
        o.min_subgroup_size > o.max_subgroup_size) {
        diag.error("a2s_compile_options: min_subgroup_size > max_subgroup_size");
        return A2S_ERROR_INVALID_ARGUMENT;
    }

    out.features = o.features & kKnownFeatureMask;
    out.min_subgroup_size = o.min_subgroup_size;
    out.max_subgroup_size = o.max_subgroup_size;
    out.required_subgroup_size_stages = o.required_subgroup_size_stages & kKnownStageMask;
    out.flags = o.flags;
    return A2S_OK;
}

namespace {

a2s_status load_function_constants(const a2s_input& in, CompileRequest& out, Diagnostics& diag) {
    if (in.function_constant_count == 0) {
        return A2S_OK;
    }
    if (in.function_constants == nullptr) {
        diag.error("a2s_input: function_constants is NULL but function_constant_count > 0");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (in.function_constant_count > A2S_MAX_FUNCTION_CONSTANTS) {
        diag.error("a2s_input: too many function constants");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    const std::span<const a2s_function_constant> constants(in.function_constants,
                                                           in.function_constant_count);
    out.function_constants.reserve(constants.size());
    for (std::size_t i = 0; i < constants.size(); ++i) {
        const a2s_function_constant& c = constants[i];
        const std::string where = "a2s_input: function_constants[" + std::to_string(i) + "]: ";
        const std::size_t component_size = constant_component_size(c.type);
        if (component_size == 0) {
            diag.error(where + "invalid type");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        if (c.component_count < 1 || c.component_count > 4) {
            diag.error(where + "component_count must be in [1, 4]");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        if (c.reserved != 0) {
            diag.error(where + "reserved must be 0");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        FunctionConstantValue value;
        value.index = c.index;
        value.type = static_cast<a2s_constant_type>(c.type);
        value.component_count = c.component_count;
        std::memcpy(value.data.data(), c.data, component_size * c.component_count);
        if (c.type == A2S_CONSTANT_TYPE_BOOL) {
            for (std::uint32_t k = 0; k < c.component_count; ++k) {
                if (c.data[k] > 1) {
                    diag.error(where + "bool components must be 0 or 1");
                    return A2S_ERROR_INVALID_ARGUMENT;
                }
            }
        }
        out.function_constants.push_back(value);
    }
    std::sort(out.function_constants.begin(), out.function_constants.end(),
              [](const FunctionConstantValue& a, const FunctionConstantValue& b) {
                  return a.index < b.index;
              });
    const auto dup =
        std::adjacent_find(out.function_constants.begin(), out.function_constants.end(),
                           [](const FunctionConstantValue& a, const FunctionConstantValue& b) {
                               return a.index == b.index;
                           });
    if (dup != out.function_constants.end()) {
        diag.error("a2s_input: duplicate function constant index " + std::to_string(dup->index));
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    return A2S_OK;
}

} // namespace

a2s_status load_compile_request(const a2s_input* input, CompileRequest& out, Diagnostics& diag) {
    out = CompileRequest{};
    a2s_input in{};
    if (load_versioned_struct(input, in) != A2S_OK) {
        diag.error("a2s_input: NULL, invalid struct_size or non-zero unknown trailing bytes");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (in.data == nullptr && in.size != 0) {
        diag.error("a2s_input: data is NULL but size is non-zero");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (in.entry_point != nullptr) {
        // Bounded scan: never read past A2S_MAX_ENTRY_POINT_LENGTH + 1 bytes.
        std::size_t length = 0;
        while (length <= A2S_MAX_ENTRY_POINT_LENGTH && in.entry_point[length] != '\0') {
            ++length;
        }
        if (length > A2S_MAX_ENTRY_POINT_LENGTH) {
            diag.error("a2s_input: entry_point is longer than A2S_MAX_ENTRY_POINT_LENGTH");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        if (length == 0) {
            diag.error("a2s_input: entry_point is empty (use NULL for the sole entry point)");
            return A2S_ERROR_INVALID_ARGUMENT;
        }
        out.entry_point.emplace(in.entry_point, length);
    }
    if (const a2s_status s = load_compile_options(in.options, out.options, diag); s != A2S_OK) {
        return s;
    }
    if (const a2s_status s = load_function_constants(in, out, diag); s != A2S_OK) {
        return s;
    }
    if (in.data != nullptr) {
        out.data = std::as_bytes(std::span<const std::uint8_t>(in.data, in.size));
    }
    return A2S_OK;
}

} // namespace air2spirv
