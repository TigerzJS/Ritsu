// Tests for the input sniffer: raw bitcode magic and LLVM bitcode wrapper header parsing.
#include "byte_reader.hpp"
#include "input_sniffer.hpp"
#include "test_harness.hpp"
#include "test_inputs.hpp"

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

using namespace air2spirv;
using a2s_test::as_bytes;

namespace {

a2s_status sniff(const std::vector<std::uint8_t>& bytes, SniffedInput& out) {
    Diagnostics diag;
    const a2s_status s = sniff_input(as_bytes(bytes), out, diag);
    // Every failure must come with at least one error diagnostic; success never has errors.
    if (s == A2S_OK) {
        A2S_CHECK(!diag.has_errors());
    } else {
        A2S_CHECK(diag.has_errors());
    }
    return s;
}

a2s_status sniff(const std::vector<std::uint8_t>& bytes) {
    SniffedInput out;
    return sniff(bytes, out);
}

bool is_inside(std::span<const std::byte> inner, std::span<const std::byte> outer) {
    return inner.data() >= outer.data() &&
           inner.data() + inner.size() <= outer.data() + outer.size();
}

} // namespace

A2S_TEST(read_u32_is_little_endian_and_bounds_checked) {
    const std::vector<std::uint8_t> bytes = {0x78, 0x56, 0x34, 0x12, 0xAA};
    A2S_CHECK_EQ(read_u32_le(as_bytes(bytes), 0).value_or(0), 0x12345678U);
    A2S_CHECK_EQ(read_u32_le(as_bytes(bytes), 1).value_or(0), 0xAA123456U);
    A2S_CHECK(!read_u32_le(as_bytes(bytes), 2).has_value());
    A2S_CHECK(!read_u32_le(as_bytes(bytes), 5).has_value());
    A2S_CHECK(!read_u32_le(as_bytes(bytes), static_cast<std::size_t>(-1)).has_value());
    A2S_CHECK(!read_u32_le({}, 0).has_value());
}

A2S_TEST(detects_raw_bitcode_magic) {
    A2S_CHECK(has_raw_bitcode_magic(as_bytes(a2s_test::make_raw_bitcode(4))));
    A2S_CHECK(has_raw_bitcode_magic(as_bytes(a2s_test::make_raw_bitcode(64))));
    A2S_CHECK(!has_raw_bitcode_magic(as_bytes(std::vector<std::uint8_t>{'B', 'C', 0xC0})));
    A2S_CHECK(!has_raw_bitcode_magic(as_bytes(std::vector<std::uint8_t>{'B', 'C', 0xC0, 0xDF})));
    A2S_CHECK(!has_raw_bitcode_magic(as_bytes(std::vector<std::uint8_t>{0xDE, 0xC0, 'C', 'B'})));
    A2S_CHECK(!has_raw_bitcode_magic({}));
}

A2S_TEST(detects_wrapper_magic) {
    A2S_CHECK(has_bitcode_wrapper_magic(as_bytes(a2s_test::make_valid_wrapper())));
    // Byte order matters: the magic is little-endian on disk.
    A2S_CHECK(
        !has_bitcode_wrapper_magic(as_bytes(std::vector<std::uint8_t>{0x0B, 0x17, 0xC0, 0xDE})));
    A2S_CHECK(
        has_bitcode_wrapper_magic(as_bytes(std::vector<std::uint8_t>{0xDE, 0xC0, 0x17, 0x0B})));
    A2S_CHECK(!has_bitcode_wrapper_magic(as_bytes(std::vector<std::uint8_t>{0xDE, 0xC0, 0x17})));
}

A2S_TEST(accepts_raw_bitcode) {
    const auto bytes = a2s_test::make_raw_bitcode(32);
    SniffedInput out;
    A2S_CHECK_EQ(sniff(bytes, out), A2S_OK);
    A2S_CHECK(out.format == InputFormat::RawBitcode);
    A2S_CHECK_EQ(out.bitcode.size(), bytes.size());
    A2S_CHECK(out.bitcode.data() == as_bytes(bytes).data());
    A2S_CHECK(!out.wrapper.has_value());
}

A2S_TEST(rejects_raw_bitcode_with_odd_length) {
    A2S_CHECK_EQ(sniff(a2s_test::make_raw_bitcode(5)), A2S_ERROR_MALFORMED_INPUT);
    A2S_CHECK_EQ(sniff(a2s_test::make_raw_bitcode(18)), A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(rejects_empty_and_tiny_inputs) {
    A2S_CHECK_EQ(sniff({}), A2S_ERROR_MALFORMED_INPUT);
    A2S_CHECK_EQ(sniff({'B'}), A2S_ERROR_MALFORMED_INPUT);
    A2S_CHECK_EQ(sniff({'B', 'C', 0xC0}), A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(reports_unknown_format_as_unsupported) {
    const std::vector<std::uint8_t> text = {'h', 'e', 'l', 'l', 'o', ' ', 'w', 'o'};
    A2S_CHECK_EQ(sniff(text), A2S_ERROR_UNSUPPORTED);
    const std::vector<std::uint8_t> elf = {0x7F, 'E', 'L', 'F', 2, 1, 1, 0};
    A2S_CHECK_EQ(sniff(elf), A2S_ERROR_UNSUPPORTED);
}

A2S_TEST(accepts_valid_wrapper) {
    const auto bytes = a2s_test::make_valid_wrapper();
    SniffedInput out;
    A2S_CHECK_EQ(sniff(bytes, out), A2S_OK);
    A2S_CHECK(out.format == InputFormat::WrappedBitcode);
    A2S_CHECK(out.wrapper.has_value());
    if (out.wrapper) {
        A2S_CHECK_EQ(out.wrapper->magic, 0x0B17C0DEU);
        A2S_CHECK_EQ(out.wrapper->version, 0U);
        A2S_CHECK_EQ(out.wrapper->offset, 20U);
        A2S_CHECK_EQ(out.wrapper->size, 16U);
        A2S_CHECK_EQ(out.wrapper->cpu_type, 0x01000007U);
    }
    A2S_CHECK_EQ(out.bitcode.size(), std::size_t{16});
    A2S_CHECK(out.bitcode.data() == as_bytes(bytes).data() + 20);
    A2S_CHECK(has_raw_bitcode_magic(out.bitcode));
}

A2S_TEST(accepts_wrapper_with_gap_and_trailing_bytes) {
    const auto payload = a2s_test::make_raw_bitcode(8);
    a2s_test::WrapperFields f;
    f.offset = 32;
    f.size = 8;
    auto bytes = a2s_test::make_wrapper(f, payload, 32);
    bytes.resize(bytes.size() + 12, 0xEE); // padding after the stream is allowed
    SniffedInput out;
    A2S_CHECK_EQ(sniff(bytes, out), A2S_OK);
    A2S_CHECK(out.bitcode.data() == as_bytes(bytes).data() + 32);
    A2S_CHECK_EQ(out.bitcode.size(), std::size_t{8});
}

A2S_TEST(rejects_truncated_header) {
    const auto valid = a2s_test::make_valid_wrapper();
    for (std::size_t len = 4; len < kBitcodeWrapperHeaderSize; ++len) {
        const std::vector<std::uint8_t> prefix(valid.begin(),
                                               valid.begin() + static_cast<std::ptrdiff_t>(len));
        A2S_CHECK_EQ(sniff(prefix), A2S_ERROR_MALFORMED_INPUT);
    }
}

A2S_TEST(rejects_every_truncation_of_a_valid_wrapper) {
    const auto valid = a2s_test::make_valid_wrapper();
    for (std::size_t len = 0; len < valid.size(); ++len) {
        const std::vector<std::uint8_t> prefix(valid.begin(),
                                               valid.begin() + static_cast<std::ptrdiff_t>(len));
        A2S_CHECK(sniff(prefix) != A2S_OK);
    }
    A2S_CHECK_EQ(sniff(valid), A2S_OK);
}

A2S_TEST(rejects_offset_inside_header) {
    const auto payload = a2s_test::make_raw_bitcode(16);
    for (std::uint32_t offset = 0; offset < 20; ++offset) {
        a2s_test::WrapperFields f;
        f.offset = offset;
        A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    }
}

A2S_TEST(rejects_offset_beyond_end) {
    const auto payload = a2s_test::make_raw_bitcode(16);
    a2s_test::WrapperFields f;
    f.offset = 37; // input is 36 bytes
    f.size = 4;
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    f.offset = 0xFFFFFFFFU;
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(rejects_size_overflow) {
    const auto payload = a2s_test::make_raw_bitcode(16);
    a2s_test::WrapperFields f;
    f.size = 0xFFFFFFFFU; // offset + size wraps in 32 bits
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    f.size = 0xFFFFFFECU; // 20 + size == 2^32 exactly
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    f.size = 17; // one byte past the end
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(rejects_empty_or_misaligned_stream) {
    const auto payload = a2s_test::make_raw_bitcode(16);
    a2s_test::WrapperFields f;
    f.size = 0;
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    f.size = 3;
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
    f.size = 6;
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(f, payload)), A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(rejects_wrapped_stream_without_bitcode_magic) {
    std::vector<std::uint8_t> payload(16, 0);
    A2S_CHECK_EQ(sniff(a2s_test::make_wrapper(a2s_test::WrapperFields{}, payload)),
                 A2S_ERROR_MALFORMED_INPUT);
}

A2S_TEST(warns_about_nonzero_wrapper_version) {
    const auto payload = a2s_test::make_raw_bitcode(16);
    a2s_test::WrapperFields f;
    f.version = 7;
    const auto bytes = a2s_test::make_wrapper(f, payload);
    Diagnostics diag;
    SniffedInput out;
    A2S_CHECK_EQ(sniff_input(as_bytes(bytes), out, diag), A2S_OK);
    bool warned = false;
    for (const Diagnostic& d : diag.entries()) {
        warned = warned || d.severity == Severity::Warning;
    }
    A2S_CHECK(warned);
    A2S_CHECK(out.wrapper.has_value() && out.wrapper->version == 7U);
}

A2S_TEST(parse_wrapper_leaves_outputs_empty_on_failure) {
    const std::vector<std::uint8_t> bytes = {0xDE, 0xC0, 0x17, 0x0B, 0, 0, 0, 0};
    BitcodeWrapperHeader header;
    header.size = 99;
    std::span<const std::byte> bitcode = as_bytes(bytes);
    Diagnostics diag;
    A2S_CHECK_EQ(parse_bitcode_wrapper(as_bytes(bytes), header, bitcode, diag),
                 A2S_ERROR_MALFORMED_INPUT);
    A2S_CHECK_EQ(header.size, 0U);
    A2S_CHECK(bitcode.empty());
}

// Deterministic mini-fuzz (the libFuzzer target fuzz_air_sniff goes much further): random
// wrapper headers must never make the parser report a stream outside the input.
A2S_TEST(random_wrappers_respect_invariants) {
    a2s_test::SplitMix64 rng{a2s_test::case_seed("random_wrappers_respect_invariants")};
    for (int iteration = 0; iteration < 20000; ++iteration) {
        const std::size_t length = static_cast<std::size_t>(rng.next() % 96);
        std::vector<std::uint8_t> bytes(length);
        for (auto& b : bytes) {
            b = static_cast<std::uint8_t>(rng.next());
        }
        if (length >= 4) {
            a2s_test::put_u32_le(bytes, 0, 0x0B17C0DEU);
        }
        // Bias offset/size towards interesting small values.
        if (length >= 16 && (rng.next() & 1U) != 0) {
            a2s_test::put_u32_le(bytes, 8, static_cast<std::uint32_t>(rng.next() % 64));
            a2s_test::put_u32_le(bytes, 12, static_cast<std::uint32_t>(rng.next() % 64));
        }
        const std::uint32_t off = static_cast<std::uint32_t>(rng.next() % 64);
        if (off + 4 <= length && (rng.next() & 1U) != 0) {
            bytes[off] = 'B';
            bytes[off + 1] = 'C';
            bytes[off + 2] = 0xC0;
            bytes[off + 3] = 0xDE;
        }
        SniffedInput out;
        Diagnostics diag;
        const a2s_status s = sniff_input(as_bytes(bytes), out, diag);
        A2S_CHECK(s == A2S_OK || s == A2S_ERROR_MALFORMED_INPUT || s == A2S_ERROR_UNSUPPORTED);
        if (s == A2S_OK) {
            A2S_CHECK(is_inside(out.bitcode, as_bytes(bytes)));
            A2S_CHECK(!out.bitcode.empty() && out.bitcode.size() % 4 == 0);
            A2S_CHECK(has_raw_bitcode_magic(out.bitcode));
        } else {
            A2S_CHECK(diag.has_errors());
        }
    }
}

A2S_TEST(escape_untrusted_neutralizes_control_bytes) {
    A2S_CHECK_EQ(escape_untrusted("main0"), std::string("main0"));
    A2S_CHECK_EQ(escape_untrusted(std::string_view("a\nb\x1b\\", 5)),
                 std::string("a\\x0ab\\x1b\\x5c"));
    A2S_CHECK_EQ(escape_untrusted("\xC3\xA4"), std::string("\\xc3\\xa4"));
    A2S_CHECK_EQ(escape_untrusted("abcdef", 3), std::string("abc..."));
}
