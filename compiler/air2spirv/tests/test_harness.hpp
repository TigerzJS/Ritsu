// Minimal self-contained test harness (no external dependencies, no network).
//
// Each test executable registers cases with A2S_TEST(name). The executable runs all cases, or
// only the one named on the command line (CMake registers every case as its own ctest test
// "air2spirv.<area>.<name>", see tests/CMakeLists.txt). `--list` prints the case names.
#pragma once

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <ostream>
#include <sstream>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>

namespace a2s_test {

using TestFn = void (*)();

struct TestCase {
    const char* name;
    TestFn fn;
};

std::vector<TestCase>& registry();
void report_failure(const char* file, int line, const std::string& message);

struct Registrar {
    Registrar(const char* name, TestFn fn) { registry().push_back(TestCase{name, fn}); }
};

// Master seed for randomized tests: RITSU_TEST_SEED (decimal or 0x-hex) or a fixed default.
std::uint64_t master_seed();
// Per-case seed derived from the master seed and the case name (docs/process/testing.md §4.5).
std::uint64_t case_seed(std::string_view case_name);

template <typename T>
void print_value(std::ostream& out, const T& value) {
    if constexpr (std::is_same_v<T, std::byte>) {
        out << "0x" << std::hex << std::to_integer<unsigned>(value) << std::dec;
    } else if constexpr (std::is_enum_v<T>) {
        out << static_cast<long long>(value);
    } else if constexpr (std::is_same_v<T, char> || std::is_same_v<T, signed char> ||
                         std::is_same_v<T, unsigned char>) {
        out << static_cast<int>(value);
    } else {
        out << value;
    }
}

template <typename A, typename B>
std::string describe_mismatch(const char* a_text, const char* b_text, const A& a, const B& b) {
    std::ostringstream out;
    out << a_text << " == " << b_text << " failed: ";
    print_value(out, a);
    out << " vs ";
    print_value(out, b);
    return out.str();
}

} // namespace a2s_test

#define A2S_TEST(name)                                                                             \
    static void name();                                                                            \
    static const ::a2s_test::Registrar name##_registrar(#name, &name);                             \
    static void name()

#define A2S_CHECK(cond)                                                                            \
    do {                                                                                           \
        if (!(cond)) {                                                                             \
            ::a2s_test::report_failure(__FILE__, __LINE__, "check failed: " #cond);                \
        }                                                                                          \
    } while (0)

#define A2S_CHECK_EQ(a, b)                                                                         \
    do {                                                                                           \
        const auto& a2s_check_a = (a);                                                             \
        const auto& a2s_check_b = (b);                                                             \
        if (!(a2s_check_a == a2s_check_b)) {                                                       \
            ::a2s_test::report_failure(                                                            \
                __FILE__, __LINE__,                                                                \
                ::a2s_test::describe_mismatch(#a, #b, a2s_check_a, a2s_check_b));                  \
        }                                                                                          \
    } while (0)
