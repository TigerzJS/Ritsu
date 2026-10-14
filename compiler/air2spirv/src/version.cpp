#include "version.hpp"

#include <spirv-tools/libspirv.h>
#include <string>

#ifndef A2S_VERSION
#error "A2S_VERSION must be defined by the build system"
#endif
#ifndef A2S_SPIRV_TOOLS_PACKAGE
#error "A2S_SPIRV_TOOLS_PACKAGE must be defined by the build system (may be empty)"
#endif

#if defined(A2S_HAVE_LLVM) && A2S_HAVE_LLVM
#include <llvm/Config/llvm-config.h>
#define A2S_LLVM_DESCRIPTION "llvm " LLVM_VERSION_STRING
#else
#define A2S_LLVM_DESCRIPTION "without llvm"
#endif

namespace air2spirv {
namespace {

std::string make_version_string() {
    const char* version = spvSoftwareVersionString();
    std::string text = std::string("air2spirv ") + A2S_VERSION + " (" + A2S_LLVM_DESCRIPTION +
                       ", spirv-tools " + (version != nullptr ? version : "unknown") + " [";
    // spvSoftwareVersionString() is only "v2025.1"; it does not distinguish release candidates
    // or distro patches. Add the package identity detected at configure time, or, if there is
    // none, the library's own build details (git hash / build date of a self-built library).
    constexpr const char* kPackage = A2S_SPIRV_TOOLS_PACKAGE;
    if (kPackage[0] != '\0') {
        text += kPackage;
    } else {
        const char* details = spvSoftwareVersionDetailsString();
        text += details != nullptr ? details : "no build details";
    }
    text += "])";
    return text;
}

} // namespace

const char* version_string() noexcept {
    // Built once, never truncated. An allocation failure here (a few hundred bytes, first call)
    // terminates instead of returning a wrong cache-key component.
    static const std::string text = make_version_string();
    return text.c_str();
}

} // namespace air2spirv
