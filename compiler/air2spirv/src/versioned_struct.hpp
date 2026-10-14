// Helpers for the struct_size ABI-evolution convention of the public C header.
//
// Loading (caller -> library), same semantics as Linux' copy_struct_from_user():
//   - struct_size < minimum known size        -> A2S_ERROR_INVALID_ARGUMENT
//   - struct_size <= sizeof(T)                -> copy struct_size bytes, zero-fill the rest
//   - struct_size >  sizeof(T)                -> the unknown tail must be all zero bytes,
//                                                otherwise A2S_ERROR_INVALID_ARGUMENT
// Storing (library -> caller):
//   - struct_size < minimum known size        -> A2S_ERROR_INVALID_ARGUMENT
//   - copy sizeof(T) bytes, zero any larger caller tail, set struct_size = sizeof(T)
#pragma once

#include "air2spirv/air2spirv.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <type_traits>

namespace air2spirv {

// Size of the first published version of each struct. All structs are at their first version
// (API 0.1); when a field is appended, keep the old size here.
template <typename T>
inline constexpr std::size_t kMinStructSize = sizeof(T);

template <typename T>
[[nodiscard]] a2s_status load_versioned_struct(const T* src, T& dst) noexcept {
    static_assert(std::is_trivially_copyable_v<T> && std::is_standard_layout_v<T>);
    static_assert(offsetof(T, struct_size) == 0);
    if (src == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    std::uint32_t size = 0;
    std::memcpy(&size, src, sizeof(size));
    if (size < kMinStructSize<T>) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    const auto* bytes = reinterpret_cast<const unsigned char*>(src);
    for (std::size_t i = sizeof(T); i < size; ++i) {
        if (bytes[i] != 0) {
            return A2S_ERROR_INVALID_ARGUMENT;
        }
    }
    dst = T{};
    std::memcpy(&dst, src, size < sizeof(T) ? size : sizeof(T));
    dst.struct_size = static_cast<std::uint32_t>(sizeof(T));
    return A2S_OK;
}

template <typename T>
[[nodiscard]] a2s_status store_versioned_struct(const T& src, T* dst) noexcept {
    static_assert(std::is_trivially_copyable_v<T> && std::is_standard_layout_v<T>);
    static_assert(offsetof(T, struct_size) == 0);
    if (dst == nullptr) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    std::uint32_t size = 0;
    std::memcpy(&size, dst, sizeof(size));
    if (size < kMinStructSize<T>) {
        return A2S_ERROR_INVALID_ARGUMENT;
    }
    T copy = src;
    copy.struct_size = static_cast<std::uint32_t>(sizeof(T));
    std::memcpy(dst, &copy, sizeof(T));
    if (size > sizeof(T)) {
        std::memset(reinterpret_cast<unsigned char*>(dst) + sizeof(T), 0, size - sizeof(T));
    }
    return A2S_OK;
}

} // namespace air2spirv
