#include "input_sniffer.hpp"

#include "byte_reader.hpp"

#include <algorithm>
#include <string>

namespace air2spirv {
namespace {

// Common checks for a located bitcode stream.
a2s_status check_bitcode_stream(std::span<const std::byte> bitcode, Diagnostics& diag) {
    if (!has_raw_bitcode_magic(bitcode)) {
        diag.error("bitcode stream does not start with the LLVM bitcode magic 'BC' 0xC0DE");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    // The LLVM bitstream is a sequence of 32-bit words.
    if (bitcode.size() % 4 != 0) {
        diag.error("bitcode stream size " + std::to_string(bitcode.size()) +
                   " is not a multiple of 4");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    return A2S_OK;
}

} // namespace

bool has_raw_bitcode_magic(std::span<const std::byte> data) noexcept {
    return data.size() >= kRawBitcodeMagic.size() &&
           std::equal(kRawBitcodeMagic.begin(), kRawBitcodeMagic.end(), data.begin());
}

bool has_bitcode_wrapper_magic(std::span<const std::byte> data) noexcept {
    const auto magic = read_u32_le(data, 0);
    return magic.has_value() && *magic == kBitcodeWrapperMagic;
}

a2s_status parse_bitcode_wrapper(std::span<const std::byte> data, BitcodeWrapperHeader& header,
                                 std::span<const std::byte>& bitcode, Diagnostics& diag) {
    header = BitcodeWrapperHeader{};
    bitcode = {};
    if (data.size() < kBitcodeWrapperHeaderSize) {
        diag.error("bitcode wrapper header truncated: " + std::to_string(data.size()) +
                   " bytes, need " + std::to_string(kBitcodeWrapperHeaderSize));
        return A2S_ERROR_MALFORMED_INPUT;
    }
    // The size check above makes every read below in bounds; value_or keeps it total anyway.
    BitcodeWrapperHeader h;
    h.magic = read_u32_le(data, 0).value_or(0);
    h.version = read_u32_le(data, 4).value_or(0);
    h.offset = read_u32_le(data, 8).value_or(0);
    h.size = read_u32_le(data, 12).value_or(0);
    h.cpu_type = read_u32_le(data, 16).value_or(0);

    if (h.magic != kBitcodeWrapperMagic) {
        diag.error("bitcode wrapper magic mismatch");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    if (h.offset < kBitcodeWrapperHeaderSize) {
        diag.error("bitcode wrapper offset " + std::to_string(h.offset) +
                   " overlaps the wrapper header");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    if (h.offset > data.size()) {
        diag.error("bitcode wrapper offset " + std::to_string(h.offset) +
                   " is beyond the end of the input (" + std::to_string(data.size()) + " bytes)");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    if (h.size == 0) {
        diag.error("bitcode wrapper declares an empty bitcode stream");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    // Overflow-safe form of offset + size > data.size().
    if (h.size > data.size() - h.offset) {
        diag.error("bitcode wrapper range [" + std::to_string(h.offset) + ", +" +
                   std::to_string(h.size) + ") exceeds the input (" + std::to_string(data.size()) +
                   " bytes)");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    const std::span<const std::byte> stream = data.subspan(h.offset, h.size);
    if (const a2s_status s = check_bitcode_stream(stream, diag); s != A2S_OK) {
        return s;
    }
    if (h.version != 0) {
        // LLVM documents version 0 only. Producers outside upstream LLVM may differ; report it
        // instead of silently accepting or rejecting (ADR-0007).
        diag.warning("bitcode wrapper version " + std::to_string(h.version) +
                     " is not 0 (the only publicly documented value)");
    }
    header = h;
    bitcode = stream;
    return A2S_OK;
}

a2s_status sniff_input(std::span<const std::byte> data, SniffedInput& out, Diagnostics& diag) {
    out = SniffedInput{};
    if (data.empty()) {
        diag.error("input is empty");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    if (data.size() < kRawBitcodeMagic.size()) {
        diag.error("input is too small (" + std::to_string(data.size()) +
                   " bytes) to contain any supported format");
        return A2S_ERROR_MALFORMED_INPUT;
    }
    if (has_raw_bitcode_magic(data)) {
        if (const a2s_status s = check_bitcode_stream(data, diag); s != A2S_OK) {
            return s;
        }
        out.format = InputFormat::RawBitcode;
        out.bitcode = data;
        return A2S_OK;
    }
    if (has_bitcode_wrapper_magic(data)) {
        BitcodeWrapperHeader header;
        std::span<const std::byte> bitcode;
        if (const a2s_status s = parse_bitcode_wrapper(data, header, bitcode, diag); s != A2S_OK) {
            return s;
        }
        out.format = InputFormat::WrappedBitcode;
        out.bitcode = bitcode;
        out.wrapper = header;
        return A2S_OK;
    }
    diag.error("unrecognized input format (first bytes: " + hex_bytes(data, 8) +
               "); accepted are raw LLVM bitcode and the LLVM bitcode wrapper");
    diag.note(".metallib containers are not supported yet: the container layout needs a "
              "clean-room specification (docs/specs/metallib/, ADR-0008)");
    return A2S_ERROR_UNSUPPORTED;
}

std::string describe(const SniffedInput& input) {
    std::string text;
    switch (input.format) {
    case InputFormat::RawBitcode:
        text = "input is raw LLVM bitcode";
        break;
    case InputFormat::WrappedBitcode:
        text = "input is LLVM bitcode in a bitcode wrapper";
        break;
    }
    if (input.wrapper.has_value()) {
        const BitcodeWrapperHeader& h = *input.wrapper;
        text += " (wrapper version " + std::to_string(h.version) + ", offset " +
                std::to_string(h.offset) + ", size " + std::to_string(h.size) + ", cpu type 0x" +
                hex_u32(h.cpu_type) + ")";
    }
    text += ", " + std::to_string(input.bitcode.size()) + " bytes of bitcode";
    return text;
}

} // namespace air2spirv
