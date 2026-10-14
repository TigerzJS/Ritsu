#include "bitcode/llvm_runtime.hpp"

#include <array>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <llvm/Support/CommandLine.h>
#include <llvm/Support/ErrorHandling.h>
#include <mutex>

namespace air2spirv::bitcode {
namespace {

constexpr const char* kDisableDebugInfoUpgrade = "disable-auto-upgrade-debug-info";

// Called by LLVM on report_fatal_error / allocation failure inside LLVM. Must not return
// (report_bad_alloc_error treats a return as unreachable). Avoids heap allocation: the reason
// may be an out-of-memory report. Guest-controlled bytes in `reason` are replaced so they
// cannot inject terminal control sequences into the host log.
[[noreturn]] void report_and_abort(const char* kind, const char* reason) noexcept {
    std::array<char, 512> text{};
    std::size_t n = 0;
    if (reason != nullptr) {
        for (const char* p = reason; *p != '\0' && n + 1 < text.size(); ++p) {
            const auto c = static_cast<unsigned char>(*p);
            text[n++] = (c >= 0x20 && c < 0x7F) ? static_cast<char>(c) : '?';
        }
    }
    text[n] = '\0';
    std::fprintf(stderr, "air2spirv: unrecoverable %s inside LLVM, aborting: %s\n", kind,
                 text.data());
    std::fflush(stderr);
    std::abort();
}

void fatal_error_handler(void* /*user_data*/, const char* reason, bool /*gen_crash_diag*/) {
    report_and_abort("fatal error", reason);
}

void bad_alloc_handler(void* /*user_data*/, const char* reason, bool /*gen_crash_diag*/) {
    report_and_abort("allocation failure", reason);
}

struct RuntimeState {
    bool ok = false;
    std::string error;
};

RuntimeState configure() {
    RuntimeState state;
    llvm::install_fatal_error_handler(&fatal_error_handler, nullptr);
    llvm::install_bad_alloc_error_handler(&bad_alloc_handler, nullptr);

    auto& options = llvm::cl::getRegisteredOptions();
    const auto it = options.find(kDisableDebugInfoUpgrade);
    if (it == options.end() || it->second == nullptr) {
        state.error = std::string("LLVM option -") + kDisableDebugInfoUpgrade +
                      " is not registered; the linked LLVM is not the pinned version";
        return state;
    }
    // addOccurrence returns true on error.
    if (it->second->addOccurrence(0, kDisableDebugInfoUpgrade, "true")) {
        state.error = std::string("could not set LLVM option -") + kDisableDebugInfoUpgrade;
        return state;
    }
    state.ok = true;
    return state;
}

} // namespace

bool prepare_llvm_runtime(std::string& error) {
    static std::once_flag once;
    static RuntimeState state;
    std::call_once(once, [] { state = configure(); });
    if (!state.ok) {
        error = state.error;
    }
    return state.ok;
}

} // namespace air2spirv::bitcode
