// End-to-end tests of air2spirv-cli exit codes. POSIX only (uses std::system + wait status).
#include "test_harness.hpp"
#include "test_inputs.hpp"

#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <string>
#include <sys/wait.h>
#include <vector>

#ifndef A2S_CLI_PATH
#error "A2S_CLI_PATH must point to the air2spirv-cli executable"
#endif
#ifndef A2S_TEST_TMPDIR
#error "A2S_TEST_TMPDIR must name a scratch directory for this test"
#endif

namespace fs = std::filesystem;

namespace {

fs::path scratch(const std::string& case_name) {
    const fs::path dir = fs::path(A2S_TEST_TMPDIR) / case_name;
    fs::remove_all(dir);
    fs::create_directories(dir);
    return dir;
}

fs::path write_file(const fs::path& path, const std::vector<std::uint8_t>& bytes) {
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    out.write(reinterpret_cast<const char*>(bytes.data()),
              static_cast<std::streamsize>(bytes.size()));
    return path;
}

std::string quote(const std::string& s) {
    std::string out = "'";
    for (const char c : s) {
        if (c == '\'') {
            out += "'\\''";
        } else {
            out += c;
        }
    }
    return out + "'";
}

// Runs the CLI with `args` (already quoted), output silenced; returns the exit code or -1.
int run_cli(const std::string& args) {
    const std::string command = quote(A2S_CLI_PATH) + " " + args + " >/dev/null 2>&1";
    const int status = std::system(command.c_str());
    if (status == -1 || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status);
}

} // namespace

A2S_TEST(help_and_version_succeed) {
    A2S_CHECK_EQ(run_cli("--help"), 0);
    A2S_CHECK_EQ(run_cli("-h"), 0);
    A2S_CHECK_EQ(run_cli("--version"), 0);
}

A2S_TEST(usage_errors_exit_2) {
    const fs::path dir = scratch("usage_errors_exit_2");
    const fs::path in = write_file(dir / "in.bc", a2s_test::make_raw_bitcode());
    A2S_CHECK_EQ(run_cli(""), 2);
    A2S_CHECK_EQ(run_cli(quote(in.string())), 2);              // missing -o
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " -o"), 2);      // -o without value
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " --entry"), 2); // --entry without value
    A2S_CHECK_EQ(run_cli("--bogus " + quote(in.string()) + " -o x.spv"), 2);
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " " + quote(in.string()) + " -o x.spv"), 2);
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " -o x.spv --entry ''"), 2); // empty entry point
}

A2S_TEST(missing_input_exits_3) {
    const fs::path dir = scratch("missing_input_exits_3");
    A2S_CHECK_EQ(
        run_cli(quote((dir / "nope.air").string()) + " -o " + quote((dir / "out.spv").string())),
        3);
}

A2S_TEST(malformed_input_exits_1) {
    const fs::path dir = scratch("malformed_input_exits_1");
    const fs::path in = write_file(dir / "empty.bc", {});
    const fs::path out = dir / "out.spv";
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " -o " + quote(out.string())), 1);
    A2S_CHECK(!fs::exists(out));
}

A2S_TEST(unknown_format_exits_4) {
    const fs::path dir = scratch("unknown_format_exits_4");
    const fs::path in = write_file(dir / "text.bin", {'h', 'e', 'l', 'l', 'o', '!', '!', '!'});
    const fs::path out = dir / "out.spv";
    A2S_CHECK_EQ(run_cli(quote(in.string()) + " -o " + quote(out.string())), 4);
    A2S_CHECK(!fs::exists(out));
}

A2S_TEST(bitcode_exits_5_until_translation_exists) {
    const fs::path dir = scratch("bitcode_exits_5_until_translation_exists");
    const fs::path raw = write_file(dir / "raw.bc", a2s_test::make_raw_bitcode(32));
    const fs::path wrapped = write_file(dir / "wrapped.bc", a2s_test::make_valid_wrapper());
    const fs::path out = dir / "out.spv";
    A2S_CHECK_EQ(run_cli(quote(raw.string()) + " -o " + quote(out.string())), 5);
    A2S_CHECK_EQ(run_cli(quote(wrapped.string()) + " -o " + quote(out.string()) + " --entry main0"),
                 5);
    A2S_CHECK(!fs::exists(out));
}
