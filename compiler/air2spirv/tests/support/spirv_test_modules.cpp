#include "spirv_test_modules.hpp"

#include <array>
#include <stdexcept>
#include <string>

namespace a2s_test {

using air2spirv::spirv::BuilderOptions;
using air2spirv::spirv::Id;
using air2spirv::spirv::kNoId;
using air2spirv::spirv::ModuleBuilder;

namespace {

template <typename Enum>
constexpr std::uint32_t word(Enum value) noexcept {
    return static_cast<std::uint32_t>(value);
}

} // namespace

const char* defect_name(ComputeDefect defect) noexcept {
    switch (defect) {
    case ComputeDefect::None:
        return "None";
    case ComputeDefect::MissingLocalSize:
        return "MissingLocalSize";
    case ComputeDefect::MissingAlignedOnPhysicalStorageStore:
        return "MissingAlignedOnPhysicalStorageStore";
    case ComputeDefect::MissingPhysicalStorageBufferCapability:
        return "MissingPhysicalStorageBufferCapability";
    case ComputeDefect::MissingBlockOnPushConstant:
        return "MissingBlockOnPushConstant";
    }
    return "?";
}

std::vector<std::uint32_t> build_compute_bda_module(ComputeDefect defect, bool debug_names,
                                                    std::string_view entry_name) {
    ModuleBuilder b(BuilderOptions{debug_names});
    b.add_capability(spv::Capability::Shader);
    if (defect != ComputeDefect::MissingPhysicalStorageBufferCapability) {
        b.add_capability(spv::Capability::PhysicalStorageBufferAddresses);
    }
    b.set_memory_model(spv::AddressingModel::PhysicalStorageBuffer64, spv::MemoryModel::GLSL450);
    const Id main = b.reserve_id();

    const Id void_type = b.type_void();
    const Id main_type = b.type_function(void_type, {});
    const Id uint_type = b.type_int(32, false);
    const Id uvec3_type = b.type_vector(uint_type, 3);
    const Id input_uvec3_ptr = b.type_pointer(spv::StorageClass::Input, uvec3_type);
    const Id global_id = b.global_variable(input_uvec3_ptr, spv::StorageClass::Input);
    b.decorate(global_id, spv::Decoration::BuiltIn, {word(spv::BuiltIn::GlobalInvocationId)});

    const Id data_type = b.type_runtime_array(uint_type, 4);
    const Id buffer_type = b.type_struct(std::array{data_type});
    b.decorate(buffer_type, spv::Decoration::Block);
    b.decorate_member(buffer_type, 0, spv::Decoration::Offset, {0});
    const Id buffer_ptr = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, buffer_type);

    const Id push_type = b.type_struct(std::array{buffer_ptr});
    if (defect != ComputeDefect::MissingBlockOnPushConstant) {
        b.decorate(push_type, spv::Decoration::Block);
    }
    b.decorate_member(push_type, 0, spv::Decoration::Offset, {0});
    const Id push_ptr = b.type_pointer(spv::StorageClass::PushConstant, push_type);
    const Id push_constants = b.global_variable(push_ptr, spv::StorageClass::PushConstant);
    const Id push_member_ptr = b.type_pointer(spv::StorageClass::PushConstant, buffer_ptr);
    const Id element_ptr = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, uint_type);
    const Id input_uint_ptr = b.type_pointer(spv::StorageClass::Input, uint_type);
    const Id zero = b.constant_u32(0);

    b.set_name(main, entry_name);
    b.set_name(global_id, "gl_GlobalInvocationID");
    b.set_name(buffer_type, "Buffer");
    b.set_member_name(buffer_type, 0, "data");
    b.set_name(push_type, "PushConstants");
    b.set_member_name(push_type, 0, "buffer");
    b.set_name(push_constants, "push_constants");

    // SPIR-V 1.4+: the interface lists every global variable the entry point uses.
    b.add_entry_point(spv::ExecutionModel::GLCompute, main, entry_name,
                      std::array{global_id, push_constants});
    if (defect != ComputeDefect::MissingLocalSize) {
        b.add_execution_mode(main, spv::ExecutionMode::LocalSize, {64, 1, 1});
    }

    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    const Id index_ptr = b.emit(spv::Op::OpAccessChain, input_uint_ptr, {global_id, zero});
    const Id index = b.emit(spv::Op::OpLoad, uint_type, {index_ptr});
    const Id buffer_slot = b.emit(spv::Op::OpAccessChain, push_member_ptr, {push_constants, zero});
    const Id buffer = b.emit(spv::Op::OpLoad, buffer_ptr, {buffer_slot});
    const Id element = b.emit(spv::Op::OpAccessChain, element_ptr, {buffer, zero, index});
    if (defect == ComputeDefect::MissingAlignedOnPhysicalStorageStore) {
        b.emit(spv::Op::OpStore, kNoId, {element, index});
    } else {
        b.emit(spv::Op::OpStore, kNoId, {element, index, word(spv::MemoryAccessMask::Aligned), 4});
    }
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    return b.finish();
}

std::vector<std::uint32_t> build_forward_pointer_module() {
    ModuleBuilder b;
    b.add_capability(spv::Capability::Shader);
    b.add_capability(spv::Capability::PhysicalStorageBufferAddresses);
    b.set_memory_model(spv::AddressingModel::PhysicalStorageBuffer64, spv::MemoryModel::GLSL450);
    const Id main = b.reserve_id();

    const Id void_type = b.type_void();
    const Id main_type = b.type_function(void_type, {});
    const Id uint_type = b.type_int(32, false);
    const Id node_ptr = b.type_forward_pointer(spv::StorageClass::PhysicalStorageBuffer);
    const Id node_type = b.type_struct(std::array{node_ptr, uint_type});
    b.decorate(node_type, spv::Decoration::Block);
    b.decorate_member(node_type, 0, spv::Decoration::Offset, {0});
    b.decorate_member(node_type, 1, spv::Decoration::Offset, {8});
    b.complete_forward_pointer(node_ptr, node_type);
    // The completed forward pointer is the canonical pointer type from now on.
    if (b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, node_type) != node_ptr) {
        throw std::logic_error("forward pointer was not reused as the canonical pointer type");
    }

    const Id push_type = b.type_struct(std::array{node_ptr});
    b.decorate(push_type, spv::Decoration::Block);
    b.decorate_member(push_type, 0, spv::Decoration::Offset, {0});
    const Id push_ptr = b.type_pointer(spv::StorageClass::PushConstant, push_type);
    const Id push_constants = b.global_variable(push_ptr, spv::StorageClass::PushConstant);
    const Id push_member_ptr = b.type_pointer(spv::StorageClass::PushConstant, node_ptr);
    const Id next_slot_ptr = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, node_ptr);
    const Id value_ptr = b.type_pointer(spv::StorageClass::PhysicalStorageBuffer, uint_type);
    const Id zero = b.constant_u32(0);
    const Id one = b.constant_u32(1);

    b.set_name(node_type, "Node");
    b.add_entry_point(spv::ExecutionModel::GLCompute, main, "main", std::array{push_constants});
    b.add_execution_mode(main, spv::ExecutionMode::LocalSize, {1, 1, 1});

    const auto aligned = word(spv::MemoryAccessMask::Aligned);
    b.begin_function(void_type, spv::FunctionControlMask::MaskNone, main_type, main);
    b.begin_block();
    const Id head_slot = b.emit(spv::Op::OpAccessChain, push_member_ptr, {push_constants, zero});
    const Id head = b.emit(spv::Op::OpLoad, node_ptr, {head_slot});
    const Id head_value_ptr = b.emit(spv::Op::OpAccessChain, value_ptr, {head, one});
    const Id head_value = b.emit(spv::Op::OpLoad, uint_type, {head_value_ptr, aligned, 4});
    const Id next_slot = b.emit(spv::Op::OpAccessChain, next_slot_ptr, {head, zero});
    const Id next = b.emit(spv::Op::OpLoad, node_ptr, {next_slot, aligned, 8});
    const Id next_value_ptr = b.emit(spv::Op::OpAccessChain, value_ptr, {next, one});
    b.emit(spv::Op::OpStore, kNoId, {next_value_ptr, head_value, aligned, 4});
    b.emit(spv::Op::OpReturn, kNoId, {});
    b.end_function();
    return b.finish();
}

FileCheckResult check_spirv(std::span<const std::uint32_t> words, std::string_view checks,
                            air2spirv::spirv::TargetEnv env) {
    const auto validation = air2spirv::spirv::validate(words, env);
    if (!validation.valid) {
        return FileCheckResult{false, "spirv-val failed:\n" + validation.log};
    }
    const auto disassembly = air2spirv::spirv::disassemble(words);
    if (!disassembly.ok) {
        return FileCheckResult{false, "spirv-dis failed:\n" + disassembly.log};
    }
    FileCheckResult result = filecheck(checks, disassembly.text);
    if (!result.ok) {
        result.message += "\n--- disassembly ---\n" + disassembly.text;
    }
    return result;
}

} // namespace a2s_test
