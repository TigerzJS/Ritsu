// air2spirv-cli: command-line front end for the air2spirv C API.
//
//   air2spirv-cli <input> -o <out.spv> [--entry <name>]
//
// Exit codes (stable, used by tests and scripts):
//   0  success, SPIR-V written
//   1  input malformed           (A2S_ERROR_MALFORMED_INPUT)
//   2  usage error / invalid argument
//   3  I/O error (reading input or writing output)
//   4  input unsupported         (A2S_ERROR_UNSUPPORTED)
//   5  not implemented yet       (A2S_ERROR_UNIMPLEMENTED)
//   6  internal error / out of memory
#include "air2spirv/air2spirv.h"

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <exception>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace {

enum ExitCode : int {
    kExitOk = 0,
    kExitMalformed = 1,
    kExitUsage = 2,
    kExitIo = 3,
    kExitUnsupported = 4,
    kExitUnimplemented = 5,
    kExitInternal = 6,
};

constexpr std::string_view kUsage =
    "usage: air2spirv-cli <input> -o <out.spv> [--entry <name>]\n"
    "       air2spirv-cli --help | --version\n"
    "\n"
    "Translates Apple AIR (LLVM bitcode, optionally in a bitcode wrapper) to Vulkan SPIR-V.\n"
    "\n"
    "options:\n"
    "  -o <file>        write the SPIR-V module to <file> (required)\n"
    "  --entry <name>   entry point to translate (default: the only one in the input)\n"
    "  -h, --help       show this help\n"
    "  --version        show the library version\n"
    "\n"
    "exit codes: 0 ok, 1 malformed input, 2 usage, 3 I/O, 4 unsupported,\n"
    "            5 not implemented, 6 internal error\n";

struct Arguments {
    std::string input_path;
    std::string output_path;
    std::optional<std::string> entry_point;
};

int exit_code_for(a2s_status status) {
    switch (status) {
    case A2S_OK:
        return kExitOk;
    case A2S_ERROR_MALFORMED_INPUT:
        return kExitMalformed;
    case A2S_ERROR_INVALID_ARGUMENT:
        return kExitUsage;
    case A2S_ERROR_UNSUPPORTED:
        return kExitUnsupported;
    case A2S_ERROR_UNIMPLEMENTED:
        return kExitUnimplemented;
    case A2S_ERROR_INTERNAL:
    case A2S_ERROR_OUT_OF_MEMORY:
    case A2S_STATUS_MAX_ENUM:
        break;
    }
    return kExitInternal;
}

// Returns nullopt and prints a message on usage errors. Sets `exit_early` for --help/--version.
std::optional<Arguments> parse_arguments(std::span<char*> args, bool& exit_early) {
    exit_early = false;
    Arguments parsed;
    bool have_input = false;
    for (std::size_t i = 1; i < args.size(); ++i) {
        const std::string_view arg = args[i];
        const auto next_value = [&](std::string_view option) -> std::optional<std::string> {
            if (i + 1 >= args.size()) {
                std::cerr << "air2spirv-cli: option " << option << " needs a value\n";
                return std::nullopt;
            }
            ++i;
            return std::string(args[i]);
        };
        if (arg == "-h" || arg == "--help") {
            std::cout << kUsage;
            exit_early = true;
            return parsed;
        }
        if (arg == "--version") {
            std::cout << a2s_get_version_string() << '\n';
            exit_early = true;
            return parsed;
        }
        if (arg == "-o") {
            auto value = next_value(arg);
            if (!value) {
                return std::nullopt;
            }
            parsed.output_path = std::move(*value);
        } else if (arg == "--entry") {
            auto value = next_value(arg);
            if (!value) {
                return std::nullopt;
            }
            parsed.entry_point = std::move(*value);
        } else if (!arg.empty() && arg.front() == '-' && arg != "-") {
            std::cerr << "air2spirv-cli: unknown option '" << arg << "'\n";
            return std::nullopt;
        } else if (have_input) {
            std::cerr << "air2spirv-cli: more than one input file given\n";
            return std::nullopt;
        } else {
            parsed.input_path = std::string(arg);
            have_input = true;
        }
    }
    if (!have_input) {
        std::cerr << "air2spirv-cli: no input file given\n";
        return std::nullopt;
    }
    if (parsed.output_path.empty()) {
        std::cerr << "air2spirv-cli: no output file given (-o)\n";
        return std::nullopt;
    }
    return parsed;
}

std::optional<std::vector<std::uint8_t>> read_file(const std::string& path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) {
        return std::nullopt;
    }
    std::vector<std::uint8_t> bytes((std::istreambuf_iterator<char>(in)),
                                    std::istreambuf_iterator<char>());
    if (in.bad()) {
        return std::nullopt;
    }
    return bytes;
}

bool write_spirv(const std::string& path, std::span<const std::uint32_t> words) {
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    if (!out) {
        return false;
    }
    // SPIR-V files are conventionally little-endian; write explicitly, independent of the host.
    for (const std::uint32_t word : words) {
        const char bytes[4] = {
            static_cast<char>(word & 0xFFU),
            static_cast<char>((word >> 8U) & 0xFFU),
            static_cast<char>((word >> 16U) & 0xFFU),
            static_cast<char>((word >> 24U) & 0xFFU),
        };
        out.write(bytes, sizeof(bytes));
    }
    out.flush();
    return static_cast<bool>(out);
}

struct ContextDeleter {
    void operator()(a2s_context* context) const noexcept { a2s_context_destroy(context); }
};
struct OutputDeleter {
    void operator()(a2s_output* output) const noexcept { a2s_output_free(output); }
};

int run(std::span<char*> args) {
    bool exit_early = false;
    const std::optional<Arguments> parsed = parse_arguments(args, exit_early);
    if (exit_early) {
        return kExitOk;
    }
    if (!parsed) {
        std::cerr << kUsage;
        return kExitUsage;
    }

    const auto bytes = read_file(parsed->input_path);
    if (!bytes) {
        std::cerr << "air2spirv-cli: cannot read '" << parsed->input_path << "'\n";
        return kExitIo;
    }

    a2s_context* raw_context = nullptr;
    const a2s_status create_status = a2s_context_create(nullptr, &raw_context);
    std::unique_ptr<a2s_context, ContextDeleter> context(raw_context);
    if (create_status != A2S_OK) {
        std::cerr << "air2spirv-cli: cannot create context: " << a2s_status_to_string(create_status)
                  << '\n';
        return exit_code_for(create_status);
    }

    a2s_compile_options options;
    a2s_compile_options_init(&options);
    a2s_input input;
    a2s_input_init(&input);
    input.data = bytes->data();
    input.size = bytes->size();
    input.entry_point = parsed->entry_point ? parsed->entry_point->c_str() : nullptr;
    input.options = &options;

    a2s_output* raw_output = nullptr;
    const a2s_status status = a2s_compile(context.get(), &input, &raw_output);
    std::unique_ptr<a2s_output, OutputDeleter> output(raw_output);

    std::cerr << a2s_output_get_diagnostics(output.get());
    if (status != A2S_OK) {
        std::cerr << "air2spirv-cli: " << parsed->input_path << ": " << a2s_status_to_string(status)
                  << '\n';
        return exit_code_for(status);
    }

    std::size_t word_count = 0;
    const std::uint32_t* words = a2s_output_get_spirv(output.get(), &word_count);
    if (words == nullptr || word_count == 0) {
        std::cerr << "air2spirv-cli: internal error: success without a SPIR-V module\n";
        return kExitInternal;
    }
    if (!write_spirv(parsed->output_path, std::span<const std::uint32_t>(words, word_count))) {
        std::cerr << "air2spirv-cli: cannot write '" << parsed->output_path << "'\n";
        std::remove(parsed->output_path.c_str());
        return kExitIo;
    }
    return kExitOk;
}

} // namespace

int main(int argc, char** argv) {
    try {
        return run(std::span<char*>(argv, static_cast<std::size_t>(argc)));
    } catch (const std::exception& e) {
        std::cerr << "air2spirv-cli: internal error: " << e.what() << '\n';
    } catch (...) {
        std::cerr << "air2spirv-cli: internal error: unknown exception\n";
    }
    return kExitInternal;
}
