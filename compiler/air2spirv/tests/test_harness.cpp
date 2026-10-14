#include "test_harness.hpp"

#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>

namespace a2s_test {
namespace {

int g_failures = 0;
constexpr std::uint64_t kDefaultSeed = 0x9E3779B97F4A7C15ULL;

std::uint64_t fnv1a(std::string_view text, std::uint64_t hash) {
    for (const char c : text) {
        hash ^= static_cast<unsigned char>(c);
        hash *= 0x100000001B3ULL;
    }
    return hash;
}

} // namespace

std::vector<TestCase>& registry() {
    static std::vector<TestCase> cases;
    return cases;
}

void report_failure(const char* file, int line, const std::string& message) {
    ++g_failures;
    std::cerr << file << ':' << line << ": " << message << '\n';
}

std::uint64_t master_seed() {
    const char* env = std::getenv("RITSU_TEST_SEED");
    if (env == nullptr || *env == '\0') {
        return kDefaultSeed;
    }
    char* end = nullptr;
    const unsigned long long value = std::strtoull(env, &end, 0);
    if (end == nullptr || *end != '\0') {
        std::cerr << "warning: ignoring unparsable RITSU_TEST_SEED='" << env << "'\n";
        return kDefaultSeed;
    }
    return static_cast<std::uint64_t>(value);
}

std::uint64_t case_seed(std::string_view case_name) {
    std::uint64_t hash = 0xCBF29CE484222325ULL;
    const std::uint64_t seed = master_seed();
    for (int i = 0; i < 8; ++i) {
        hash ^= (seed >> (8 * i)) & 0xFFU;
        hash *= 0x100000001B3ULL;
    }
    return fnv1a(case_name, hash);
}

} // namespace a2s_test

int main(int argc, char** argv) {
    using a2s_test::registry;
    if (argc == 2 && std::strcmp(argv[1], "--list") == 0) {
        for (const auto& test : registry()) {
            std::cout << test.name << '\n';
        }
        return 0;
    }
    const char* filter = argc >= 2 ? argv[1] : nullptr;
    int ran = 0;
    for (const auto& test : registry()) {
        if (filter != nullptr && std::strcmp(filter, test.name) != 0) {
            continue;
        }
        const int before = a2s_test::g_failures;
        test.fn();
        ++ran;
        if (a2s_test::g_failures != before) {
            std::cerr << "[FAIL] " << test.name << '\n';
            std::cerr << "reproduce: RITSU_TEST_SEED=0x" << std::hex << a2s_test::master_seed()
                      << std::dec << ' ' << argv[0] << ' ' << test.name << '\n';
        } else {
            std::cout << "[ OK ] " << test.name << '\n';
        }
    }
    if (ran == 0) {
        std::cerr << "no test case matches '" << (filter != nullptr ? filter : "") << "'\n";
        return 1;
    }
    return a2s_test::g_failures == 0 ? 0 : 1;
}
