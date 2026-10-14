// In-process SPIRV-Tools: validation (spirv-val), disassembly (spirv-dis), assembly (spirv-as,
// for tests) and optimization (spirv-opt). docs/subsystems/shader-compiler.md §2.6, WP-SC-03.
//
// All functions are reentrant: each call creates its own SPIRV-Tools context, so concurrent
// compilations do not share state. Messages from SPIRV-Tools are collected into `log`, one
// line per message ("error: ...", "warning: ..."), in the order SPIRV-Tools reports them.
#pragma once

#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"

#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace air2spirv::spirv {

// Vulkan client environments air2spirv emits for. Both imply SPIR-V 1.6 (ADR-0007).
enum class TargetEnv : std::uint8_t {
    Vulkan1_3,
    Vulkan1_4,
};

// Maps a normalized target version (options.cpp guarantees major == 1, minor >= 3) to the
// environment to validate against. Newer 1.x minors than SPIRV-Tools knows are validated with
// the newest known environment.
[[nodiscard]] TargetEnv target_env_for_vulkan(std::uint32_t major, std::uint32_t minor) noexcept;
[[nodiscard]] const char* target_env_name(TargetEnv env) noexcept;

// Validator relaxations that correspond to Vulkan features the host enables. Default = none.
struct ValidatorSettings {
    bool scalar_block_layout = false;           // VK_EXT_scalar_block_layout / Vulkan 1.2 feature
    bool workgroup_scalar_block_layout = false; // workgroupMemoryExplicitLayoutScalarBlockLayout
};

struct ValidationResult {
    bool valid = false;
    std::string log;
};

// spirv-val. A module is valid only if SPIRV-Tools reports no error for `env`.
[[nodiscard]] ValidationResult validate(std::span<const std::uint32_t> words, TargetEnv env,
                                        const ValidatorSettings& settings = {});

struct DisassemblyOptions {
    bool friendly_names = true; // %uint, %main instead of %5 (spirv-dis default)
    bool header = false;        // "; SPIR-V\n; Version: 1.6\n..." comment block
    bool indent = false;        // align "%id = Op..." columns
};

struct DisassemblyResult {
    bool ok = false;
    std::string text;
    std::string log;
};

// spirv-dis. Output is deterministic for a given SPIRV-Tools version and options.
[[nodiscard]] DisassemblyResult disassemble(std::span<const std::uint32_t> words,
                                            const DisassemblyOptions& options = {});

struct AssemblyResult {
    bool ok = false;
    std::vector<std::uint32_t> words;
    std::string log;
};

// spirv-as (used by tests to build deliberately broken modules). The header version is the
// one of `env` (SPIR-V 1.6). `preserve_numeric_ids` keeps "%5" as id 5.
[[nodiscard]] AssemblyResult assemble(std::string_view text, TargetEnv env,
                                      bool preserve_numeric_ids = false);

struct OptimizationResult {
    bool ok = false;
    std::vector<std::uint32_t> words;
    std::string log;
};

// spirv-opt with an explicit pass list in spirv-opt flag syntax
// ("--eliminate-dead-code-aggressive",
// "-O", ...). The input is validated first; the output is validated again, and an invalid
// result is reported as failure. The pass list must be fixed per compiler version (§2.6: the
// cache key depends on it).
[[nodiscard]] OptimizationResult optimize(std::span<const std::uint32_t> words, TargetEnv env,
                                          std::span<const std::string> pass_flags,
                                          const ValidatorSettings& settings = {});

// Pipeline post-condition (ADR-0007, §2.6): validates a module air2spirv generated. A failure
// is a compiler bug: adds an error diagnostic with the validator log and returns
// A2S_ERROR_INTERNAL. Returns A2S_OK otherwise.
[[nodiscard]] a2s_status check_generated_module(std::span<const std::uint32_t> words, TargetEnv env,
                                                const ValidatorSettings& settings,
                                                Diagnostics& diag);

// "v2025.1" etc.; part of the library version string (cache key).
[[nodiscard]] const char* spirv_tools_version() noexcept;

} // namespace air2spirv::spirv
