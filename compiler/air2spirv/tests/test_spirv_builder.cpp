// Tests for the deterministic SPIR-V module builder (src/spirv/module_builder.*, WP-SC-03).
#include "diagnostics.hpp"
#include "spirv/module_builder.hpp"
#include "spirv/tools.hpp"
#include "spirv_test_modules.hpp"
#include "test_harness.hpp"

#include <array>
#include <cstdint>
#include <cstdio>
#include <functional>
#include <string>
#include <string_view>
#include <vector>

using namespace air2spirv::spirv;
using a2s_test::build_compute_bda_module;
using a2s_test::ComputeDefect;

namespace {

bool is_valid(const Words& words, TargetEnv env = TargetEnv::Vulkan1_3) {
    const ValidationResult result = validate(words, env);
    if (!result.valid) {
        std::fprintf(stderr, "spirv-val:\n%s", result.log.c_str());
    }
    return result.valid;
}

bool throws_builder_error(const std::function<void()>& fn) {
    try {
        fn();
    } catch (const BuilderError&) {
        return true;
    }
    return false;
}

// A minimal valid compute module around a caller-provided body (types may be added freely).
ModuleBuilder minimal_compute_prefix(Id& main, Id& void_type, Id& main_type) {
    ModuleBuilder b;
    b.add_capability(spv::Capability::Shader);
    b.set_memory_model(spv::AddressingModel::Logical, spv::MemoryModel::GLSL450);
    main = b.reserve_id();
    void_type = b.type_void();
    main_type = b.type_function(void_type, {});
    b.add_entry_point(spv::ExecutionModel::GLCompute, main, "main", {});
    b.add_execution_mode(main, spv::ExecutionMode::LocalSize, {1, 1, 1});
    return b;
}

// Opcodes of all instructions after the header, in order.
std::vector<spv::Op> opcodes(const Words& words) {
    std::vector<spv::Op> ops;
    for (std::size_t i = 5; i < words.size();) {
        const std::uint32_t count = words[i] >> 16U;
        ops.push_back(static_cast<spv::Op>(words[i] & 0xFFFFU));
        if (count == 0) {
            break;
        }
        i += count;
    }
    return ops;
}

} // namespace

// ---- acceptance: hand-built compute module ---------------------------------------------------

A2S_TEST(compute_bda_module_passes_spirv_val_vulkan_1_3) {
    A2S_CHECK(is_valid(build_compute_bda_module(), TargetEnv::Vulkan1_3));
}

A2S_TEST(compute_bda_module_passes_spirv_val_vulkan_1_4) {
    A2S_CHECK(is_valid(build_compute_bda_module(), TargetEnv::Vulkan1_4));
}

A2S_TEST(compute_bda_module_without_debug_names_passes_spirv_val) {
    A2S_CHECK(is_valid(build_compute_bda_module(ComputeDefect::None, false)));
}

A2S_TEST(every_deliberately_broken_module_is_rejected) {
    for (const ComputeDefect defect : a2s_test::kAllComputeDefects) {
        const Words words = build_compute_bda_module(defect);
        const ValidationResult result = validate(words, TargetEnv::Vulkan1_3);
        if (result.valid || result.log.empty()) {
            std::fprintf(stderr, "defect %s was not rejected with a message\n",
                         a2s_test::defect_name(defect));
        }
        A2S_CHECK(!result.valid);
        A2S_CHECK(result.log.find("error: ") != std::string::npos);
    }
}

A2S_TEST(broken_modules_name_the_violated_rule) {
    // Stable VUIDs from the Vulkan spec; message wording may change between SPIRV-Tools versions.
    const ValidationResult no_local_size =
        validate(build_compute_bda_module(ComputeDefect::MissingLocalSize), TargetEnv::Vulkan1_3);
    A2S_CHECK(no_local_size.log.find("VUID-StandaloneSpirv-LocalSize-06426") != std::string::npos);
    const ValidationResult no_aligned =
        validate(build_compute_bda_module(ComputeDefect::MissingAlignedOnPhysicalStorageStore),
                 TargetEnv::Vulkan1_3);
    A2S_CHECK(no_aligned.log.find("VUID-StandaloneSpirv-PhysicalStorageBuffer64-04708") !=
              std::string::npos);
    const ValidationResult no_block = validate(
        build_compute_bda_module(ComputeDefect::MissingBlockOnPushConstant), TargetEnv::Vulkan1_3);
    A2S_CHECK(no_block.log.find("VUID-StandaloneSpirv-PushConstant-06675") != std::string::npos);
    // A core SPIR-V rule without a Vulkan VUID: check the capability named in the message.
    const ValidationResult no_capability =
        validate(build_compute_bda_module(ComputeDefect::MissingPhysicalStorageBufferCapability),
                 TargetEnv::Vulkan1_3);
    A2S_CHECK(no_capability.log.find(
                  "requires one of these capabilities: PhysicalStorageBufferAddresses") !=
              std::string::npos);
}

A2S_TEST(compute_bda_module_matches_check_patterns) {
    const auto result = a2s_test::check_spirv(build_compute_bda_module(), R"(
; CHECK-DAG: OpCapability Shader
; CHECK-DAG: OpCapability PhysicalStorageBufferAddresses
; CHECK: OpMemoryModel PhysicalStorageBuffer64 GLSL450
; CHECK-NEXT: OpEntryPoint GLCompute %main "main" %gl_GlobalInvocationID %push_constants
; CHECK-NEXT: OpExecutionMode %main LocalSize 64 1 1
; CHECK: OpDecorate %gl_GlobalInvocationID BuiltIn GlobalInvocationId
; CHECK: OpDecorate [[ARRAY:%[A-Za-z0-9_]+]] ArrayStride 4
; CHECK: OpDecorate %PushConstants Block
; CHECK: [[UINT:%[a-z0-9_]+]] = OpTypeInt 32 0
; CHECK: [[ARRAY]] = OpTypeRuntimeArray [[UINT]]
; CHECK-NEXT: %Buffer = OpTypeStruct [[ARRAY]]
; CHECK-NEXT: [[BDA:%[A-Za-z0-9_]+]] = OpTypePointer PhysicalStorageBuffer %Buffer
; CHECK: %main = OpFunction %void None
; CHECK-NEXT: OpLabel
; CHECK-NOT: OpUnreachable
; CHECK: [[PTR:%[0-9]+]] = OpLoad [[BDA]]
; CHECK-NEXT: [[ELEM:%[0-9]+]] = OpAccessChain {{%[A-Za-z0-9_]+}} [[PTR]] %uint_0
; CHECK-NEXT: OpStore [[ELEM]] {{%[0-9]+}} Aligned 4
; CHECK-NEXT: OpReturn
; CHECK-NEXT: OpFunctionEnd
)");
    if (!result.ok) {
        std::fprintf(stderr, "%s\n", result.message.c_str());
    }
    A2S_CHECK(result.ok);
}

// ---- determinism -------------------------------------------------------------------------------

A2S_TEST(same_module_twice_is_bytewise_identical) {
    const Words first = build_compute_bda_module();
    const Words second = build_compute_bda_module();
    A2S_CHECK(!first.empty());
    A2S_CHECK(first == second);
    const Words fwd_first = a2s_test::build_forward_pointer_module();
    A2S_CHECK(fwd_first == a2s_test::build_forward_pointer_module());
}

A2S_TEST(finish_is_repeatable) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    const Words once = b.finish();
    A2S_CHECK(once == b.finish());
    A2S_CHECK(is_valid(once));
}

A2S_TEST(capability_and_extension_order_does_not_matter) {
    auto build = [](bool reversed) {
        ModuleBuilder b;
        const std::array caps = {spv::Capability::Shader, spv::Capability::Int64,
                                 spv::Capability::Float16, spv::Capability::Int8};
        const std::array<std::string_view, 2> exts = {"SPV_KHR_b", "SPV_KHR_a"};
        for (std::size_t i = 0; i < caps.size(); ++i) {
            b.add_capability(caps[reversed ? caps.size() - 1 - i : i]);
            b.add_capability(spv::Capability::Shader); // duplicates collapse
        }
        for (std::size_t i = 0; i < exts.size(); ++i) {
            b.add_extension(exts[reversed ? exts.size() - 1 - i : i]);
        }
        b.set_memory_model(spv::AddressingModel::Logical, spv::MemoryModel::GLSL450);
        return b.finish();
    };
    const Words forward = build(false);
    A2S_CHECK(forward == build(true));
    const std::vector<spv::Op> ops = opcodes(forward);
    A2S_CHECK_EQ(ops.size(), std::size_t{4 + 2 + 1});
    // Capabilities sorted by enum value: Shader(1) Float16(9) Int64(11) Int8(39).
    A2S_CHECK_EQ(forward[6], static_cast<std::uint32_t>(spv::Capability::Shader));
    A2S_CHECK_EQ(forward[8], static_cast<std::uint32_t>(spv::Capability::Float16));
    A2S_CHECK_EQ(forward[10], static_cast<std::uint32_t>(spv::Capability::Int64));
    A2S_CHECK_EQ(forward[12], static_cast<std::uint32_t>(spv::Capability::Int8));
    // Extensions sorted by name: "SPV_KHR_a" ("SPV_" "KHR_" "a\0\0\0") first.
    A2S_CHECK_EQ(forward[13], (4U << 16U) | static_cast<std::uint32_t>(spv::Op::OpExtension));
    A2S_CHECK_EQ(forward[16], static_cast<std::uint32_t>('a'));
}

A2S_TEST(debug_names_do_not_change_ids) {
    DisassemblyOptions raw;
    raw.friendly_names = false;
    const auto with_names = disassemble(build_compute_bda_module(ComputeDefect::None, true), raw);
    const auto without = disassemble(build_compute_bda_module(ComputeDefect::None, false), raw);
    A2S_CHECK(with_names.ok && without.ok);
    std::string filtered;
    std::string_view rest = with_names.text;
    while (!rest.empty()) {
        const std::size_t end = rest.find('\n');
        const std::string_view line =
            rest.substr(0, end == std::string_view::npos ? rest.size() : end + 1);
        rest.remove_prefix(line.size());
        if (line.find("OpName") == std::string_view::npos &&
            line.find("OpMemberName") == std::string_view::npos) {
            filtered += line;
        }
    }
    A2S_CHECK(filtered == without.text);
    A2S_CHECK(with_names.text.size() > without.text.size());
}

// ---- header and encoding -----------------------------------------------------------------------

A2S_TEST(header_is_spirv_1_6) {
    const Words words = build_compute_bda_module();
    A2S_CHECK(words.size() > 5);
    A2S_CHECK_EQ(words[0], 0x07230203U);
    A2S_CHECK_EQ(words[1], 0x00010600U);
    A2S_CHECK_EQ(words[2], kGeneratorMagic);
    A2S_CHECK_EQ(words[4], 0U);
    // Bound = largest id + 1; the compute module uses ids 1..23 without gaps.
    A2S_CHECK_EQ(words[3], 24U);
}

A2S_TEST(literal_strings_are_nul_terminated_and_padded) {
    Words w;
    append_literal_string(w, "");
    A2S_CHECK(w == Words{0U});
    w.clear();
    append_literal_string(w, "abc");
    A2S_CHECK(w == Words{0x00636261U});
    w.clear();
    append_literal_string(w, "abcd");
    A2S_CHECK(w == (Words{0x64636261U, 0U}));
    w.clear();
    append_literal_string(w, "abcde");
    A2S_CHECK(w == (Words{0x64636261U, 0x65U}));
    w.clear();
    append_literal_string(w, "\xC3\xA9"); // UTF-8 bytes are copied unchanged
    A2S_CHECK(w == Words{0x0000A9C3U});
    A2S_CHECK(throws_builder_error([] {
        Words x;
        append_literal_string(x, std::string_view("a\0b", 3));
    }));
}

// ---- deduplication -----------------------------------------------------------------------------

A2S_TEST(types_are_deduplicated_except_structs) {
    ModuleBuilder b;
    const Id u32 = b.type_int(32, false);
    A2S_CHECK_EQ(b.type_int(32, false), u32);
    A2S_CHECK(b.type_int(32, true) != u32);
    A2S_CHECK(b.type_int(64, false) != u32);
    const Id f32 = b.type_float(32);
    A2S_CHECK_EQ(b.type_float(32), f32);
    A2S_CHECK_EQ(b.type_vector(f32, 4), b.type_vector(f32, 4));
    A2S_CHECK(b.type_vector(f32, 4) != b.type_vector(f32, 3));
    A2S_CHECK_EQ(b.type_pointer(spv::StorageClass::Private, f32),
                 b.type_pointer(spv::StorageClass::Private, f32));
    A2S_CHECK(b.type_pointer(spv::StorageClass::Private, f32) !=
              b.type_pointer(spv::StorageClass::Function, f32));
    A2S_CHECK_EQ(b.type_function(b.type_void(), std::array{u32, f32}),
                 b.type_function(b.type_void(), std::array{u32, f32}));
    A2S_CHECK(b.type_function(b.type_void(), std::array{u32, f32}) !=
              b.type_function(b.type_void(), std::array{f32, u32}));
    const std::array members{u32};
    A2S_CHECK(b.type_struct(members) != b.type_struct(members));
}

A2S_TEST(array_stride_is_part_of_the_type_identity) {
    ModuleBuilder b;
    const Id u32 = b.type_int(32, false);
    const Id rta4 = b.type_runtime_array(u32, 4);
    A2S_CHECK_EQ(b.type_runtime_array(u32, 4), rta4);
    A2S_CHECK(b.type_runtime_array(u32, 8) != rta4);
    A2S_CHECK(b.type_runtime_array(u32) != rta4);
    const Id len = b.constant_u32(16);
    const Id arr = b.type_array(u32, len, 4);
    A2S_CHECK_EQ(b.type_array(u32, len, 4), arr);
    A2S_CHECK(b.type_array(u32, len) != arr);
}

A2S_TEST(constants_are_deduplicated_by_type_and_bits) {
    ModuleBuilder b;
    const Id seven = b.constant_u32(7);
    A2S_CHECK_EQ(b.constant_u32(7), seven);
    A2S_CHECK(b.constant_i32(7) != seven); // different type
    A2S_CHECK(b.constant_u32(8) != seven);
    A2S_CHECK(b.constant_f32(0.0F) != b.constant_f32(-0.0F));
    A2S_CHECK_EQ(b.constant_f32(1.5F), b.constant_f32(1.5F));
    A2S_CHECK_EQ(b.constant_bool(true), b.constant_bool(true));
    A2S_CHECK(b.constant_bool(true) != b.constant_bool(false));
    const Id v2 = b.type_vector(b.type_int(32, false), 2);
    const Id composite = b.constant_composite(v2, std::array{seven, seven});
    A2S_CHECK_EQ(b.constant_composite(v2, std::array{seven, seven}), composite);
    A2S_CHECK_EQ(b.constant_null(v2), b.constant_null(v2));
    const std::uint32_t big[] = {1U, 2U};
    A2S_CHECK_EQ(b.constant(b.type_int(64, false), big), b.constant(b.type_int(64, false), big));
}

A2S_TEST(extended_instruction_imports_and_decorations_are_deduplicated) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
    const Id glsl = b.import_extended_instructions("GLSL.std.450");
    A2S_CHECK_EQ(b.import_extended_instructions("GLSL.std.450"), glsl);
    const Id f32 = b.type_float(32);
    const Id one = b.constant_f32(1.0F);
    const Id local_ptr = b.type_pointer(spv::StorageClass::Function, f32);
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    const Id local = b.emit(spv::Op::OpVariable, local_ptr,
                            {static_cast<std::uint32_t>(spv::StorageClass::Function)});
    // GLSL.std.450 Sqrt = 31
    const Id root = b.emit(spv::Op::OpExtInst, f32, {glsl, 31U, one});
    b.decorate(root, spv::Decoration::RelaxedPrecision);
    b.decorate(root, spv::Decoration::RelaxedPrecision);
    b.emit(spv::Op::OpStore, kNoId, {local, root});
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    const Words words = b.finish();
    A2S_CHECK(is_valid(words));
    const auto result = a2s_test::check_spirv(words, R"(
; CHECK: OpExtInstImport "GLSL.std.450"
; CHECK-NOT: OpExtInstImport
; CHECK: OpDecorate [[ROOT:%[0-9]+]] RelaxedPrecision
; CHECK-NOT: RelaxedPrecision
; CHECK: [[ROOT]] = OpExtInst %float {{%[A-Za-z0-9_.]+}} Sqrt %float_1
)");
    if (!result.ok) {
        std::fprintf(stderr, "%s\n", result.message.c_str());
    }
    A2S_CHECK(result.ok);
}

A2S_TEST(forward_pointer_module_passes_spirv_val) {
    const Words words = a2s_test::build_forward_pointer_module();
    A2S_CHECK(is_valid(words));
    const auto result = a2s_test::check_spirv(words, R"(
; CHECK: OpTypeForwardPointer [[P:%[A-Za-z0-9_]+]] PhysicalStorageBuffer
; CHECK-NEXT: %Node = OpTypeStruct [[P]] %uint
; CHECK-NEXT: [[P]] = OpTypePointer PhysicalStorageBuffer %Node
; CHECK-NOT: OpTypePointer PhysicalStorageBuffer %Node
; CHECK: OpFunctionEnd
)");
    if (!result.ok) {
        std::fprintf(stderr, "%s\n", result.message.c_str());
    }
    A2S_CHECK(result.ok);
}

// ---- misuse ------------------------------------------------------------------------------------

A2S_TEST(structural_misuse_throws) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    {
        ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
        A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpReturn, kNoId, {}); }));
        A2S_CHECK(throws_builder_error([&] { b.begin_block(); })); // no function
        A2S_CHECK(throws_builder_error([&] { b.end_function(); }));
        b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
        A2S_CHECK(throws_builder_error([&] { (void)b.finish(); })); // open function
        A2S_CHECK(throws_builder_error(
            [&] { b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type); }));
        b.begin_block();
        A2S_CHECK(throws_builder_error([&] { (void)b.add_parameter(void_type); }));
        A2S_CHECK(throws_builder_error([&] { b.begin_block(); }));  // no terminator yet
        A2S_CHECK(throws_builder_error([&] { b.end_function(); })); // open block
        b.emit(spv::Op::OpReturn, kNoId, {});
        A2S_CHECK(!b.in_block());
        A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpNop, kNoId, {}); }));
        b.end_function();
        A2S_CHECK(is_valid(b.finish()));
    }
    {
        ModuleBuilder b;
        A2S_CHECK(throws_builder_error([&] { (void)b.finish(); })); // no memory model
        b.set_memory_model(spv::AddressingModel::Logical, spv::MemoryModel::GLSL450);
        b.set_memory_model(spv::AddressingModel::Logical, spv::MemoryModel::GLSL450); // same: ok
        A2S_CHECK(throws_builder_error(
            [&] { b.set_memory_model(spv::AddressingModel::Logical, spv::MemoryModel::Vulkan); }));
        A2S_CHECK(throws_builder_error([&] {
            b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, 9999U);
        })); // id never reserved
    }
}

A2S_TEST(emit_checks_result_type_and_rejects_declarations) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
    const Id u32 = b.type_int(32, false);
    const Id one = b.constant_u32(1);
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpIAdd, kNoId, {one, one}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpReturn, u32, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpTypeInt, kNoId, {16, 0}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpLabel, kNoId, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpFunctionEnd, kNoId, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpConstant, u32, {5}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpDecorate, kNoId, {one, 0}); }));
    const Id sum = b.emit(spv::Op::OpIAdd, u32, {one, one});
    A2S_CHECK(sum != kNoId);
    A2S_CHECK_EQ(b.emit(spv::Op::OpNop, kNoId, {}), kNoId);
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    A2S_CHECK(is_valid(b.finish()));
}

A2S_TEST(invalid_operands_throw) {
    ModuleBuilder b;
    const Id u32 = b.type_int(32, false);
    A2S_CHECK(throws_builder_error([&] { (void)b.type_vector(kNoId, 4); }));
    A2S_CHECK(throws_builder_error([&] { (void)b.type_pointer(spv::StorageClass::Private, 0); }));
    A2S_CHECK(throws_builder_error([&] { (void)b.constant(u32, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.decorate(kNoId, spv::Decoration::Block); }));
    A2S_CHECK(throws_builder_error([&] {
        (void)b.global_variable(b.type_pointer(spv::StorageClass::Function, u32),
                                spv::StorageClass::Function);
    }));
    A2S_CHECK(throws_builder_error([&] { b.add_extension(std::string_view("SPV_\0x", 6)); }));
    A2S_CHECK(throws_builder_error([&] {
        b.add_entry_point(spv::ExecutionModel::GLCompute, 1, std::string_view("ma\0in", 5), {});
    }));
    A2S_CHECK(throws_builder_error([&] { b.complete_forward_pointer(u32, u32); }));
    // An uncompleted forward pointer makes the module unfinishable.
    b.set_memory_model(spv::AddressingModel::PhysicalStorageBuffer64, spv::MemoryModel::GLSL450);
    (void)b.type_forward_pointer(spv::StorageClass::PhysicalStorageBuffer);
    A2S_CHECK(throws_builder_error([&] { (void)b.finish(); }));
}

A2S_TEST(oversized_instruction_throws) {
    ModuleBuilder b;
    const std::string huge(4 * 70000, 'x'); // > 65535 words as one literal string
    b.set_name(b.type_void(), "ok");
    A2S_CHECK(throws_builder_error([&] { b.set_name(b.type_void(), huge); }));
}

// ---- pipeline post-condition -------------------------------------------------------------------

A2S_TEST(check_generated_module_reports_internal_error) {
    air2spirv::Diagnostics ok_diag;
    A2S_CHECK_EQ(
        check_generated_module(build_compute_bda_module(), TargetEnv::Vulkan1_3, {}, ok_diag),
        A2S_OK);
    A2S_CHECK(!ok_diag.has_errors());

    // Hostile entry point name: control characters must not reach the rendered diagnostics.
    const Words broken =
        build_compute_bda_module(ComputeDefect::MissingLocalSize, true, "evil\x1b[2J\nname");
    air2spirv::Diagnostics diag;
    A2S_CHECK_EQ(check_generated_module(broken, TargetEnv::Vulkan1_3, {}, diag),
                 A2S_ERROR_INTERNAL);
    A2S_CHECK(diag.has_errors());
    const std::string text = diag.render();
    A2S_CHECK(text.find("internal error: generated SPIR-V fails spirv-val") != std::string::npos);
    A2S_CHECK(text.find("VUID-StandaloneSpirv-LocalSize-06426") != std::string::npos);
    A2S_CHECK(text.find('\x1b') == std::string::npos);
    std::size_t lines = 0;
    for (const char c : text) {
        lines += c == '\n' ? 1U : 0U;
    }
    A2S_CHECK_EQ(lines, diag.entries().size()); // one line per diagnostic
}

// ---- review follow-ups: ids, pointers, module-level instructions ------------------------------

A2S_TEST(completing_a_forward_pointer_after_the_pointer_type_exists_throws) {
    ModuleBuilder b;
    const Id u32 = b.type_int(32, false);
    const Id node_ptr = b.type_forward_pointer(spv::StorageClass::PhysicalStorageBuffer);
    const Id node = b.type_struct(std::array{node_ptr, u32});
    (void)b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, node); // second pointer type
    A2S_CHECK(throws_builder_error([&] { b.complete_forward_pointer(node_ptr, node); }));
}

A2S_TEST(pointer_array_stride_is_part_of_the_type_identity) {
    ModuleBuilder b;
    const Id u32 = b.type_int(32, false);
    const Id plain = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, u32);
    const Id strided = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, u32, 4);
    A2S_CHECK(plain != strided);
    A2S_CHECK_EQ(b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, u32, 4), strided);
    A2S_CHECK(b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, u32, 8) != strided);
    b.set_memory_model(spv::AddressingModel::PhysicalStorageBuffer64, spv::MemoryModel::GLSL450);
    const auto text = disassemble(b.finish());
    A2S_CHECK(text.ok);
    A2S_CHECK(text.text.find("ArrayStride 4") != std::string::npos);
    A2S_CHECK(text.text.find("ArrayStride 8") != std::string::npos);
}

A2S_TEST(function_and_label_ids_must_be_reserved_and_unused) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
    const Id not_reserved = b.type_int(32, false); // a type id, not from reserve_id()
    A2S_CHECK(throws_builder_error([&] {
        b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, not_reserved);
    }));
    const Id target = b.reserve_id();
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    b.emit(spv::Op::OpBranch, kNoId, {target});
    A2S_CHECK(throws_builder_error([&] { b.begin_block(not_reserved); }));
    A2S_CHECK(throws_builder_error([&] { b.begin_block(main); })); // already defined
    b.begin_block(target);
    b.emit(spv::Op::OpReturn, kNoId, {});
    A2S_CHECK(throws_builder_error([&] { b.begin_block(target); })); // defined twice
    b.end_function();
    A2S_CHECK(is_valid(b.finish()));
    ModuleBuilder other;
    A2S_CHECK(throws_builder_error([&] {
        other.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    }));
}

A2S_TEST(emit_rejects_module_level_instructions_in_a_body) {
    Id main = 0;
    Id void_type = 0;
    Id main_type = 0;
    ModuleBuilder b = minimal_compute_prefix(main, void_type, main_type);
    const Id u32 = b.type_int(32, false);
    const Id private_ptr = b.type_pointer(spv::StorageClass::Private, u32);
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    const auto storage = [](spv::StorageClass sc) { return static_cast<std::uint32_t>(sc); };
    A2S_CHECK(throws_builder_error(
        [&] { b.emit(spv::Op::OpVariable, private_ptr, {storage(spv::StorageClass::Private)}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpVariable, private_ptr, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpSpecConstant, u32, {1}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpSpecConstantTrue, u32, {}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpSource, kNoId, {2, 450}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpString, kNoId, {0}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(spv::Op::OpModuleProcessed, kNoId, {0}); }));
    A2S_CHECK(throws_builder_error([&] { b.emit(static_cast<spv::Op>(0x10000U), kNoId, {}); }));
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    A2S_CHECK(is_valid(b.finish()));
}

A2S_TEST(id_bound_limit_is_enforced) {
    BuilderOptions options;
    options.max_id_bound = 10; // ids 1..9
    ModuleBuilder b(options);
    for (std::uint32_t i = 1; i < 10; ++i) {
        A2S_CHECK_EQ(b.reserve_id(), i);
    }
    A2S_CHECK(throws_builder_error([&] { (void)b.reserve_id(); }));
    A2S_CHECK(throws_builder_error([&] { (void)b.type_void(); }));
    A2S_CHECK_EQ(b.id_bound(), 10U);
    A2S_CHECK_EQ(b.max_id_bound(), 10U);
    BuilderOptions too_large;
    too_large.max_id_bound = 0xFFFFFFFFU;
    A2S_CHECK_EQ(ModuleBuilder(too_large).max_id_bound(), kMaxIdBound); // universal limit
    A2S_CHECK_EQ(ModuleBuilder().max_id_bound(), kMaxIdBound);
}
