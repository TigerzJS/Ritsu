// Tests for the public C API: argument validation, NULL handling, struct_size evolution,
// status/diagnostic contract and thread safety.
#include "air2spirv/air2spirv.h"

#include "test_harness.hpp"
#include "test_inputs.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

namespace {

struct ContextDeleter {
    void operator()(a2s_context* c) const noexcept { a2s_context_destroy(c); }
};
struct OutputDeleter {
    void operator()(a2s_output* o) const noexcept { a2s_output_free(o); }
};
using ContextPtr = std::unique_ptr<a2s_context, ContextDeleter>;
using OutputPtr = std::unique_ptr<a2s_output, OutputDeleter>;

ContextPtr make_context(const a2s_context_desc* desc = nullptr) {
    a2s_context* raw = nullptr;
    const a2s_status s = a2s_context_create(desc, &raw);
    A2S_CHECK_EQ(s, A2S_OK);
    A2S_CHECK(raw != nullptr);
    return ContextPtr(raw);
}

struct CompileOutcome {
    a2s_status status;
    OutputPtr output;
};

CompileOutcome compile(a2s_context* ctx, const a2s_input* input) {
    a2s_output* raw = reinterpret_cast<a2s_output*>(std::uintptr_t{0x1}); // must be overwritten
    const a2s_status s = a2s_compile(ctx, input, &raw);
    A2S_CHECK(raw != reinterpret_cast<a2s_output*>(std::uintptr_t{0x1}));
    OutputPtr out(raw);
    if (out) {
        A2S_CHECK_EQ(a2s_output_get_status(out.get()), s);
        // Failures always explain themselves.
        if (s != A2S_OK) {
            A2S_CHECK(std::string_view(a2s_output_get_diagnostics(out.get())).find("error: ") !=
                      std::string_view::npos);
            std::size_t words = 99;
            A2S_CHECK(a2s_output_get_spirv(out.get(), &words) == nullptr);
            A2S_CHECK_EQ(words, std::size_t{0});
        }
    }
    return CompileOutcome{s, std::move(out)};
}

a2s_input input_for(const std::vector<std::uint8_t>& bytes) {
    a2s_input input;
    a2s_input_init(&input);
    input.data = bytes.data();
    input.size = bytes.size();
    return input;
}

bool diagnostics_contain(const a2s_output* out, std::string_view needle) {
    return std::string_view(a2s_output_get_diagnostics(out)).find(needle) != std::string_view::npos;
}

} // namespace

// ---- versions and strings ---------------------------------------------------------------------

A2S_TEST(api_version_matches_header) {
    A2S_CHECK_EQ(a2s_get_api_version(), static_cast<std::uint32_t>(A2S_API_VERSION));
    A2S_CHECK_EQ(A2S_API_VERSION_GET_MAJOR(A2S_API_VERSION), 0U);
    A2S_CHECK_EQ(A2S_API_VERSION_GET_MINOR(A2S_API_VERSION), 1U);
    A2S_CHECK_EQ(A2S_VULKAN_VERSION(1, 3), (1U << 22U) | (3U << 12U));
}

A2S_TEST(version_string_is_present) {
    const char* v = a2s_get_version_string();
    A2S_CHECK(v != nullptr);
    A2S_CHECK(std::string_view(v).starts_with("air2spirv "));
}

A2S_TEST(status_strings_are_stable) {
    A2S_CHECK_EQ(std::string(a2s_status_to_string(A2S_OK)), std::string("A2S_OK"));
    A2S_CHECK_EQ(std::string(a2s_status_to_string(A2S_ERROR_UNIMPLEMENTED)),
                 std::string("A2S_ERROR_UNIMPLEMENTED"));
    A2S_CHECK_EQ(std::string(a2s_status_to_string(static_cast<a2s_status>(1234))),
                 std::string("A2S_UNKNOWN_STATUS"));
    for (int s = 0; s <= 6; ++s) {
        const std::string_view name = a2s_status_to_string(static_cast<a2s_status>(s));
        A2S_CHECK(name.starts_with("A2S_") && name != "A2S_UNKNOWN_STATUS");
    }
}

// ---- init helpers -------------------------------------------------------------------------------

A2S_TEST(init_functions_set_struct_size_and_defaults) {
    a2s_context_desc desc;
    std::memset(&desc, 0xAB, sizeof(desc));
    a2s_context_desc_init(&desc);
    A2S_CHECK_EQ(desc.struct_size, static_cast<std::uint32_t>(sizeof(desc)));
    A2S_CHECK_EQ(desc.api_version, static_cast<std::uint32_t>(A2S_API_VERSION));
    A2S_CHECK_EQ(desc.max_input_size, std::uint64_t{0});
    A2S_CHECK_EQ(desc.flags, 0U);

    a2s_compile_options options;
    std::memset(&options, 0xAB, sizeof(options));
    a2s_compile_options_init(&options);
    A2S_CHECK_EQ(options.struct_size, static_cast<std::uint32_t>(sizeof(options)));
    A2S_CHECK_EQ(options.target_vulkan_version,
                 static_cast<std::uint32_t>(A2S_VULKAN_VERSION(1, 3)));
    A2S_CHECK_EQ(options.features, std::uint64_t{0});
    A2S_CHECK_EQ(options.flags, 0U);

    a2s_input input;
    std::memset(&input, 0xAB, sizeof(input));
    a2s_input_init(&input);
    A2S_CHECK_EQ(input.struct_size, static_cast<std::uint32_t>(sizeof(input)));
    A2S_CHECK(input.data == nullptr && input.size == 0 && input.entry_point == nullptr);
    A2S_CHECK(input.options == nullptr && input.function_constants == nullptr);

    // NULL is ignored.
    a2s_context_desc_init(nullptr);
    a2s_compile_options_init(nullptr);
    a2s_input_init(nullptr);
}

// ---- context ------------------------------------------------------------------------------------

A2S_TEST(context_create_rejects_null_out_pointer) {
    A2S_CHECK_EQ(a2s_context_create(nullptr, nullptr), A2S_ERROR_INVALID_ARGUMENT);
}

A2S_TEST(context_create_with_defaults) {
    auto ctx = make_context();
    a2s_context_desc desc;
    a2s_context_desc_init(&desc);
    auto ctx2 = make_context(&desc);
}

A2S_TEST(context_destroy_and_output_free_accept_null) {
    a2s_context_destroy(nullptr);
    a2s_output_free(nullptr);
}

A2S_TEST(context_create_validates_desc) {
    a2s_context* raw = reinterpret_cast<a2s_context*>(std::uintptr_t{0x1});
    a2s_context_desc desc;

    a2s_context_desc_init(&desc);
    desc.struct_size = 0;
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(raw == nullptr);

    a2s_context_desc_init(&desc);
    desc.struct_size = sizeof(desc) - 1;
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_INVALID_ARGUMENT);

    a2s_context_desc_init(&desc);
    desc.flags = 1;
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_INVALID_ARGUMENT);

    a2s_context_desc_init(&desc);
    desc.reserved = 1;
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(raw == nullptr);
}

A2S_TEST(context_create_checks_api_version) {
    a2s_context* raw = nullptr;
    a2s_context_desc desc;
    a2s_context_desc_init(&desc);
    desc.api_version = A2S_MAKE_API_VERSION(1, 0);
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_UNSUPPORTED);
    // While the major version is 0, minor versions must match exactly.
    desc.api_version = A2S_MAKE_API_VERSION(0, A2S_API_VERSION_MINOR + 1);
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_UNSUPPORTED);
    desc.api_version = A2S_MAKE_API_VERSION(0, 0);
    A2S_CHECK_EQ(a2s_context_create(&desc, &raw), A2S_ERROR_UNSUPPORTED);
    A2S_CHECK(raw == nullptr);
}

A2S_TEST(context_desc_accepts_larger_struct_with_zero_tail) {
    struct Extended {
        a2s_context_desc base;
        std::uint64_t future_field;
    };
    Extended ext{};
    a2s_context_desc_init(&ext.base);
    ext.base.struct_size = sizeof(Extended);
    ext.future_field = 0;
    a2s_context* raw = nullptr;
    A2S_CHECK_EQ(a2s_context_create(&ext.base, &raw), A2S_OK);
    a2s_context_destroy(raw);

    ext.future_field = 1; // a feature this library does not know: must not be ignored
    raw = nullptr;
    A2S_CHECK_EQ(a2s_context_create(&ext.base, &raw), A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(raw == nullptr);
}

// ---- compile: argument validation ---------------------------------------------------------------

A2S_TEST(compile_rejects_null_out_pointer) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    const a2s_input input = input_for(bytes);
    A2S_CHECK_EQ(a2s_compile(ctx.get(), &input, nullptr), A2S_ERROR_INVALID_ARGUMENT);
}

A2S_TEST(compile_rejects_null_context_with_diagnostic) {
    const auto bytes = a2s_test::make_raw_bitcode();
    const a2s_input input = input_for(bytes);
    auto r = compile(nullptr, &input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(r.output != nullptr);
    A2S_CHECK(diagnostics_contain(r.output.get(), "context"));
}

A2S_TEST(compile_rejects_null_input) {
    auto ctx = make_context();
    auto r = compile(ctx.get(), nullptr);
    A2S_CHECK_EQ(r.status, A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(diagnostics_contain(r.output.get(), "input"));
}

A2S_TEST(compile_rejects_bad_input_struct_size) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    a2s_input input = input_for(bytes);
    input.struct_size = 0;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    input.struct_size = 8;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
}

A2S_TEST(compile_rejects_null_data_with_size) {
    auto ctx = make_context();
    a2s_input input;
    a2s_input_init(&input);
    input.size = 16;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
}

A2S_TEST(compile_reports_empty_input_as_malformed) {
    auto ctx = make_context();
    a2s_input input;
    a2s_input_init(&input);
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(compile_validates_entry_point) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    a2s_input input = input_for(bytes);

    input.entry_point = "";
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    const std::string too_long(A2S_MAX_ENTRY_POINT_LENGTH + 1, 'x');
    input.entry_point = too_long.c_str();
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    const std::string longest(A2S_MAX_ENTRY_POINT_LENGTH, 'x');
    input.entry_point = longest.c_str();
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_UNIMPLEMENTED);

    // Untrusted names are escaped in diagnostics.
    input.entry_point = "evil\n\x1b[2Jname";
    auto r = compile(ctx.get(), &input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "evil\\x0a\\x1b[2Jname"));
}

A2S_TEST(compile_validates_options) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    a2s_input input = input_for(bytes);
    a2s_compile_options options;
    input.options = &options;

    a2s_compile_options_init(&options);
    options.struct_size = 4;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    a2s_compile_options_init(&options);
    options.flags = 0x80000000U;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    a2s_compile_options_init(&options);
    options.target_vulkan_version = A2S_VULKAN_VERSION(1, 2);
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_UNSUPPORTED);

    a2s_compile_options_init(&options);
    options.target_vulkan_version = A2S_VULKAN_VERSION(2, 0);
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_UNSUPPORTED);

    a2s_compile_options_init(&options);
    options.target_vulkan_version = A2S_VULKAN_VERSION(1, 3) | (1U << 29U);
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    a2s_compile_options_init(&options);
    options.min_subgroup_size = 3;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    a2s_compile_options_init(&options);
    options.max_subgroup_size = 256;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    a2s_compile_options_init(&options);
    options.min_subgroup_size = 32;
    options.max_subgroup_size = 16;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    // Valid, realistic options: Intel Xe-LP style range, newer Vulkan, unknown feature bits.
    a2s_compile_options_init(&options);
    options.target_vulkan_version = A2S_VULKAN_VERSION(1, 4) | 250U; // patch is ignored
    options.min_subgroup_size = 8;
    options.max_subgroup_size = 32;
    options.required_subgroup_size_stages = A2S_STAGE_BIT(A2S_STAGE_KERNEL) | 0x80000000U;
    options.features =
        A2S_FEATURE_BUFFER_DEVICE_ADDRESS | A2S_FEATURE_SHADER_FLOAT16 | (UINT64_C(1) << 63U);
    options.flags = A2S_COMPILE_FLAG_OPTIMIZE;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_UNIMPLEMENTED);
}

A2S_TEST(compile_validates_function_constants) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    a2s_input input = input_for(bytes);

    input.function_constant_count = 1;
    input.function_constants = nullptr;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);

    std::array<a2s_function_constant, 2> constants{};
    constants[0].index = 3;
    constants[0].type = A2S_CONSTANT_TYPE_FLOAT;
    constants[0].component_count = 4;
    constants[1].index = 0;
    constants[1].type = A2S_CONSTANT_TYPE_BOOL;
    constants[1].component_count = 1;
    constants[1].data[0] = 1;
    input.function_constants = constants.data();
    input.function_constant_count = 2;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_UNIMPLEMENTED);

    constants[1].data[0] = 2; // bool must be 0/1
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    constants[1].data[0] = 0;

    constants[1].index = 3; // duplicate
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    constants[1].index = 0;

    constants[0].type = 999;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    constants[0].type = A2S_CONSTANT_TYPE_FLOAT;

    constants[0].component_count = 5;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    constants[0].component_count = 0;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
    constants[0].component_count = 4;

    constants[0].reserved = 1;
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_INVALID_ARGUMENT);
}

// ---- compile: input classification
// ---------------------------------------------------------------

A2S_TEST(compile_returns_unimplemented_for_raw_bitcode) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode(64);
    const a2s_input input = input_for(bytes);
    auto r = compile(ctx.get(), &input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "not implemented"));
    A2S_CHECK(diagnostics_contain(r.output.get(), "note: input is raw LLVM bitcode"));

    a2s_reflection reflection{};
    reflection.struct_size = sizeof(reflection);
    A2S_CHECK_EQ(a2s_output_get_reflection(r.output.get(), &reflection), A2S_ERROR_UNIMPLEMENTED);
    a2s_binding binding{};
    binding.struct_size = sizeof(binding);
    A2S_CHECK_EQ(a2s_output_get_binding(r.output.get(), 0, &binding), A2S_ERROR_UNIMPLEMENTED);
}

A2S_TEST(compile_returns_unimplemented_for_wrapped_bitcode) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_valid_wrapper();
    a2s_input input = input_for(bytes);
    input.entry_point = "kernel_main";
    auto r = compile(ctx.get(), &input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "bitcode wrapper"));
    A2S_CHECK(diagnostics_contain(r.output.get(), "kernel_main"));
}

A2S_TEST(compile_reports_malformed_wrapper) {
    auto ctx = make_context();
    a2s_test::WrapperFields f;
    f.size = 1000;
    const auto payload = a2s_test::make_raw_bitcode(16);
    const auto bytes = a2s_test::make_wrapper(f, payload);
    const a2s_input input = input_for(bytes);
    A2S_CHECK_EQ(compile(ctx.get(), &input).status, A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(compile_reports_unknown_format_as_unsupported) {
    auto ctx = make_context();
    const std::vector<std::uint8_t> bytes = {'n', 'o', 't', ' ', 'a', 'i', 'r', '!'};
    const a2s_input input = input_for(bytes);
    auto r = compile(ctx.get(), &input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNSUPPORTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "unrecognized input format"));
    A2S_CHECK(diagnostics_contain(r.output.get(), "ADR-0008"));
}

A2S_TEST(compile_enforces_context_input_limit) {
    a2s_context_desc desc;
    a2s_context_desc_init(&desc);
    desc.max_input_size = 32;
    auto ctx = make_context(&desc);
    const auto small = a2s_test::make_raw_bitcode(32);
    const auto large = a2s_test::make_raw_bitcode(36);
    const a2s_input small_input = input_for(small);
    const a2s_input large_input = input_for(large);
    A2S_CHECK_EQ(compile(ctx.get(), &small_input).status, A2S_ERROR_UNIMPLEMENTED);
    auto r = compile(ctx.get(), &large_input);
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNSUPPORTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "exceeds"));
}

// ---- outputs
// -------------------------------------------------------------------------------------

A2S_TEST(output_getters_handle_null) {
    A2S_CHECK_EQ(a2s_output_get_status(nullptr), A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK(a2s_output_get_diagnostics(nullptr) != nullptr);
    A2S_CHECK_EQ(std::string(a2s_output_get_diagnostics(nullptr)), std::string());
    std::size_t words = 7;
    A2S_CHECK(a2s_output_get_spirv(nullptr, &words) == nullptr);
    A2S_CHECK_EQ(words, std::size_t{0});
    a2s_reflection reflection{};
    reflection.struct_size = sizeof(reflection);
    A2S_CHECK_EQ(a2s_output_get_reflection(nullptr, &reflection), A2S_ERROR_INVALID_ARGUMENT);
    a2s_binding binding{};
    binding.struct_size = sizeof(binding);
    A2S_CHECK_EQ(a2s_output_get_binding(nullptr, 0, &binding), A2S_ERROR_INVALID_ARGUMENT);

    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    const a2s_input input = input_for(bytes);
    auto r = compile(ctx.get(), &input);
    A2S_CHECK(a2s_output_get_spirv(r.output.get(), nullptr) == nullptr);
    A2S_CHECK_EQ(a2s_output_get_reflection(r.output.get(), nullptr), A2S_ERROR_INVALID_ARGUMENT);
    A2S_CHECK_EQ(a2s_output_get_binding(r.output.get(), 0, nullptr), A2S_ERROR_INVALID_ARGUMENT);
}

A2S_TEST(output_outlives_context) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_raw_bitcode();
    const a2s_input input = input_for(bytes);
    auto r = compile(ctx.get(), &input);
    ctx.reset();
    A2S_CHECK_EQ(a2s_output_get_status(r.output.get()), A2S_ERROR_UNIMPLEMENTED);
    A2S_CHECK(diagnostics_contain(r.output.get(), "not implemented"));
}

A2S_TEST(compile_is_thread_safe_on_shared_context) {
    auto ctx = make_context();
    const auto raw = a2s_test::make_raw_bitcode(64);
    const auto wrapped = a2s_test::make_valid_wrapper();
    constexpr int kThreads = 8;
    constexpr int kIterations = 200;
    std::array<int, kThreads> mismatches{};
    std::vector<std::thread> threads;
    threads.reserve(kThreads);
    for (int t = 0; t < kThreads; ++t) {
        threads.emplace_back([&, t] {
            const a2s_input input = input_for((t % 2) == 0 ? raw : wrapped);
            for (int i = 0; i < kIterations; ++i) {
                a2s_output* out = nullptr;
                const a2s_status s = a2s_compile(ctx.get(), &input, &out);
                if (s != A2S_ERROR_UNIMPLEMENTED || out == nullptr ||
                    a2s_output_get_status(out) != s) {
                    ++mismatches[static_cast<std::size_t>(t)];
                }
                a2s_output_free(out);
            }
        });
    }
    for (auto& thread : threads) {
        thread.join();
    }
    for (const int m : mismatches) {
        A2S_CHECK_EQ(m, 0);
    }
}

A2S_TEST(compile_is_deterministic) {
    auto ctx = make_context();
    const auto bytes = a2s_test::make_valid_wrapper();
    const a2s_input input = input_for(bytes);
    auto a = compile(ctx.get(), &input);
    auto b = compile(ctx.get(), &input);
    A2S_CHECK_EQ(a.status, b.status);
    A2S_CHECK_EQ(std::string(a2s_output_get_diagnostics(a.output.get())),
                 std::string(a2s_output_get_diagnostics(b.output.get())));
}
