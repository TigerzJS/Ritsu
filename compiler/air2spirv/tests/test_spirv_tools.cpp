// Tests for the in-process SPIRV-Tools wrapper (src/spirv/tools.*, WP-SC-03).
#include "air2spirv/air2spirv.h"

#include "spirv/tools.hpp"
#include "spirv_test_modules.hpp"
#include "test_harness.hpp"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

using namespace air2spirv::spirv;
using a2s_test::build_compute_bda_module;
using a2s_test::ComputeDefect;

namespace {

void print_log(const std::string& log) {
    if (!log.empty()) {
        std::fprintf(stderr, "%s", log.c_str());
    }
}

} // namespace

// ---- validation --------------------------------------------------------------------------------

A2S_TEST(short_or_garbage_input_is_rejected_without_crashing) {
    const ValidationResult empty = validate({}, TargetEnv::Vulkan1_3);
    A2S_CHECK(!empty.valid);
    A2S_CHECK(empty.log.find("fewer than the 5-word") != std::string::npos);
    const Words four = {0x07230203U, 0x00010600U, 0U, 1U};
    A2S_CHECK(!validate(four, TargetEnv::Vulkan1_3).valid);

    Words wrong_magic = build_compute_bda_module();
    wrong_magic[0] = 0xDEADBEEFU;
    const ValidationResult magic = validate(wrong_magic, TargetEnv::Vulkan1_3);
    A2S_CHECK(!magic.valid);
    A2S_CHECK(!magic.log.empty());

    Words no_function_end = build_compute_bda_module();
    no_function_end.pop_back(); // drops the one-word OpFunctionEnd
    A2S_CHECK(!validate(no_function_end, TargetEnv::Vulkan1_3).valid);

    Words truncated = build_compute_bda_module();
    truncated.resize(truncated.size() - 3); // OpFunctionEnd, OpReturn, last word of OpStore
    A2S_CHECK(!validate(truncated, TargetEnv::Vulkan1_3).valid);

    Words bad_bound = build_compute_bda_module();
    bad_bound[3] = 2U; // ids exceed the declared bound
    A2S_CHECK(!validate(bad_bound, TargetEnv::Vulkan1_3).valid);
}

A2S_TEST(header_only_module_is_invalid_for_vulkan) {
    // Structurally parseable, but Vulkan needs at least Shader + a memory model.
    const Words header = {0x07230203U, 0x00010600U, 0U, 1U, 0U};
    A2S_CHECK(!validate(header, TargetEnv::Vulkan1_3).valid);
}

A2S_TEST(spirv_version_newer_than_the_target_is_rejected) {
    // The Vulkan 1.3 environment accepts SPIR-V <= 1.6; air2spirv only emits 1.6.
    Words module = build_compute_bda_module();
    module[1] = 0x00010700U; // SPIR-V 1.7
    A2S_CHECK(!validate(module, TargetEnv::Vulkan1_3).valid);
}

A2S_TEST(validator_settings_are_applied) {
    // vec3 at offset 8 straddles a 16-byte boundary: invalid even with the relaxed block layout
    // of Vulkan >= 1.1, legal with scalar block layout.
    const auto assembled = assemble(R"(
               OpCapability Shader
               OpMemoryModel Logical GLSL450
               OpEntryPoint GLCompute %main "main" %buf
               OpExecutionMode %main LocalSize 1 1 1
               OpMemberDecorate %S 0 Offset 0
               OpMemberDecorate %S 1 Offset 8
               OpDecorate %S Block
               OpDecorate %buf DescriptorSet 0
               OpDecorate %buf Binding 0
       %void = OpTypeVoid
         %fn = OpTypeFunction %void
      %float = OpTypeFloat 32
       %vec3 = OpTypeVector %float 3
          %S = OpTypeStruct %float %vec3
      %S_ptr = OpTypePointer StorageBuffer %S
        %buf = OpVariable %S_ptr StorageBuffer
       %main = OpFunction %void None %fn
      %entry = OpLabel
               OpReturn
               OpFunctionEnd
)",
                                    TargetEnv::Vulkan1_3);
    print_log(assembled.log);
    A2S_CHECK(assembled.ok);
    A2S_CHECK(!validate(assembled.words, TargetEnv::Vulkan1_3).valid);
    ValidatorSettings scalar;
    scalar.scalar_block_layout = true;
    const ValidationResult relaxed = validate(assembled.words, TargetEnv::Vulkan1_3, scalar);
    print_log(relaxed.log);
    A2S_CHECK(relaxed.valid);
}

A2S_TEST(validation_is_reentrant_across_threads) {
    const Words good = build_compute_bda_module();
    const Words bad = build_compute_bda_module(ComputeDefect::MissingLocalSize);
    std::atomic<int> wrong{0};
    std::vector<std::thread> threads;
    for (int t = 0; t < 4; ++t) {
        threads.emplace_back([&] {
            for (int i = 0; i < 25; ++i) {
                if (!validate(good, TargetEnv::Vulkan1_3).valid ||
                    validate(bad, TargetEnv::Vulkan1_3).valid) {
                    ++wrong;
                }
            }
        });
    }
    for (std::thread& thread : threads) {
        thread.join();
    }
    A2S_CHECK_EQ(wrong.load(), 0);
}

// ---- disassembly / assembly --------------------------------------------------------------------

A2S_TEST(disassembly_is_deterministic_and_configurable) {
    const Words module = build_compute_bda_module();
    const DisassemblyResult first = disassemble(module);
    const DisassemblyResult second = disassemble(module);
    A2S_CHECK(first.ok);
    A2S_CHECK(first.text == second.text);
    A2S_CHECK(first.text.find("OpEntryPoint GLCompute %main \"main\"") != std::string::npos);
    A2S_CHECK(first.text.find("; SPIR-V") == std::string::npos);

    DisassemblyOptions options;
    options.header = true;
    options.friendly_names = false;
    options.indent = true;
    const DisassemblyResult raw = disassemble(module, options);
    A2S_CHECK(raw.ok);
    A2S_CHECK(raw.text.find("; Version: 1.6") != std::string::npos);
    A2S_CHECK(raw.text.find("OpEntryPoint GLCompute %1 \"main\"") != std::string::npos);
}

A2S_TEST(disassembly_of_truncated_module_fails_cleanly) {
    Words truncated = build_compute_bda_module();
    truncated.resize(truncated.size() - 3); // ends inside the 5-word OpStore
    const DisassemblyResult result = disassemble(truncated);
    A2S_CHECK(!result.ok);
    A2S_CHECK(result.text.empty());
    A2S_CHECK(!result.log.empty());
    A2S_CHECK(!disassemble({}).ok);
}

A2S_TEST(assembly_roundtrip_preserves_instruction_words) {
    const Words module = build_compute_bda_module();
    DisassemblyOptions raw;
    raw.friendly_names = false;
    const DisassemblyResult text = disassemble(module, raw);
    A2S_CHECK(text.ok);
    const AssemblyResult back = assemble(text.text, TargetEnv::Vulkan1_3, true);
    print_log(back.log);
    A2S_CHECK(back.ok);
    // Header words 2 (generator) and 3 (bound) are chosen by the assembler; the instruction
    // stream must be identical.
    A2S_CHECK(back.words.size() == module.size());
    A2S_CHECK(Words(back.words.begin() + 5, back.words.end()) ==
              Words(module.begin() + 5, module.end()));
    A2S_CHECK_EQ(back.words[1], module[1]);
}

A2S_TEST(assembly_errors_are_reported) {
    const AssemblyResult result =
        assemble("OpCapability Shader\n%1 = OpNoSuchOpcode\n", TargetEnv::Vulkan1_3);
    A2S_CHECK(!result.ok);
    A2S_CHECK(result.words.empty());
    A2S_CHECK(result.log.find("error: [line 2:") != std::string::npos);
}

// ---- optimization ------------------------------------------------------------------------------

A2S_TEST(optimizer_output_is_valid_and_deterministic) {
    const Words module = build_compute_bda_module();
    const std::vector<std::string> passes = {"--eliminate-dead-code-aggressive", "--strip-debug"};
    const OptimizationResult first = optimize(module, TargetEnv::Vulkan1_3, passes);
    print_log(first.log);
    A2S_CHECK(first.ok);
    const OptimizationResult second = optimize(module, TargetEnv::Vulkan1_3, passes);
    A2S_CHECK(first.words == second.words);
    A2S_CHECK(validate(first.words, TargetEnv::Vulkan1_3).valid);
    const auto checked = a2s_test::check_spirv(first.words, R"(
; CHECK-NOT: OpName
; CHECK: OpEntryPoint GLCompute
; CHECK: OpStore {{%[0-9]+}} {{%[0-9]+}} Aligned 4
)");
    if (!checked.ok) {
        std::fprintf(stderr, "%s\n", checked.message.c_str());
    }
    A2S_CHECK(checked.ok);
}

A2S_TEST(optimizer_performance_preset_keeps_module_valid) {
    const std::vector<std::string> passes = {"-O"};
    const OptimizationResult result =
        optimize(build_compute_bda_module(), TargetEnv::Vulkan1_3, passes);
    print_log(result.log);
    A2S_CHECK(result.ok);
    A2S_CHECK(validate(result.words, TargetEnv::Vulkan1_3).valid);
}

A2S_TEST(optimizer_with_no_passes_returns_an_equivalent_module) {
    const Words module = build_compute_bda_module();
    const OptimizationResult result = optimize(module, TargetEnv::Vulkan1_3, {});
    A2S_CHECK(result.ok);
    A2S_CHECK(result.words == module);
}

A2S_TEST(optimizer_rejects_unknown_passes_and_invalid_input) {
    const std::vector<std::string> unknown = {"--no-such-pass-exists"};
    const OptimizationResult bad_pass =
        optimize(build_compute_bda_module(), TargetEnv::Vulkan1_3, unknown);
    A2S_CHECK(!bad_pass.ok);
    A2S_CHECK(!bad_pass.log.empty());

    const std::vector<std::string> passes = {"--eliminate-dead-code-aggressive"};
    const OptimizationResult bad_input =
        optimize(build_compute_bda_module(ComputeDefect::MissingAlignedOnPhysicalStorageStore),
                 TargetEnv::Vulkan1_3, passes);
    A2S_CHECK(!bad_input.ok);
    A2S_CHECK(bad_input.words.empty());
    A2S_CHECK(!bad_input.log.empty());

    A2S_CHECK(!optimize({}, TargetEnv::Vulkan1_3, passes).ok);
}

// ---- environment and version -------------------------------------------------------------------

A2S_TEST(target_env_mapping) {
    A2S_CHECK(target_env_for_vulkan(1, 3) == TargetEnv::Vulkan1_3);
    A2S_CHECK(target_env_for_vulkan(1, 4) == TargetEnv::Vulkan1_4);
    A2S_CHECK(target_env_for_vulkan(1, 9) == TargetEnv::Vulkan1_4);
    A2S_CHECK(std::string_view(target_env_name(TargetEnv::Vulkan1_3)).find("SPIR-V 1.6") !=
              std::string_view::npos);
}

A2S_TEST(version_string_names_the_spirv_tools_version) {
    const std::string version = a2s_get_version_string();
    const std::string tools = spirv_tools_version();
    A2S_CHECK(!tools.empty());
    A2S_CHECK(version.starts_with("air2spirv "));
    A2S_CHECK(version.find("spirv-tools " + tools + " [") != std::string::npos);
    // Package identity or build details follow in brackets and are never empty or truncated.
    A2S_CHECK(version.ends_with("])"));
    A2S_CHECK(version.find(" []") == std::string::npos);
    // Stable storage: the same pointer every time.
    A2S_CHECK(a2s_get_version_string() == a2s_get_version_string());
}
