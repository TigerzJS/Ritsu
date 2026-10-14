#include "diagnostics.hpp"

#include <array>
#include <utility>

namespace air2spirv {

void Diagnostics::error(std::string message) {
    add(Severity::Error, std::move(message));
}
void Diagnostics::warning(std::string message) {
    add(Severity::Warning, std::move(message));
}
void Diagnostics::note(std::string message) {
    add(Severity::Note, std::move(message));
}

void Diagnostics::add(Severity severity, std::string message) {
    if (severity == Severity::Error) {
        has_errors_ = true;
    }
    entries_.push_back(Diagnostic{severity, std::move(message)});
}

std::string Diagnostics::render() const {
    std::string text;
    for (const Diagnostic& d : entries_) {
        switch (d.severity) {
        case Severity::Error:
            text += "error: ";
            break;
        case Severity::Warning:
            text += "warning: ";
            break;
        case Severity::Note:
            text += "note: ";
            break;
        }
        text += d.message;
        text += '\n';
    }
    return text;
}

namespace {
constexpr std::array<char, 16> kHex = {'0', '1', '2', '3', '4', '5', '6', '7',
                                       '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'};
} // namespace

std::string escape_untrusted(std::string_view text, std::size_t max_length) {
    std::string out;
    const std::size_t n = text.size() < max_length ? text.size() : max_length;
    out.reserve(n);
    for (std::size_t i = 0; i < n; ++i) {
        const auto byte = static_cast<unsigned char>(text[i]);
        if (byte < 0x20 || byte >= 0x7F || byte == '\\') {
            out += "\\x";
            out += kHex[byte >> 4U];
            out += kHex[byte & 0x0FU];
        } else {
            out += static_cast<char>(byte);
        }
    }
    if (text.size() > max_length) {
        out += "...";
    }
    return out;
}

std::string hex_bytes(std::span<const std::byte> data, std::size_t max_bytes) {
    std::string out;
    const std::size_t n = data.size() < max_bytes ? data.size() : max_bytes;
    for (std::size_t i = 0; i < n; ++i) {
        const auto byte = std::to_integer<unsigned>(data[i]);
        if (i != 0) {
            out += ' ';
        }
        out += kHex[byte >> 4U];
        out += kHex[byte & 0x0FU];
    }
    return out;
}

std::string hex_u32(std::uint32_t value) {
    std::string out(8, '0');
    for (std::size_t i = 0; i < 8; ++i) {
        out[7 - i] = kHex[(value >> (4U * i)) & 0x0FU];
    }
    return out;
}

} // namespace air2spirv
