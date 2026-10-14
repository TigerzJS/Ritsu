// Stage 3 (LLVM parse + verifier) through the public API, with the fixtures from
// tests/fixtures/bitcode (own sources only, see README.md there). Every case runs in both build
// modes: with AIR2SPIRV_WITH_LLVM=OFF the bitcode is not parsed, and the cases check that this
// is reported honestly instead.
#include "air2spirv/air2spirv.h"

#include "test_harness.hpp"
#include "test_inputs.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#ifndef A2S_TEST_FIXTURE_DIR
#error "A2S_TEST_FIXTURE_DIR must name tests/fixtures/bitcode"
#endif
#ifndef A2S_TEST_FUZZ_REGRESSION_DIR
#error "A2S_TEST_FUZZ_REGRESSION_DIR must name fuzz/regressions/fuzz_air_bitcode"
#endif
#ifndef A2S_TEST_HAVE_LLVM
#error "A2S_TEST_HAVE_LLVM must be 0 or 1"
#endif

namespace {

constexpr bool kHaveLlvm = A2S_TEST_HAVE_LLVM != 0;

struct ContextDeleter {
    void operator()(a2s_context* c) const noexcept { a2s_context_destroy(c); }
};
using ContextPtr = std::unique_ptr<a2s_context, ContextDeleter>;

ContextPtr make_context() {
    a2s_context* raw = nullptr;
    A2S_CHECK_EQ(a2s_context_create(nullptr, &raw), A2S_OK);
    return ContextPtr(raw);
}

std::vector<std::uint8_t> load_fixture(const std::string& name) {
    const std::string path = std::string(A2S_TEST_FIXTURE_DIR) + "/" + name;
    std::ifstream in(path, std::ios::binary);
    A2S_CHECK(in.good());
    std::vector<std::uint8_t> bytes((std::istreambuf_iterator<char>(in)),
                                    std::istreambuf_iterator<char>());
    A2S_CHECK(!bytes.empty());
    return bytes;
}

struct Result {
    a2s_status status = A2S_ERROR_INTERNAL;
    std::string diagnostics;
    bool contains(std::string_view needle) const {
        return diagnostics.find(needle) != std::string::npos;
    }
};

Result run(a2s_context* ctx, const std::vector<std::uint8_t>& bytes) {
    a2s_input input;
    a2s_input_init(&input);
    input.data = bytes.data();
    input.size = bytes.size();
    a2s_output* out = nullptr;
    Result r;
    r.status = a2s_compile(ctx, &input, &out);
    A2S_CHECK(out != nullptr);
    if (out != nullptr) {
        A2S_CHECK_EQ(a2s_output_get_status(out), r.status);
        r.diagnostics = a2s_output_get_diagnostics(out);
        std::size_t words = 1;
        A2S_CHECK(a2s_output_get_spirv(out, &words) == nullptr); // nothing is translated yet
        A2S_CHECK_EQ(words, std::size_t{0});
    }
    a2s_output_free(out);
    // Every failure explains itself.
    A2S_CHECK(r.diagnostics.find("error: ") != std::string::npos);
    return r;
}

// Status for input the LLVM build must reject: without LLVM nothing is parsed, and the
// pipeline stops at "not implemented".
a2s_status expected_without_llvm(a2s_status with_llvm) {
    return kHaveLlvm ? with_llvm : A2S_ERROR_UNIMPLEMENTED;
}

void check_not_parsed_note(const Result& r) {
    if (!kHaveLlvm) {
        A2S_CHECK(r.contains("note: bitcode not parsed: air2spirv was built without LLVM"));
    }
}

struct ValidFixture {
    const char* file;
    const char* parsed; // expected "parsed ..." diagnostic
};

constexpr std::array<ValidFixture, 6> kValidFixtures = {{
    {"minimal.bc", "parsed 1 function (1 defined, 0 declared), 0 global variables, "
                   "1 instruction"},
    {"vector_add.spir64.bc", "parsed 3 functions (2 defined, 1 declared)"},
    {"control_flow.x86_64.bc", "parsed 2 functions (2 defined, 0 declared)"},
    {"control_flow_O0.x86_64.bc", "parsed 3 functions (3 defined, 0 declared)"},
    {"structs_debug.x86_64.bc", "parsed 4 functions (2 defined, 2 declared), 1 global variable"},
    {"structs.arm64-apple.wrapped.bc", "parsed 3 functions (2 defined, 1 declared)"},
}};

// Rewraps a truncated payload so that the wrapper itself stays consistent (the truncation then
// reaches LLVM instead of being caught by the sniffer).
std::vector<std::uint8_t> truncated(const std::vector<std::uint8_t>& bytes, std::size_t keep) {
    const bool wrapped = bytes.size() >= 20 && bytes[0] == 0xDE && bytes[1] == 0xC0 &&
                         bytes[2] == 0x17 && bytes[3] == 0x0B;
    if (!wrapped) {
        return std::vector<std::uint8_t>(bytes.begin(),
                                         bytes.begin() + static_cast<std::ptrdiff_t>(keep));
    }
    const std::vector<std::uint8_t> payload(bytes.begin() + 20,
                                            bytes.begin() + 20 + static_cast<std::ptrdiff_t>(keep));
    a2s_test::WrapperFields f;
    f.size = static_cast<std::uint32_t>(payload.size());
    return a2s_test::make_wrapper(f, payload);
}

std::size_t payload_size(const std::vector<std::uint8_t>& bytes) {
    const bool wrapped = bytes.size() >= 20 && bytes[0] == 0xDE && bytes[1] == 0xC0;
    if (!wrapped) {
        return bytes.size();
    }
    return static_cast<std::size_t>(bytes[12]) | (static_cast<std::size_t>(bytes[13]) << 8U) |
           (static_cast<std::size_t>(bytes[14]) << 16U) |
           (static_cast<std::size_t>(bytes[15]) << 24U);
}

} // namespace

// ---- valid bitcode ---------------------------------------------------------------------------

A2S_TEST(valid_fixtures_parse_and_stop_at_unimplemented) {
    auto ctx = make_context();
    for (const ValidFixture& fixture : kValidFixtures) {
        const Result r = run(ctx.get(), load_fixture(fixture.file));
        A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
        A2S_CHECK(r.contains("not implemented"));
        if (kHaveLlvm) {
            if (!r.contains(fixture.parsed)) {
                a2s_test::report_failure(__FILE__, __LINE__,
                                         std::string(fixture.file) + ": expected '" +
                                             fixture.parsed + "' in:\n" + r.diagnostics);
            }
            A2S_CHECK(!r.contains("warning: "));
        } else {
            check_not_parsed_note(r);
        }
    }
}

A2S_TEST(wrapped_fixture_reports_wrapper_and_module) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), a2s_test::make_valid_bitcode_wrapper());
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    A2S_CHECK(r.contains("bitcode wrapper"));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("parsed 1 function"));
    }
}

A2S_TEST(version_string_names_the_linked_llvm) {
    const std::string version = a2s_get_version_string();
    if (kHaveLlvm) {
        A2S_CHECK(version.find("(llvm 18.1.") != std::string::npos);
        A2S_CHECK(version.find("without llvm") == std::string::npos);
    } else {
        A2S_CHECK(version.find("without llvm") != std::string::npos);
    }
}

// ---- broken bitcode -> MALFORMED ------------------------------------------------------------

A2S_TEST(truncated_bitcode_is_malformed) {
    auto ctx = make_context();
    for (const ValidFixture& fixture : kValidFixtures) {
        const auto bytes = load_fixture(fixture.file);
        const std::size_t size = payload_size(bytes);
        for (std::size_t keep = 4; keep < size; keep += 4) {
            const Result r = run(ctx.get(), truncated(bytes, keep));
            if (r.status != expected_without_llvm(A2S_ERROR_MALFORMED_INPUT)) {
                a2s_test::report_failure(__FILE__, __LINE__,
                                         std::string(fixture.file) + " truncated to " +
                                             std::to_string(keep) + " bytes: status " +
                                             a2s_status_to_string(r.status) + "\n" + r.diagnostics);
                return;
            }
        }
    }
}

A2S_TEST(destroyed_bitcode_is_malformed) {
    auto ctx = make_context();
    const auto original = load_fixture("control_flow.x86_64.bc");
    // Everything behind the identification block overwritten with a pattern.
    for (const std::uint8_t fill : {std::uint8_t{0x00}, std::uint8_t{0xFF}, std::uint8_t{0x5A}}) {
        auto bytes = original;
        for (std::size_t i = 64; i < bytes.size(); ++i) {
            bytes[i] = fill;
        }
        const Result r = run(ctx.get(), bytes);
        A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_MALFORMED_INPUT));
    }
    // Only the bitcode magic, then zeros.
    {
        const Result r = run(ctx.get(), a2s_test::make_raw_bitcode(64));
        A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_MALFORMED_INPUT));
        check_not_parsed_note(r);
    }
}

A2S_TEST(invalid_ir_fails_the_verifier) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("invalid_dominance.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_MALFORMED_INPUT));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("error: bitcode: the module does not pass the LLVM verifier"));
        A2S_CHECK(r.contains("error: verifier: Instruction does not dominate all uses!"));
    }
}

// LLVM 18 would call report_fatal_error("Broken module found") for this input from inside the
// bitcode reader (debug-info auto-upgrade). Reaching the checks at all proves the process
// survived it.
A2S_TEST(invalid_ir_with_debug_info_does_not_terminate) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("invalid_dominance_debug.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_MALFORMED_INPUT));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("does not dominate all uses"));
    }
}

// Inputs found by fuzz_air_bitcode that crash, abort or exhaust memory inside LLVM 18's reader
// and that the pre-scan now rejects (fuzz/regressions/fuzz_air_bitcode; they are also replayed
// by every fuzz run). Random damage in general is the fuzzer's job: LLVM 18 still has crash
// classes the pre-scan cannot rule out (docs/subsystems/shader-compiler.md §11.4), so a
// randomized unit test would not be deterministic in outcome.
A2S_TEST(fuzz_regressions_are_rejected_before_llvm) {
    struct Regression {
        const char* file;
        a2s_status status;
        const char* diagnostic;
    };
    constexpr std::array<Regression, 5> kRegressions = {{
        {"attr_group_unterminated_string.bc", A2S_ERROR_MALFORMED_INPUT,
         "unterminated string in attribute group record"},
        {"data_constant_scalar_type.bc", A2S_ERROR_MALFORMED_INPUT,
         "DATA constant of a type that is neither array nor vector"},
        {"vector_type_67m_elements.bc", A2S_ERROR_UNSUPPORTED,
         "vector element count 67108731 exceeds the limit"},
        {"attachment_bad_instruction.bc", A2S_ERROR_MALFORMED_INPUT,
         "metadata attachment for instruction"},
        // Use-list blocks are neutralised; the rest of this module is valid.
        {"uselist_bad_value_id.bc", A2S_ERROR_UNIMPLEMENTED, "use-list order block(s) ignored"},
    }};
    auto ctx = make_context();
    for (const Regression& regression : kRegressions) {
        const std::string path = std::string(A2S_TEST_FUZZ_REGRESSION_DIR) + "/" + regression.file;
        std::ifstream in(path, std::ios::binary);
        A2S_CHECK(in.good());
        const std::vector<std::uint8_t> bytes((std::istreambuf_iterator<char>(in)),
                                              std::istreambuf_iterator<char>());
        const Result r = run(ctx.get(), bytes);
        A2S_CHECK_EQ(r.status, expected_without_llvm(regression.status));
        if (kHaveLlvm && !r.contains(regression.diagnostic)) {
            a2s_test::report_failure(__FILE__, __LINE__,
                                     std::string(regression.file) + ": expected '" +
                                         regression.diagnostic + "' in:\n" + r.diagnostics);
        }
    }
}

// LLVM 18 would try to allocate 16 GiB for this attribute group (llvm-dis-18 aborts with
// "out of memory"); the pre-scan bounds the index by what the stream can encode.
A2S_TEST(huge_attribute_index_is_rejected_before_llvm) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("huge_attribute_index.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_MALFORMED_INPUT));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("attribute parameter index 2147483648 exceeds what the bitstream"));
    }
}

// ---- the pre-scan decodes like LLVM 18 (block layout) ---------------------------------------

// Variants of control_flow.x86_64.bc from make_layout_fixtures.py. The pre-scan protects LLVM
// only if it reads the same records LLVM reads, with the same abbreviations: layouts LLVM
// handles in a way the pre-scan cannot follow exactly are rejected, the others must come out
// as LLVM sees them.
A2S_TEST(prescan_follows_llvm_block_layout) {
    struct Layout {
        const char* file;
        a2s_status status;
        const char* diagnostic;
    };
    constexpr std::array<Layout, 11> kLayouts = {{
        // LLVM reads BLOCKINFO only as a direct child of the module and skips it elsewhere.
        {"layout_blockinfo_in_function_block.bc", A2S_ERROR_UNIMPLEMENTED, "parsed 2 functions"},
        {"layout_blockinfo_at_top_level.bc", A2S_ERROR_UNIMPLEMENTED, "parsed 2 functions"},
        {"layout_blockinfo_twice.bc", A2S_ERROR_UNSUPPORTED,
         "BLOCKINFO block at a non-standard position"},
        {"layout_blockinfo_after_function_block.bc", A2S_ERROR_UNSUPPORTED,
         "BLOCKINFO block at a non-standard position"},
        // LLVM never sees module-level definitions between function blocks.
        {"layout_abbrev_after_function_block.bc", A2S_ERROR_UNSUPPORTED,
         "abbreviation definition after the first function block"},
        // Skipped by length vs. parsed to END_BLOCK must end at the same place.
        {"layout_function_block_length_mismatch.bc", A2S_ERROR_MALFORMED_INPUT,
         "FUNCTION_BLOCK length does not match its content"},
        // LLVM jumps to symbol-table and function-body offsets without checking them.
        {"layout_vstoffset_mismatch.bc", A2S_ERROR_MALFORMED_INPUT,
         "VSTOFFSET does not point at a symbol table"},
        {"layout_fnentry_offset_mismatch.bc", A2S_ERROR_MALFORMED_INPUT,
         "does not point at a function block"},
        {"layout_fnentry_in_function_symtab.bc", A2S_ERROR_MALFORMED_INPUT,
         "function offset in a function-level symbol table"},
        {"layout_function_block_id_two_chunks.bc", A2S_ERROR_MALFORMED_INPUT,
         "does not point at a function block"},
        // LLVM skips blocks nested in a constants block; so does the pre-scan's state.
        {"layout_nested_constants_block.bc", A2S_ERROR_UNIMPLEMENTED, "parsed 2 functions"},
    }};
    auto ctx = make_context();
    for (const Layout& layout : kLayouts) {
        const Result r = run(ctx.get(), load_fixture(layout.file));
        if (r.status != expected_without_llvm(layout.status) ||
            (kHaveLlvm && !r.contains(layout.diagnostic))) {
            a2s_test::report_failure(__FILE__, __LINE__,
                                     std::string(layout.file) + ": status " +
                                         a2s_status_to_string(r.status) + ", expected '" +
                                         layout.diagnostic + "' in:\n" + r.diagnostics);
        }
    }
}

// ---- newer than LLVM 18 -> UNSUPPORTED -------------------------------------------------------

A2S_TEST(newer_epoch_is_unsupported) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("unsupported_epoch1.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_UNSUPPORTED));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("error: bitcode: epoch 1 is not supported"));
    }
}

A2S_TEST(newer_module_version_is_unsupported) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("unsupported_module_version3.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_UNSUPPORTED));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("error: bitcode: module version 3 is newer than the linked LLVM"));
    }
}

A2S_TEST(unknown_attribute_kind_is_unsupported) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("unsupported_attribute_kind100.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_UNSUPPORTED));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("attribute kind unknown to the linked LLVM"));
        A2S_CHECK(r.contains("Unknown attribute kind (100)"));
    }
}

A2S_TEST(several_modules_are_unsupported) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("two_modules.bc"));
    A2S_CHECK_EQ(r.status, expected_without_llvm(A2S_ERROR_UNSUPPORTED));
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("contains 2 modules"));
    }
}

// ---- debug info ------------------------------------------------------------------------------

A2S_TEST(invalid_debug_info_is_dropped_with_warning) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("broken_debug_info.bc"));
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("warning: bitcode: invalid debug info dropped"));
        A2S_CHECK(r.contains("parsed 1 function"));
    }
}

A2S_TEST(unsupported_debug_info_version_is_dropped_with_warning) {
    auto ctx = make_context();
    const Result r = run(ctx.get(), load_fixture("old_debug_info_version.bc"));
    A2S_CHECK_EQ(r.status, A2S_ERROR_UNIMPLEMENTED);
    if (kHaveLlvm) {
        A2S_CHECK(r.contains("warning: bitcode: debug info version 2 is not supported"));
        A2S_CHECK(r.contains("parsed 1 function"));
    }
}

// ---- concurrency and determinism -------------------------------------------------------------

A2S_TEST(parallel_parsing_matches_sequential_results) {
    auto ctx = make_context();
    const std::array<const char*, 6> files = {
        "control_flow.x86_64.bc",           "structs_debug.x86_64.bc", "invalid_dominance_debug.bc",
        "unsupported_attribute_kind100.bc", "broken_debug_info.bc",    "vector_add.spir64.bc"};
    std::vector<std::vector<std::uint8_t>> inputs;
    std::vector<Result> expected;
    for (const char* file : files) {
        inputs.push_back(load_fixture(file));
        expected.push_back(run(ctx.get(), inputs.back()));
    }
    constexpr std::size_t kThreads = 6;
    constexpr int kIterations = 20;
    std::array<int, kThreads> mismatches{};
    std::vector<std::thread> threads;
    for (std::size_t t = 0; t < kThreads; ++t) {
        threads.emplace_back([&, t] {
            for (int i = 0; i < kIterations; ++i) {
                const std::size_t k = (t + static_cast<std::size_t>(i)) % inputs.size();
                a2s_input input;
                a2s_input_init(&input);
                input.data = inputs[k].data();
                input.size = inputs[k].size();
                a2s_output* out = nullptr;
                const a2s_status s = a2s_compile(ctx.get(), &input, &out);
                if (out == nullptr || s != expected[k].status ||
                    expected[k].diagnostics != a2s_output_get_diagnostics(out)) {
                    ++mismatches[t];
                }
                a2s_output_free(out);
            }
        });
    }
    for (auto& thread : threads) {
        thread.join();
    }
    for (const int m : mismatches) {
        A2S_CHECK_EQ(m, 0);
    }
}
