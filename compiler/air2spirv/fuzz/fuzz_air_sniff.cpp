// libFuzzer target: arbitrary bytes as bitcode / container input (docs/process/testing.md §5.1).
// Goes through the public entry point a2s_compile() exactly like production code, and checks
// the internal sniffer invariants on the same bytes.
#include "air2spirv/air2spirv.h"

#include "diagnostics.hpp"
#include "input_sniffer.hpp"

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <span>

namespace {

a2s_context* g_context = nullptr;

[[noreturn]] void invariant_failed() {
    std::abort();
}

} // namespace

extern "C" int LLVMFuzzerInitialize(int* /*argc*/, char*** /*argv*/) {
    if (a2s_context_create(nullptr, &g_context) != A2S_OK) {
        invariant_failed();
    }
    return 0;
}

extern "C" int LLVMFuzzerTestOneInput(const std::uint8_t* data, std::size_t size) {
    // 1. Public API: must return a defined status and a consistent output object.
    a2s_input input;
    a2s_input_init(&input);
    input.data = data;
    input.size = size;
    a2s_output* output = nullptr;
    const a2s_status status = a2s_compile(g_context, &input, &output);
    switch (status) {
    case A2S_ERROR_MALFORMED_INPUT:
    case A2S_ERROR_UNSUPPORTED:
    case A2S_ERROR_UNIMPLEMENTED:
        break;
    default: // OK is impossible until translation exists; anything else is a bug
        invariant_failed();
    }
    if (output == nullptr || a2s_output_get_status(output) != status ||
        a2s_output_get_diagnostics(output)[0] == '\0') {
        invariant_failed();
    }
    a2s_output_free(output);

    // 2. Sniffer invariants: a located stream lies inside the input and looks like bitcode.
    const auto bytes = std::as_bytes(std::span<const std::uint8_t>(data, size));
    air2spirv::SniffedInput sniffed;
    air2spirv::Diagnostics diag;
    if (air2spirv::sniff_input(bytes, sniffed, diag) == A2S_OK) {
        const auto* begin = bytes.data();
        const auto* end = bytes.data() + bytes.size();
        if (sniffed.bitcode.data() < begin ||
            sniffed.bitcode.data() + sniffed.bitcode.size() > end || sniffed.bitcode.empty() ||
            sniffed.bitcode.size() % 4 != 0 || !air2spirv::has_raw_bitcode_magic(sniffed.bitcode)) {
            invariant_failed();
        }
    } else if (!diag.has_errors()) {
        invariant_failed();
    }
    return 0;
}
