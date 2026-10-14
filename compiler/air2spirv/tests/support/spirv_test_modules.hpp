// Hand-built SPIR-V modules and check helpers for the SPIR-V infrastructure tests (WP-SC-03).
#pragma once

#include "filecheck.hpp"
#include "spirv/module_builder.hpp"
#include "spirv/tools.hpp"

#include <cstdint>
#include <span>
#include <string_view>
#include <vector>

namespace a2s_test {

// One deliberate mistake per value; each must be rejected by spirv-val for Vulkan 1.3.
enum class ComputeDefect {
    None,
    MissingLocalSize,                       // GLCompute without LocalSize/LocalSizeId/WorkgroupSize
    MissingAlignedOnPhysicalStorageStore,   // PhysicalStorageBuffer access without Aligned
    MissingPhysicalStorageBufferCapability, // PhysicalStorageBuffer64 without its capability
    MissingBlockOnPushConstant,             // push constant struct without Block
};

inline constexpr ComputeDefect kAllComputeDefects[] = {
    ComputeDefect::MissingLocalSize,
    ComputeDefect::MissingAlignedOnPhysicalStorageStore,
    ComputeDefect::MissingPhysicalStorageBufferCapability,
    ComputeDefect::MissingBlockOnPushConstant,
};

[[nodiscard]] const char* defect_name(ComputeDefect defect) noexcept;

// The WP-SC-03 acceptance module, written with ModuleBuilder:
//
//   layout(local_size_x = 64) in;
//   layout(buffer_reference) buffer Buffer { uint data[]; };   // PhysicalStorageBuffer (BDA)
//   layout(push_constant) uniform PushConstants { Buffer buffer; } push_constants;
//   void main() { uint i = gl_GlobalInvocationID.x; push_constants.buffer.data[i] = i; }
//
// `entry_name` is the OpEntryPoint name (tests use it to inject hostile text).
[[nodiscard]] std::vector<std::uint32_t>
build_compute_bda_module(ComputeDefect defect = ComputeDefect::None, bool debug_names = true,
                         std::string_view entry_name = "main");

// A linked list over PhysicalStorageBuffer pointers (OpTypeForwardPointer):
//   struct Node { Node* next; uint value; };  main(): node->next->value = node->value;
[[nodiscard]] std::vector<std::uint32_t> build_forward_pointer_module();

// spirv-val (must pass) -> spirv-dis (friendly names) -> filecheck(checks). On failure the
// message contains the validator log or the full disassembly.
[[nodiscard]] FileCheckResult
check_spirv(std::span<const std::uint32_t> words, std::string_view checks,
            air2spirv::spirv::TargetEnv env = air2spirv::spirv::TargetEnv::Vulkan1_3);

} // namespace a2s_test
