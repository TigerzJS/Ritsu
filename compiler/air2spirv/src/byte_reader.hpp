// Bounds-checked, endian-independent reads from untrusted byte buffers.
#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <span>

namespace air2spirv {

// Reads a little-endian u32 at `offset`. Returns nullopt if [offset, offset + 4) is not fully
// inside `data` (overflow-safe).
[[nodiscard]] constexpr std::optional<std::uint32_t> read_u32_le(std::span<const std::byte> data,
                                                                 std::size_t offset) noexcept {
    if (offset > data.size() || data.size() - offset < sizeof(std::uint32_t)) {
        return std::nullopt;
    }
    return static_cast<std::uint32_t>(data[offset]) |
           (static_cast<std::uint32_t>(data[offset + 1]) << 8U) |
           (static_cast<std::uint32_t>(data[offset + 2]) << 16U) |
           (static_cast<std::uint32_t>(data[offset + 3]) << 24U);
}

} // namespace air2spirv
