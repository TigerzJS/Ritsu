// Builders for synthetic test inputs. Only public LLVM formats; no Apple data (ADR-0008).
#pragma once

#include <cstddef>
#include <cstdint>
#include <span>
#include <vector>

namespace a2s_test {

// Raw bitcode stream: the magic 'B' 'C' 0xC0 0xDE followed by zero padding to `size` bytes.
inline std::vector<std::uint8_t> make_raw_bitcode(std::size_t size = 16) {
    std::vector<std::uint8_t> bytes(size < 4 ? 4 : size, 0);
    bytes[0] = 'B';
    bytes[1] = 'C';
    bytes[2] = 0xC0;
    bytes[3] = 0xDE;
    bytes.resize(size);
    return bytes;
}

inline void put_u32_le(std::vector<std::uint8_t>& bytes, std::size_t offset, std::uint32_t v) {
    if (bytes.size() < offset + 4) {
        bytes.resize(offset + 4, 0);
    }
    bytes[offset + 0] = static_cast<std::uint8_t>(v & 0xFFU);
    bytes[offset + 1] = static_cast<std::uint8_t>((v >> 8U) & 0xFFU);
    bytes[offset + 2] = static_cast<std::uint8_t>((v >> 16U) & 0xFFU);
    bytes[offset + 3] = static_cast<std::uint8_t>((v >> 24U) & 0xFFU);
}

struct WrapperFields {
    std::uint32_t magic = 0x0B17C0DEU;
    std::uint32_t version = 0;
    std::uint32_t offset = 20;
    std::uint32_t size = 16;
    std::uint32_t cpu_type = 0x01000007U; // arbitrary test value
};

// Header with the given fields, followed (at `payload_offset`) by `payload`.
inline std::vector<std::uint8_t> make_wrapper(const WrapperFields& f,
                                              std::span<const std::uint8_t> payload,
                                              std::size_t payload_offset = 20) {
    std::vector<std::uint8_t> bytes(payload_offset + payload.size(), 0);
    put_u32_le(bytes, 0, f.magic);
    put_u32_le(bytes, 4, f.version);
    put_u32_le(bytes, 8, f.offset);
    put_u32_le(bytes, 12, f.size);
    put_u32_le(bytes, 16, f.cpu_type);
    for (std::size_t i = 0; i < payload.size(); ++i) {
        bytes[payload_offset + i] = payload[i];
    }
    return bytes;
}

// Well-formed wrapper around a 16-byte raw bitcode stream.
inline std::vector<std::uint8_t> make_valid_wrapper() {
    const auto payload = make_raw_bitcode(16);
    return make_wrapper(WrapperFields{}, payload);
}

inline std::span<const std::byte> as_bytes(const std::vector<std::uint8_t>& v) {
    return std::as_bytes(std::span<const std::uint8_t>(v));
}

// SplitMix64: small deterministic PRNG for randomized tests.
struct SplitMix64 {
    std::uint64_t state;
    std::uint64_t next() {
        std::uint64_t z = (state += 0x9E3779B97F4A7C15ULL);
        z = (z ^ (z >> 30U)) * 0xBF58476D1CE4E5B9ULL;
        z = (z ^ (z >> 27U)) * 0x94D049BB133111EBULL;
        return z ^ (z >> 31U);
    }
};

} // namespace a2s_test
