// The C ABI boundary. Rules enforced here:
//   - no exception ever escapes (every entry point is noexcept and catches everything),
//   - every pointer from the caller is checked before use,
//   - ownership is transferred only through std::unique_ptr::release / adoption.
#include "air2spirv/air2spirv.h"

#include "compiler.hpp"
#include "diagnostics.hpp"
#include "handles.hpp"
#include "options.hpp"
#include "version.hpp"
#include "versioned_struct.hpp"

#include <exception>
#include <memory>
#include <new>
#include <string>
#include <utility>

namespace {

// The public structs are designed without implicit padding (docs/subsystems/shader-compiler.md
// §6.2) so that their layout is identical for C, C++, Rust bindings and hashing.
static_assert(sizeof(a2s_context_desc) == 24);
static_assert(sizeof(a2s_compile_options) == 32);
static_assert(sizeof(a2s_function_constant) == 48);
static_assert(sizeof(a2s_reflection) == 48);
static_assert(sizeof(a2s_binding) == 32);
static_assert(sizeof(void*) != 8 || sizeof(a2s_input) == 48);
static_assert(sizeof(a2s_status) == 4);

using air2spirv::Diagnostics;

a2s_status compile_impl(a2s_context* context, const a2s_input* input, a2s_output& output,
                        Diagnostics& diag) {
    if (context == nullptr) {
        diag.error("context must not be NULL");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (input == nullptr) {
        diag.error("input must not be NULL");
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    air2spirv::CompileRequest request;
    if (const a2s_status s = air2spirv::load_compile_request(input, request, diag); s != A2S_OK) {
        return s;
    }
    air2spirv::CompileResult result;
    const a2s_status s = context->compiler.compile(request, result, diag);
    if (s == A2S_OK) {
        output.spirv = std::move(result.spirv);
        output.reflection = result.reflection;
        output.bindings = std::move(result.bindings);
    }
    return s;
}

// Best effort: turn `output` into an INTERNAL failure carrying `message`. Drops it if even
// that fails (memory exhaustion), so the caller never sees a half-filled object.
void mark_internal_error(std::unique_ptr<a2s_output>& output, const char* message) noexcept {
    try {
        if (!output) {
            output = std::make_unique<a2s_output>();
        }
        output->status = A2S_ERROR_INTERNAL;
        output->spirv.clear();
        output->bindings.clear();
        output->reflection = a2s_reflection{};
        output->diagnostics = std::string("error: internal error: ") + message + '\n';
    } catch (...) {
        output.reset();
    }
}

} // namespace

extern "C" {

uint32_t a2s_get_api_version(void) {
    return A2S_API_VERSION;
}

const char* a2s_get_version_string(void) {
    return A2S_VERSION_STRING;
}

const char* a2s_status_to_string(a2s_status status) {
    switch (status) {
    case A2S_OK:
        return "A2S_OK";
    case A2S_ERROR_INVALID_ARGUMENT:
        return "A2S_ERROR_INVALID_ARGUMENT";
    case A2S_ERROR_MALFORMED_INPUT:
        return "A2S_ERROR_MALFORMED_INPUT";
    case A2S_ERROR_UNSUPPORTED:
        return "A2S_ERROR_UNSUPPORTED";
    case A2S_ERROR_UNIMPLEMENTED:
        return "A2S_ERROR_UNIMPLEMENTED";
    case A2S_ERROR_INTERNAL:
        return "A2S_ERROR_INTERNAL";
    case A2S_ERROR_OUT_OF_MEMORY:
        return "A2S_ERROR_OUT_OF_MEMORY";
    case A2S_STATUS_MAX_ENUM:
        break;
    }
    return "A2S_UNKNOWN_STATUS";
}

void a2s_context_desc_init(a2s_context_desc* desc) {
    if (desc == nullptr) {
        return;
    }
    *desc = a2s_context_desc{};
    desc->struct_size = sizeof(a2s_context_desc);
    desc->api_version = A2S_API_VERSION;
    desc->max_input_size = 0;
}

void a2s_compile_options_init(a2s_compile_options* options) {
    if (options == nullptr) {
        return;
    }
    *options = a2s_compile_options{};
    options->struct_size = sizeof(a2s_compile_options);
    options->target_vulkan_version = A2S_VULKAN_VERSION(1, 3);
}

void a2s_input_init(a2s_input* input) {
    if (input == nullptr) {
        return;
    }
    *input = a2s_input{};
    input->struct_size = sizeof(a2s_input);
}

a2s_status a2s_context_create(const a2s_context_desc* desc, a2s_context** out_context) {
    if (out_context == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    *out_context = nullptr;
    try {
        Diagnostics diag; // context creation has no diagnostics channel; status is enough
        air2spirv::ContextConfig config;
        if (const a2s_status s = air2spirv::load_context_desc(desc, config, diag); s != A2S_OK) {
            return s;
        }
        *out_context = std::make_unique<a2s_context>(config).release();
        return A2S_OK;
    } catch (const std::bad_alloc&) {
        return A2S_ERROR_OUT_OF_MEMORY;
    } catch (...) {
        return A2S_ERROR_INTERNAL;
    }
}

void a2s_context_destroy(a2s_context* context) {
    // Adopt and drop; destructors of the members do not throw.
    std::unique_ptr<a2s_context> owned(context);
}

a2s_status a2s_compile(a2s_context* context, const a2s_input* input, a2s_output** out_output) {
    if (out_output == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    *out_output = nullptr;
    std::unique_ptr<a2s_output> output;
    a2s_status status = A2S_ERROR_INTERNAL;
    try {
        output = std::make_unique<a2s_output>();
        Diagnostics diag;
        status = compile_impl(context, input, *output, diag);
        if (status != A2S_OK) {
            output->spirv.clear();
            output->bindings.clear();
            output->reflection = a2s_reflection{};
        }
        output->status = status;
        output->diagnostics = diag.render();
    } catch (const std::bad_alloc&) {
        output.reset();
        return A2S_ERROR_OUT_OF_MEMORY;
    } catch (const std::exception& e) {
        mark_internal_error(output, e.what());
        status = A2S_ERROR_INTERNAL;
    } catch (...) {
        mark_internal_error(output, "unknown exception");
        status = A2S_ERROR_INTERNAL;
    }
    *out_output = output.release();
    return status;
}

void a2s_output_free(a2s_output* output) {
    std::unique_ptr<a2s_output> owned(output);
}

a2s_status a2s_output_get_status(const a2s_output* output) {
    return output == nullptr ? A2S_ERROR_INVALID_ARGUMENT : output->status;
}

const char* a2s_output_get_diagnostics(const a2s_output* output) {
    return output == nullptr ? "" : output->diagnostics.c_str();
}

const uint32_t* a2s_output_get_spirv(const a2s_output* output, size_t* out_word_count) {
    if (out_word_count != nullptr) {
        *out_word_count = 0;
    }
    if (output == nullptr || out_word_count == nullptr || output->status != A2S_OK ||
        output->spirv.empty()) {
        return nullptr;
    }
    *out_word_count = output->spirv.size();
    return output->spirv.data();
}

a2s_status a2s_output_get_reflection(const a2s_output* output, a2s_reflection* out_reflection) {
    if (output == nullptr || out_reflection == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (output->status != A2S_OK) {
        return output->status;
    }
    a2s_reflection reflection = output->reflection;
    reflection.binding_count = static_cast<uint32_t>(output->bindings.size());
    return air2spirv::store_versioned_struct(reflection, out_reflection);
}

a2s_status a2s_output_get_binding(const a2s_output* output, uint32_t index,
                                  a2s_binding* out_binding) {
    if (output == nullptr || out_binding == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    if (output->status != A2S_OK) {
        return output->status;
    }
    if (index >= output->bindings.size()) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    return air2spirv::store_versioned_struct(output->bindings[index], out_binding);
}

} // extern "C"
