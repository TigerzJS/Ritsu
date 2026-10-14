// Recognition of the outer input format.
//
// Only publicly documented LLVM formats are recognized here:
//   - raw LLVM bitcode: magic bytes 'B' 'C' 0xC0 0xDE
//   - the LLVM bitcode wrapper header (llvm/Bitcode/BitcodeReader.h, "Bitcode Wrapper Format"
//     in the LLVM docs): five little-endian u32 fields
//       [0] magic 0x0B17C0DE, [1] version, [2] offset of the bitcode, [3] size of the bitcode,
//       [4] CPU type
//
// The .metallib container is NOT recognized: its layout is Apple-internal and needs a clean-room
// specification (docs/specs/metallib/, ADR-0008) before any code may depend on it.
#pragma once

#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>
#include <string>

namespace air2spirv {

inline constexpr std::array<std::byte, 4> kRawBitcodeMagic = {std::byte{'B'}, std::byte{'C'},
                                                              std::byte{0xC0}, std::byte{0xDE}};
inline constexpr std::uint32_t kBitcodeWrapperMagic = 0x0B17C0DEU;
inline constexpr std::size_t kBitcodeWrapperHeaderSize = 5 * sizeof(std::uint32_t);

struct BitcodeWrapperHeader {
    std::uint32_t magic = 0;
    std::uint32_t version = 0;
    std::uint32_t offset = 0;
    std::uint32_t size = 0;
    std::uint32_t cpu_type = 0;
};

enum class InputFormat : std::uint8_t { RawBitcode, WrappedBitcode };

struct SniffedInput {
    InputFormat format = InputFormat::RawBitcode;
    // The bitcode stream proper (starts with kRawBitcodeMagic); a subspan of the input.
    std::span<const std::byte> bitcode;
    std::optional<BitcodeWrapperHeader> wrapper;
};

[[nodiscard]] bool has_raw_bitcode_magic(std::span<const std::byte> data) noexcept;
[[nodiscard]] bool has_bitcode_wrapper_magic(std::span<const std::byte> data) noexcept;

// Parses and validates a bitcode wrapper. On success `header` and `bitcode` are set and
// `bitcode` is guaranteed to lie inside `data`, to be non-empty, a multiple of 4 bytes long and
// to start with kRawBitcodeMagic. Never reads outside `data`.
[[nodiscard]] a2s_status parse_bitcode_wrapper(std::span<const std::byte> data,
                                               BitcodeWrapperHeader& header,
                                               std::span<const std::byte>& bitcode,
                                               Diagnostics& diag);

// Classifies the input and locates the bitcode stream.
//   A2S_OK                     - raw or wrapped bitcode, `out` filled
//   A2S_ERROR_MALFORMED_INPUT  - recognized format but broken, or too small to classify
//   A2S_ERROR_UNSUPPORTED      - unknown format (including .metallib, pending spec)
[[nodiscard]] a2s_status sniff_input(std::span<const std::byte> data, SniffedInput& out,
                                     Diagnostics& diag);

[[nodiscard]] std::string describe(const SniffedInput& input);

} // namespace air2spirv
