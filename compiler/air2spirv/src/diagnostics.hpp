// Diagnostics collected during one API call and rendered into the output object.
#pragma once

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace air2spirv {

enum class Severity : std::uint8_t { Error, Warning, Note };

struct Diagnostic {
    Severity severity;
    std::string message;
};

class Diagnostics {
public:
    void error(std::string message);
    void warning(std::string message);
    void note(std::string message);

    [[nodiscard]] bool has_errors() const noexcept { return has_errors_; }
    [[nodiscard]] std::span<const Diagnostic> entries() const noexcept { return entries_; }

    // One line per diagnostic: "<severity>: <message>\n".
    [[nodiscard]] std::string render() const;

private:
    void add(Severity severity, std::string message);

    std::vector<Diagnostic> entries_;
    bool has_errors_ = false;
};

// Makes untrusted text (guest-provided names, bytes) safe to embed in a diagnostic line:
// control characters, backslash and non-ASCII bytes are escaped as \xNN, and the result is
// truncated to `max_length` input bytes (with "..." appended).
[[nodiscard]] std::string escape_untrusted(std::string_view text, std::size_t max_length = 128);

// "de ad be ef" for the first `max_bytes` bytes of `data`.
[[nodiscard]] std::string hex_bytes(std::span<const std::byte> data, std::size_t max_bytes);

// Zero-padded 8-digit lowercase hex, without prefix.
[[nodiscard]] std::string hex_u32(std::uint32_t value);

} // namespace air2spirv
