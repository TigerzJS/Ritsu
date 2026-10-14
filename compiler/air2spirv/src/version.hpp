// Build identity of the library; part of every shader cache key
// (docs/subsystems/shader-compiler.md §7, §8).
#pragma once

namespace air2spirv {

// "air2spirv <version>+<build id> (<llvm description>, spirv-tools <version> [<identity>])",
// identity = package identity from configure time (e.g. "spirv-tools 2025.1~rc1-1~ubuntu0.24.04.2")
// or the SPIRV-Tools build details. Includes every component whose behaviour changes the emitted
// SPIR-V. Built once, never truncated, lives for the whole process; never returns null.
[[nodiscard]] const char* version_string() noexcept;

} // namespace air2spirv
