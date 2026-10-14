#include "spirv/tools.hpp"

#include <spirv-tools/libspirv.hpp>
#include <spirv-tools/optimizer.hpp>
#include <string>
#include <utility>

namespace air2spirv::spirv {
namespace {

// SPIR-V header: magic, version, generator, bound, schema.
constexpr std::size_t kHeaderWords = 5;

spv_target_env to_spv_env(TargetEnv env) noexcept {
    switch (env) {
    case TargetEnv::Vulkan1_3:
        return SPV_ENV_VULKAN_1_3;
    case TargetEnv::Vulkan1_4:
        return SPV_ENV_VULKAN_1_4;
    }
    return SPV_ENV_VULKAN_1_3;
}

const char* level_name(spv_message_level_t level) noexcept {
    switch (level) {
    case SPV_MSG_FATAL:
        return "fatal";
    case SPV_MSG_INTERNAL_ERROR:
        return "internal error";
    case SPV_MSG_ERROR:
        return "error";
    case SPV_MSG_WARNING:
        return "warning";
    case SPV_MSG_INFO:
        return "info";
    case SPV_MSG_DEBUG:
        return "debug";
    }
    return "message";
}

// Collects SPIRV-Tools messages into `log`. Positions are word indices (binary input) or
// line:column (assembly input); both are deterministic.
spvtools::MessageConsumer collect_into(std::string& log) {
    return [&log](spv_message_level_t level, const char* /*source*/, const spv_position_t& position,
                  const char* message) {
        log += level_name(level);
        log += ": ";
        if (position.line != 0 || position.column != 0) {
            log += "[line " + std::to_string(position.line + 1) + ':' +
                   std::to_string(position.column + 1) + "] ";
        } else if (position.index != 0) {
            log += "[word " + std::to_string(position.index) + "] ";
        }
        log += message != nullptr ? message : "(no message)";
        log += '\n';
    };
}

spvtools::ValidatorOptions to_validator_options(const ValidatorSettings& settings) {
    spvtools::ValidatorOptions options;
    options.SetScalarBlockLayout(settings.scalar_block_layout);
    options.SetWorkgroupScalarBlockLayout(settings.workgroup_scalar_block_layout);
    return options;
}

bool has_header(std::span<const std::uint32_t> words, std::string& log) {
    if (words.size() < kHeaderWords) {
        log += "error: module has " + std::to_string(words.size()) +
               " words, fewer than the 5-word SPIR-V header\n";
        return false;
    }
    return true;
}

} // namespace

TargetEnv target_env_for_vulkan(std::uint32_t major, std::uint32_t minor) noexcept {
    if (major == 1 && minor <= 3) {
        return TargetEnv::Vulkan1_3;
    }
    return TargetEnv::Vulkan1_4;
}

const char* target_env_name(TargetEnv env) noexcept {
    switch (env) {
    case TargetEnv::Vulkan1_3:
        return "Vulkan 1.3 (SPIR-V 1.6)";
    case TargetEnv::Vulkan1_4:
        return "Vulkan 1.4 (SPIR-V 1.6)";
    }
    return "unknown";
}

ValidationResult validate(std::span<const std::uint32_t> words, TargetEnv env,
                          const ValidatorSettings& settings) {
    ValidationResult result;
    if (!has_header(words, result.log)) {
        return result;
    }
    spvtools::SpirvTools tools(to_spv_env(env));
    if (!tools.IsValid()) {
        result.log += "internal error: SPIRV-Tools context creation failed\n";
        return result;
    }
    tools.SetMessageConsumer(collect_into(result.log));
    const spvtools::ValidatorOptions options = to_validator_options(settings);
    result.valid = tools.Validate(words.data(), words.size(), options);
    if (!result.valid && result.log.empty()) {
        result.log = "error: spirv-val rejected the module without a message\n";
    }
    return result;
}

DisassemblyResult disassemble(std::span<const std::uint32_t> words,
                              const DisassemblyOptions& options) {
    DisassemblyResult result;
    if (!has_header(words, result.log)) {
        return result;
    }
    spvtools::SpirvTools tools(SPV_ENV_UNIVERSAL_1_6);
    tools.SetMessageConsumer(collect_into(result.log));
    std::uint32_t flags = SPV_BINARY_TO_TEXT_OPTION_NONE;
    if (!options.header) {
        flags |= SPV_BINARY_TO_TEXT_OPTION_NO_HEADER;
    }
    if (options.friendly_names) {
        flags |= SPV_BINARY_TO_TEXT_OPTION_FRIENDLY_NAMES;
    }
    if (options.indent) {
        flags |= SPV_BINARY_TO_TEXT_OPTION_INDENT;
    }
    result.ok = tools.Disassemble(words.data(), words.size(), &result.text, flags);
    if (!result.ok) {
        result.text.clear();
    }
    return result;
}

AssemblyResult assemble(std::string_view text, TargetEnv env, bool preserve_numeric_ids) {
    AssemblyResult result;
    spvtools::SpirvTools tools(to_spv_env(env));
    tools.SetMessageConsumer(collect_into(result.log));
    const std::uint32_t flags = preserve_numeric_ids
                                    ? std::uint32_t{SPV_TEXT_TO_BINARY_OPTION_PRESERVE_NUMERIC_IDS}
                                    : std::uint32_t{SPV_TEXT_TO_BINARY_OPTION_NONE};
    result.ok = tools.Assemble(text.data(), text.size(), &result.words, flags);
    if (!result.ok) {
        result.words.clear();
    }
    return result;
}

OptimizationResult optimize(std::span<const std::uint32_t> words, TargetEnv env,
                            std::span<const std::string> pass_flags,
                            const ValidatorSettings& settings) {
    OptimizationResult result;
    if (!has_header(words, result.log)) {
        return result;
    }
    spvtools::Optimizer optimizer(to_spv_env(env));
    optimizer.SetMessageConsumer(collect_into(result.log));
    const std::vector<std::string> flags(pass_flags.begin(), pass_flags.end());
    if (!optimizer.RegisterPassesFromFlags(flags)) {
        if (result.log.empty()) {
            result.log = "error: spirv-opt rejected the pass list\n";
        }
        return result;
    }
    spvtools::OptimizerOptions optimizer_options;
    optimizer_options.set_run_validator(true);
    optimizer_options.set_validator_options(to_validator_options(settings));
    std::vector<std::uint32_t> optimized;
    if (!optimizer.Run(words.data(), words.size(), &optimized, optimizer_options)) {
        if (result.log.empty()) {
            result.log = "error: spirv-opt failed without a message\n";
        }
        return result;
    }
    // Post-condition: optimizer output must be valid as well (spirv-opt only validates input).
    ValidationResult check = validate(optimized, env, settings);
    if (!check.valid) {
        result.log += "error: spirv-opt produced a module that fails validation:\n";
        result.log += check.log;
        return result;
    }
    result.ok = true;
    result.words = std::move(optimized);
    return result;
}

a2s_status check_generated_module(std::span<const std::uint32_t> words, TargetEnv env,
                                  const ValidatorSettings& settings, Diagnostics& diag) {
    const ValidationResult check = validate(words, env, settings);
    if (check.valid) {
        return A2S_OK;
    }
    diag.error(std::string("internal error: generated SPIR-V fails spirv-val for ") +
               target_env_name(env) + " (compiler bug)");
    // Validator messages quote instructions, including literal strings that may originate
    // from the guest (entry point names). One note per line (diagnostics are single-line),
    // every line escaped, the total bounded.
    constexpr std::size_t kMaxLines = 32;
    constexpr std::size_t kMaxLineLength = 512;
    std::string_view rest = check.log;
    std::size_t lines = 0;
    while (!rest.empty()) {
        const std::size_t end = rest.find('\n');
        const std::string_view line = rest.substr(0, end);
        rest = end == std::string_view::npos ? std::string_view{} : rest.substr(end + 1);
        if (++lines > kMaxLines) {
            diag.note("spirv-val: further messages omitted");
            break;
        }
        diag.note("spirv-val: " + escape_untrusted(line, kMaxLineLength));
    }
    return A2S_ERROR_INTERNAL;
}

const char* spirv_tools_version() noexcept {
    const char* version = spvSoftwareVersionString();
    return version != nullptr ? version : "unknown";
}

} // namespace air2spirv::spirv
