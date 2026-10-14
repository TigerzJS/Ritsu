// Stage 3 of the pipeline (docs/subsystems/shader-compiler.md §2.3): LLVM bitcode -> verified
// llvm::Module. Only available when air2spirv is built with AIR2SPIRV_WITH_LLVM=ON.
//
// This header is LLVM-free on purpose: LLVM types never leave src/bitcode/, src/air/ and src/ir/
// (§3). Code in those directories reaches the llvm::Module through bitcode_module_impl.hpp.
#pragma once

#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <span>
#include <string>

namespace air2spirv::bitcode {

// What the bitstream pre-scan (prescan.hpp) found in the identification fields.
struct BitcodeIdentity {
    std::optional<std::string> producer;         // IDENTIFICATION_BLOCK / STRING (untrusted text)
    std::optional<std::uint64_t> epoch;          // IDENTIFICATION_BLOCK / EPOCH
    std::optional<std::uint64_t> module_version; // MODULE_BLOCK / VERSION of the first module
};

struct ModuleStats {
    std::size_t function_count = 0;         // definitions + declarations
    std::size_t defined_function_count = 0; // functions with a body
    std::size_t global_variable_count = 0;
    std::size_t instruction_count = 0;
    bool debug_info_dropped = false; // invalid or unsupported debug info was stripped
};

// A parsed and verified module together with the LLVMContext that owns it. Move-only.
class ParsedModule {
public:
    struct Impl; // defined in bitcode_module_impl.hpp (includes LLVM headers)

    ParsedModule();
    ~ParsedModule();
    ParsedModule(ParsedModule&&) noexcept;
    ParsedModule& operator=(ParsedModule&&) noexcept;
    ParsedModule(const ParsedModule&) = delete;
    ParsedModule& operator=(const ParsedModule&) = delete;

    [[nodiscard]] bool has_module() const noexcept;
    [[nodiscard]] const ModuleStats& stats() const noexcept { return stats_; }
    [[nodiscard]] const BitcodeIdentity& identity() const noexcept { return identity_; }

    // Only for src/bitcode, src/air, src/ir. Null until parse_bitcode_module() succeeded.
    [[nodiscard]] Impl* impl() noexcept { return impl_.get(); }

private:
    friend a2s_status parse_bitcode_module(std::span<const std::byte>, ParsedModule&, Diagnostics&);
    std::unique_ptr<Impl> impl_;
    ModuleStats stats_;
    BitcodeIdentity identity_;
};

// Parses one bitcode stream (starting with 'B' 'C' 0xC0 0xDE, wrapper already removed by the
// sniffer) into `out` and runs llvm::verifyModule on it. Never terminates the process for
// anything LLVM reports as an error; see docs/subsystems/shader-compiler.md §2.3 and §9 for
// what cannot be intercepted.
//   A2S_OK                     - `out` holds a verified module
//   A2S_ERROR_MALFORMED_INPUT  - corrupt bitstream, invalid records, verifier failure
//   A2S_ERROR_UNSUPPORTED      - valid-looking bitcode that uses features newer than the linked
//                                LLVM (epoch, module version, attribute kinds), several modules
//   A2S_ERROR_INTERNAL         - the LLVM runtime could not be prepared (llvm_runtime.hpp)
[[nodiscard]] a2s_status parse_bitcode_module(std::span<const std::byte> bitcode, ParsedModule& out,
                                              Diagnostics& diag);

// "parsed 3 functions (2 defined, 1 declared), 1 global variable, 42 instructions"
[[nodiscard]] std::string describe(const ModuleStats& stats);

// Version of the linked LLVM, e.g. "18.1.3".
[[nodiscard]] const char* llvm_version() noexcept;

} // namespace air2spirv::bitcode
