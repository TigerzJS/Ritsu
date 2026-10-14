// Process-wide LLVM settings air2spirv depends on (docs/subsystems/shader-compiler.md §2.3, §9).
//
// LLVM keeps a few pieces of global state. They are configured exactly once per process, before
// the first bitcode is parsed:
//   - a fatal-error handler and a bad-alloc handler. Both report the reason (sanitised) on
//     stderr and abort(). LLVM cannot continue after either (it is built without exceptions,
//     so unwinding out of it would skip its cleanups), and abort() is deterministic for a
//     supervising process, unlike the exit(1) LLVM would call itself.
//   - the hidden option -disable-auto-upgrade-debug-info. Without it the bitcode reader runs
//     the verifier itself whenever a module carries current debug info and calls
//     report_fatal_error("Broken module found") if it fails, so any hostile bitcode with
//     debug info and one invalid instruction would kill the process. air2spirv runs the
//     verifier itself afterwards and strips invalid debug info the way the auto-upgrade would.
#pragma once

#include <string>

namespace air2spirv::bitcode {

// Idempotent and thread-safe. Returns false (with the reason in `error`) if the LLVM runtime
// could not be configured; parsing must then be refused, because hostile input could reach a
// fatal error path.
[[nodiscard]] bool prepare_llvm_runtime(std::string& error);

} // namespace air2spirv::bitcode
