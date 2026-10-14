// Tests for the FileCheck-style matcher used on spirv-dis output (tests/support/filecheck.hpp).
#include "filecheck.hpp"
#include "test_harness.hpp"

#include <cstdio>
#include <string>
#include <string_view>

using a2s_test::filecheck;
using a2s_test::FileCheckOptions;

namespace {

constexpr std::string_view kInput = "OpCapability Shader\n"
                                    "OpMemoryModel Logical GLSL450\n"
                                    "%void = OpTypeVoid\n"
                                    "%uint = OpTypeInt 32 0\n"
                                    "\n"
                                    "%main = OpFunction %void None %3\n"
                                    "%5 = OpLabel\n"
                                    "OpReturn\n"
                                    "OpFunctionEnd\n";

bool passes(std::string_view checks, std::string_view input = kInput) {
    const auto result = filecheck(checks, input);
    if (!result.ok) {
        // Shown only when a test expecting success fails.
        std::fprintf(stderr, "filecheck message: %s\n", result.message.c_str());
    }
    return result.ok;
}

// Expects a failure whose message contains `needle`.
bool fails_with(std::string_view checks, std::string_view needle, std::string_view input = kInput) {
    const auto result = filecheck(checks, input);
    if (result.ok) {
        std::fprintf(stderr, "filecheck unexpectedly passed\n");
        return false;
    }
    if (result.message.find(needle) == std::string::npos) {
        std::fprintf(stderr, "message '%s' lacks '%.*s'\n", result.message.c_str(),
                     static_cast<int>(needle.size()), needle.data());
        return false;
    }
    return true;
}

} // namespace

// ---- CHECK ---------------------------------------------------------------------------------

A2S_TEST(check_matches_in_order) {
    A2S_CHECK(passes("; CHECK: OpCapability Shader\n"
                     "; CHECK: OpTypeInt 32 0\n"
                     "; CHECK: OpFunctionEnd\n"));
}

A2S_TEST(check_fails_when_out_of_order) {
    A2S_CHECK(fails_with("; CHECK: OpTypeInt\n; CHECK: OpTypeVoid\n",
                         "check line 2: CHECK: expected string not found"));
}

A2S_TEST(check_fails_when_missing_and_reports_pattern) {
    A2S_CHECK(fails_with("// some comment\n// CHECK: OpTypeFloat 32\n", "pattern: OpTypeFloat 32"));
}

A2S_TEST(check_can_match_later_on_the_same_line) {
    A2S_CHECK(passes("CHECK: %main\nCHECK: %void\n"));
}

A2S_TEST(check_is_a_substring_match_with_literal_metacharacters) {
    A2S_CHECK(passes("CHECK: Type\n"));
    A2S_CHECK(fails_with("CHECK: Op.ype\n", "expected string not found"));
    A2S_CHECK(passes("CHECK: (a+b)*[c]{d}|$^\\", "x (a+b)*[c]{d}|$^\\ y\n"));
}

A2S_TEST(whitespace_runs_match_flexibly) {
    A2S_CHECK(passes("CHECK: %uint   =  OpTypeInt\n"));
    A2S_CHECK(passes("CHECK: a b\n", "a \t  b\n"));
    A2S_CHECK(fails_with("CHECK: ab\n", "not found", "a b\n"));
}

// ---- CHECK-NEXT / CHECK-SAME / CHECK-EMPTY --------------------------------------------------

A2S_TEST(next_matches_the_following_line) {
    A2S_CHECK(passes("CHECK: OpLabel\nCHECK-NEXT: OpReturn\nCHECK-NEXT: OpFunctionEnd\n"));
}

A2S_TEST(next_fails_when_a_line_is_skipped) {
    A2S_CHECK(fails_with("CHECK: OpLabel\nCHECK-NEXT: OpFunctionEnd\n",
                         "check line 2: CHECK-NEXT: first match is on input line 9, expected on "
                         "the next line 8"));
}

A2S_TEST(next_fails_when_the_match_is_on_the_same_line) {
    A2S_CHECK(fails_with("CHECK: %main\nCHECK-NEXT: OpFunction\n",
                         "(the same line as the previous match)"));
    // LLVM semantics: a match later on the same line is not skipped in favour of the next line.
    A2S_CHECK(fails_with("CHECK: OpA\nCHECK-NEXT: OpB\n", "first match is on input line 1",
                         "OpA OpB\nOpB\n"));
    A2S_CHECK(passes("CHECK: OpA\nCHECK-NEXT: OpB\n", "OpA x\nOpB\n"));
}

A2S_TEST(next_fails_at_end_of_input) {
    A2S_CHECK(fails_with("CHECK: OpFunctionEnd\nCHECK-NEXT: OpNop\n", "expected string not found"));
}

A2S_TEST(same_matches_later_on_the_same_line) {
    A2S_CHECK(passes("CHECK: OpFunction\nCHECK-SAME: %void\nCHECK-SAME: None\n"));
}

A2S_TEST(same_fails_on_another_line_or_before_the_match) {
    A2S_CHECK(fails_with("CHECK: OpLabel\nCHECK-SAME: OpReturn\n", "later on the same"));
    // %main precedes OpFunction on that line.
    A2S_CHECK(fails_with("CHECK: OpFunction\nCHECK-SAME: %main\n", "later on the same"));
}

A2S_TEST(empty_requires_an_empty_next_line) {
    A2S_CHECK(passes("CHECK: OpTypeInt\nCHECK-EMPTY:\nCHECK-NEXT: OpFunction\n"));
    A2S_CHECK(fails_with("CHECK: OpTypeVoid\nCHECK-EMPTY:\n", "is not empty"));
}

// ---- CHECK-NOT ------------------------------------------------------------------------------

A2S_TEST(not_passes_when_absent_between_matches) {
    A2S_CHECK(passes("CHECK: OpTypeVoid\nCHECK-NOT: OpLabel\nCHECK: OpFunction\n"));
}

A2S_TEST(not_fails_when_present_between_matches) {
    A2S_CHECK(fails_with("CHECK: OpMemoryModel\nCHECK-NOT: OpTypeInt\nCHECK: OpFunction\n",
                         "check line 2: CHECK-NOT: excluded string found on input line 4"));
}

A2S_TEST(not_only_covers_the_range_up_to_the_next_match) {
    // OpReturn exists, but only after the OpLabel match.
    A2S_CHECK(passes("CHECK: OpFunction\nCHECK-NOT: OpReturn\nCHECK: OpLabel\n"));
}

A2S_TEST(not_at_the_start_and_end) {
    A2S_CHECK(passes("CHECK-NOT: OpTypeFloat\nCHECK: OpTypeVoid\nCHECK-NOT: OpKill\n"));
    A2S_CHECK(fails_with("CHECK-NOT: OpReturn\n", "excluded string found on input line 8"));
    A2S_CHECK(fails_with("CHECK: OpLabel\nCHECK-NOT: OpReturn\n", "excluded string found"));
}

A2S_TEST(not_does_not_see_text_before_the_previous_match) {
    A2S_CHECK(passes("CHECK: OpFunctionEnd\nCHECK-NOT: OpCapability\n"));
}

// ---- CHECK-DAG ------------------------------------------------------------------------------

A2S_TEST(dag_matches_in_any_order) {
    A2S_CHECK(passes("CHECK-DAG: %uint = OpTypeInt\n"
                     "CHECK-DAG: %void = OpTypeVoid\n"
                     "CHECK: OpFunction\n"));
}

A2S_TEST(dag_matches_must_not_overlap) {
    A2S_CHECK(passes("CHECK-DAG: x\nCHECK-DAG: x\n", "a x b x\n"));
    A2S_CHECK(fails_with("CHECK-DAG: x\nCHECK-DAG: x\n", "CHECK-DAG", "only one x\n"));
}

A2S_TEST(check_after_dag_continues_after_the_furthest_dag_match) {
    A2S_CHECK(fails_with("CHECK-DAG: OpTypeInt\nCHECK-DAG: OpTypeVoid\nCHECK: OpTypeVoid\n",
                         "check line 3"));
    A2S_CHECK(passes("CHECK-DAG: OpTypeInt\nCHECK-DAG: OpTypeVoid\nCHECK-EMPTY:\n"
                     "CHECK-NEXT: OpFunction\n",
                     "OpTypeVoid\nOpTypeInt\n\nOpFunction\n"));
}

A2S_TEST(not_before_dag_uses_bindings_from_before_the_dag_run) {
    // The DAG run rebinds X to 2; the NOT before it must still look for "x1".
    A2S_CHECK(
        fails_with("CHECK: def [[X:[0-9]+]]\nCHECK-NOT: x[[X]]\nCHECK-DAG: def [[X:[0-9]+]]\n",
                   "excluded string found on input line 2", "def 1\nx1\ndef 2\n"));
    A2S_CHECK(passes("CHECK: def [[X:[0-9]+]]\nCHECK-NOT: x[[X]]\nCHECK-DAG: def [[X:[0-9]+]]\n"
                     "CHECK: x[[X]]\n",
                     "def 1\nx2\ndef 2\nx2\n"));
    // Later DAGs of the same run see the run's bindings.
    A2S_CHECK(passes("CHECK-DAG: a=[[A:[0-9]]]\nCHECK-DAG: b=[[A]]\n", "b=5\na=5\n"));
}

A2S_TEST(not_before_dag_covers_up_to_the_first_dag_match) {
    A2S_CHECK(fails_with("CHECK: OpCapability\nCHECK-NOT: OpMemoryModel\n"
                         "CHECK-DAG: OpTypeInt\nCHECK-DAG: OpTypeVoid\n",
                         "excluded string found on input line 2"));
}

// ---- regexes and variables ------------------------------------------------------------------

A2S_TEST(regex_blocks) {
    A2S_CHECK(passes("CHECK: OpTypeInt {{[0-9]+}} {{0|1}}\n"));
    A2S_CHECK(passes("CHECK: {{^}}OpReturn{{$}}\n"));
    A2S_CHECK(fails_with("CHECK: {{^}}OpTypeVoid\n", "not found")); // not at line start
    A2S_CHECK(passes("CHECK: OpLabel\nCHECK-NEXT: {{^OpReturn$}}\n"));
}

A2S_TEST(regex_never_spans_lines) {
    A2S_CHECK(fails_with("CHECK: OpTypeVoid{{[^x]*}}OpTypeInt\n", "not found"));
}

A2S_TEST(variables_bind_and_substitute) {
    A2S_CHECK(passes("CHECK: [[T:%[a-z]+]] = OpTypeInt\n"
                     "CHECK: OpFunction [[VOID:%[a-z]+]]\n"
                     "CHECK-NOT: [[T]]\n"
                     "CHECK: OpFunctionEnd\n"));
    A2S_CHECK(passes("CHECK: %[[ID:[0-9]+]] = OpLabel\n", kInput));
    A2S_CHECK(passes("CHECK: def [[X:[0-9]+]]\nCHECK: use [[X]]\n", "def 42\nuse 42\n"));
    A2S_CHECK(fails_with("CHECK: def [[X:[0-9]+]]\nCHECK: use [[X]]\n", "with: X=\"42\"",
                         "def 42\nuse 43\n"));
}

A2S_TEST(variables_with_inner_groups_capture_the_whole_match) {
    A2S_CHECK(passes("CHECK: a [[V:(x|y)(z)]] b {{(c)}} [[W:[0-9]]]\nCHECK: [[V]][[W]]\n",
                     "a yz b c 7\nyz7\n"));
}

A2S_TEST(variables_can_be_redefined) {
    A2S_CHECK(
        passes("CHECK: v=[[A:[0-9]]]\nCHECK: v=[[A:[0-9]]]\nCHECK: [[A]]!\n", "v=1\nv=2\n2!\n"));
}

A2S_TEST(variable_values_are_matched_literally) {
    A2S_CHECK(passes("CHECK: [[V:a.c]]\nCHECK: =[[V]]\n", "a.c\n=a.c\n"));
    A2S_CHECK(fails_with("CHECK: [[V:a.c]]\nCHECK: =[[V]]\n", "not found", "a.c\n=abc\n"));
}

// ---- errors in the check text ---------------------------------------------------------------

A2S_TEST(no_directives_is_an_error) {
    A2S_CHECK(fails_with("", "no 'CHECK:' directives"));
    A2S_CHECK(fails_with("; nothing here\n; CHECKS: ignored\n", "no 'CHECK:' directives"));
}

A2S_TEST(misspelled_directive_is_an_error) {
    A2S_CHECK(fails_with("CHECK-NXET: x\n", "check line 1: unknown directive 'CHECK-NXET'"));
    A2S_CHECK(fails_with("CHECK-NXET x\n", "unknown directive 'CHECK-NXET'"));
    A2S_CHECK(fails_with("CHECK-: x\n", "unknown directive 'CHECK-'"));
}

A2S_TEST(wrong_case_suffix_is_an_error) {
    A2S_CHECK(fails_with("CHECK: Op\nCHECK-Next: zzz\n", "must be spelled 'CHECK-NEXT'", "a\n"));
    A2S_CHECK(fails_with("CHECK-not: zzz\n", "must be spelled 'CHECK-NOT'", "a\n"));
}

A2S_TEST(known_directive_without_colon_is_an_error) {
    A2S_CHECK(fails_with("CHECK: Op\nCHECK-NEXT OpMemoryModel\n",
                         "'CHECK-NEXT' must be followed directly by ':'"));
    A2S_CHECK(fails_with("CHECK-NOT : x\n", "must be followed directly by ':'"));
}

A2S_TEST(prefix_followed_by_space_or_text_is_an_error) {
    A2S_CHECK(
        fails_with("; CHECK OpNOPE\n", "check line 1: 'CHECK' must be followed directly", "a\n"));
    A2S_CHECK(fails_with("; CHECK : OpNOPE\n", "must be followed directly", "a\n"));
    A2S_CHECK(fails_with("; CHECK\n", "must be followed directly", "a\n"));
    A2S_CHECK(fails_with("; CHECK.\n", "must be followed directly", "a\n"));
}

A2S_TEST(unsupported_llvm_directives_are_errors) {
    A2S_CHECK(fails_with("CHECK-COUNT-2: zzz\n", "'CHECK-COUNT' is not supported", "a\n"));
    A2S_CHECK(fails_with("CHECK-LABEL zzz\n", "'CHECK-LABEL' is not supported", "a\n"));
    A2S_CHECK(fails_with("CHECK-LABEL: zzz\n", "'CHECK-LABEL' is not supported", "a\n"));
}

A2S_TEST(empty_pattern_is_an_error) {
    A2S_CHECK(fails_with("CHECK:\n", "empty pattern"));
    A2S_CHECK(fails_with("CHECK:   \n", "empty pattern"));
    A2S_CHECK(fails_with("CHECK: Op\nCHECK-EMPTY: x\n", "takes no pattern"));
}

A2S_TEST(relative_directive_without_previous_match_is_an_error) {
    A2S_CHECK(fails_with("CHECK-NEXT: OpCapability\n", "needs a previous positive match"));
    A2S_CHECK(fails_with("CHECK-NOT: x\nCHECK-SAME: OpCapability\n", "needs a previous"));
}

A2S_TEST(malformed_patterns_are_errors) {
    A2S_CHECK(fails_with("CHECK: {{abc\n", "unterminated '{{'"));
    A2S_CHECK(fails_with("CHECK: {{}}\n", "empty '{{}}'"));
    A2S_CHECK(fails_with("CHECK: [[X\n", "unterminated '[['"));
    A2S_CHECK(fails_with("CHECK: [[1X:a]]\n", "invalid variable name"));
    A2S_CHECK(fails_with("CHECK: [[X:]]\n", "empty regex"));
    A2S_CHECK(fails_with("CHECK: {{(}}\n", "invalid regex"));
    A2S_CHECK(fails_with("CHECK: [[NOPE]]\n", "undefined variable 'NOPE'"));
    A2S_CHECK(fails_with("CHECK: [[X:a]] [[X]]\n", "used in the pattern that defines it"));
    A2S_CHECK(fails_with("CHECK: Op\nCHECK-NOT: [[X:a]]\n", "cannot define variables"));
}

A2S_TEST(prefix_must_stand_alone_and_is_configurable) {
    // XCHECK is not CHECK; only the MINE directives count with prefix MINE.
    A2S_CHECK(passes("XCHECK: not a directive\nCHECK: OpTypeVoid\n"));
    A2S_CHECK(passes("the CHECKS_below and CHECKER: are other words\nCHECK: OpTypeVoid\n"));
    FileCheckOptions mine;
    mine.prefix = "MINE";
    A2S_CHECK(filecheck("CHECK: does not exist\nMINE: OpReturn\n", kInput, mine).ok);
    A2S_CHECK(!filecheck("MINE: does not exist\n", kInput, mine).ok);
    mine.prefix = "";
    A2S_CHECK(!filecheck("CHECK: OpReturn\n", kInput, mine).ok);
    mine.prefix = "BAD PREFIX";
    A2S_CHECK(!filecheck("CHECK: OpReturn\n", kInput, mine).ok);
}

A2S_TEST(input_without_trailing_newline_and_crlf_checks) {
    A2S_CHECK(passes("CHECK: b\nCHECK-NOT: c\n", "a\nb"));
    A2S_CHECK(passes("CHECK: one\r\nCHECK-NEXT: two\r\n", "one\ntwo\n"));
    A2S_CHECK(fails_with("CHECK: x\n", "not found", ""));
}
