// A small FileCheck-style matcher for tests on `spirv-dis` output
// (docs/subsystems/shader-compiler.md §10, WP-SC-03). No external dependencies.
//
// Check text: every line that contains a directive `<PREFIX>[-<KIND>]:` contributes one
// pattern (the text after the colon, leading/trailing whitespace removed). The prefix must not
// be preceded by a letter, digit, '_' or '-', so it can follow any comment marker
// ("; CHECK: ...", "// CHECK: ..."). An occurrence of the prefix that is followed by a letter,
// digit or '_' ("CHECKS") is a different word and ignored; every other occurrence must be a
// well-formed directive.
//
//   CHECK:       pattern found at or after the end of the previous match
//   CHECK-NEXT:  the first match after the previous match is on the line directly after it
//   CHECK-SAME:  pattern found on the same line as the previous match, after it
//   CHECK-EMPTY: the line directly after the previous match is empty (no pattern)
//   CHECK-NOT:   pattern does not occur between the previous match and the next positive
//                match (or the end of the input)
//   CHECK-DAG:   a run of consecutive CHECK-DAG lines matches in any order, after the previous
//                match, without overlapping each other; the run ends at its furthest match
//
// Pattern syntax (per line, a match never spans lines):
//   literal text   matched literally; a run of spaces/tabs matches one or more spaces/tabs
//   {{regex}}      ECMAScript regular expression (std::regex); ends at the first "}}"
//   [[NAME:regex]] matches regex and binds the matched text to NAME
//   [[NAME]]       the text bound to NAME earlier (by a previous directive), literally
//
// Errors, never silently ignored: misspelled or unknown directives (`CHECK-NXET:`,
// `CHECK-Next:`), directives without directly following colon (`CHECK-NEXT x`, `CHECK :`,
// `CHECK x`), unsupported LLVM directives (`CHECK-COUNT-<n>:`, `CHECK-LABEL:`), empty
// patterns, undefined variables and invalid regexes.
#pragma once

#include <string>
#include <string_view>

namespace a2s_test {

struct FileCheckOptions {
    std::string prefix = "CHECK";
};

struct FileCheckResult {
    bool ok = false;
    // Empty on success. On failure: check line, directive, pattern, and the input position.
    std::string message;
};

[[nodiscard]] FileCheckResult filecheck(std::string_view checks, std::string_view input,
                                        const FileCheckOptions& options = {});

} // namespace a2s_test
