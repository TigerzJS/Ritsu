#include "bitcode/bitcode_module.hpp"

#include "bitcode/bitcode_module_impl.hpp"
#include "bitcode/llvm_runtime.hpp"
#include "bitcode/prescan.hpp"

#include <llvm/ADT/StringRef.h>
#include <llvm/Bitcode/BitcodeReader.h>
#include <llvm/Config/llvm-config.h>
#include <llvm/IR/DebugInfo.h>
#include <llvm/IR/DebugInfoMetadata.h>
#include <llvm/IR/DiagnosticHandler.h>
#include <llvm/IR/DiagnosticInfo.h>
#include <llvm/IR/DiagnosticPrinter.h>
#include <llvm/IR/Verifier.h>
#include <llvm/Support/Error.h>
#include <llvm/Support/MemoryBufferRef.h>
#include <llvm/Support/raw_ostream.h>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace air2spirv::bitcode {

ParsedModule::ParsedModule() = default;
ParsedModule::~ParsedModule() = default;
ParsedModule::ParsedModule(ParsedModule&&) noexcept = default;
ParsedModule& ParsedModule::operator=(ParsedModule&&) noexcept = default;

bool ParsedModule::has_module() const noexcept {
    return impl_ != nullptr && impl_->module != nullptr;
}

const char* llvm_version() noexcept {
    return LLVM_VERSION_STRING;
}

namespace {

// Limits for LLVM text that ends up in diagnostics (it may quote guest names and IR).
constexpr std::size_t kMaxMessageLength = 512;
constexpr std::size_t kMaxVerifierLines = 16;

// Collects everything the LLVMContext reports while reading. Without a handler that claims
// the diagnostic, LLVMContext::diagnose prints to stderr and calls exit(1) for errors.
class CollectingDiagnosticHandler final : public llvm::DiagnosticHandler {
public:
    struct Entry {
        llvm::DiagnosticSeverity severity;
        std::string text;
    };

    // The sink is shared because the context (and with it this handler) outlives the parse.
    explicit CollectingDiagnosticHandler(std::shared_ptr<std::vector<Entry>> sink)
        : sink_(std::move(sink)) {}

    bool handleDiagnostics(const llvm::DiagnosticInfo& info) override {
        std::string text;
        llvm::raw_string_ostream os(text);
        llvm::DiagnosticPrinterRawOStream printer(os);
        info.print(printer);
        os.flush();
        if (sink_->size() < kMaxVerifierLines) {
            sink_->push_back(Entry{info.getSeverity(), std::move(text)});
        } else if (info.getSeverity() == llvm::DS_Error) {
            sink_->back() = Entry{llvm::DS_Error, std::move(text)}; // never lose an error
        }
        return true; // handled: never fall through to LLVM's print-and-exit default
    }

private:
    std::shared_ptr<std::vector<Entry>> sink_;
};

// Classification of llvm::Error texts from the pinned LLVM's BitcodeReader. Everything the
// reader reports is a BitcodeError::CorruptedBitcode, so only the text separates "broken" from
// "newer than us". Unknown texts are MALFORMED (never OK). Checked by the test suite against
// fixtures (tests/fixtures/bitcode/unsupported_*.bc), so a reader change shows up there.
struct UnsupportedPattern {
    std::string_view prefix;
    std::string_view explanation;
};
constexpr UnsupportedPattern kUnsupportedPatterns[] = {
    {"Unknown attribute kind", "attribute kind unknown to the linked LLVM"},
    {"Incompatible epoch", "bitcode epoch unknown to the linked LLVM"},
    {"Invalid record: Unsupported version of DISubrange", "debug info encoding too new"},
    {"Invalid summary version", "module summary version unknown to the linked LLVM"},
};

a2s_status classify_reader_error(llvm::Error error, const BitcodeIdentity& identity,
                                 Diagnostics& diag) {
    const std::string text = llvm::toString(std::move(error));
    for (const UnsupportedPattern& pattern : kUnsupportedPatterns) {
        if (std::string_view(text).starts_with(pattern.prefix)) {
            diag.error("bitcode: " + std::string(pattern.explanation) + " (LLVM " +
                       LLVM_VERSION_STRING + "): " + escape_untrusted(text, kMaxMessageLength));
            return A2S_ERROR_UNSUPPORTED;
        }
    }
    diag.error("bitcode: LLVM " LLVM_VERSION_STRING " cannot read the module: " +
               escape_untrusted(text, kMaxMessageLength));
    if (identity.producer) {
        diag.note("bitcode producer: '" + escape_untrusted(*identity.producer) +
                  "'; a producer newer than LLVM " LLVM_VERSION_STRING
                  " can also cause this (S-AIR-1, WP-SC-07)");
    }
    return A2S_ERROR_MALFORMED_INPUT;
}

void report_verifier_output(const std::string& output, Diagnostics& diag, bool as_error) {
    std::size_t lines = 0;
    std::size_t begin = 0;
    while (begin < output.size()) {
        std::size_t end = output.find('\n', begin);
        if (end == std::string::npos) {
            end = output.size();
        }
        if (end > begin) {
            if (lines == kMaxVerifierLines) {
                diag.note("verifier: (further output omitted)");
                return;
            }
            const std::string line =
                "verifier: " + escape_untrusted(std::string_view(output).substr(begin, end - begin),
                                                kMaxMessageLength);
            if (as_error) {
                diag.error(line);
            } else {
                diag.note(line);
            }
            ++lines;
        }
        begin = end + 1;
    }
}

ModuleStats collect_stats(const llvm::Module& module) {
    ModuleStats stats;
    for (const llvm::Function& function : module) {
        ++stats.function_count;
        if (!function.isDeclaration()) {
            ++stats.defined_function_count;
            stats.instruction_count += function.getInstructionCount();
        }
    }
    stats.global_variable_count = module.global_size();
    return stats;
}

const char* plural(std::size_t n, const char* one, const char* many) {
    return n == 1 ? one : many;
}

} // namespace

std::string describe(const ModuleStats& stats) {
    std::string text = "parsed " + std::to_string(stats.function_count) + " " +
                       plural(stats.function_count, "function", "functions") + " (" +
                       std::to_string(stats.defined_function_count) + " defined, " +
                       std::to_string(stats.function_count - stats.defined_function_count) +
                       " declared), " + std::to_string(stats.global_variable_count) + " " +
                       plural(stats.global_variable_count, "global variable", "global variables") +
                       ", " + std::to_string(stats.instruction_count) + " " +
                       plural(stats.instruction_count, "instruction", "instructions");
    return text;
}

a2s_status parse_bitcode_module(std::span<const std::byte> bitcode, ParsedModule& out,
                                Diagnostics& diag) {
    out = ParsedModule{};

    std::string runtime_error;
    if (!prepare_llvm_runtime(runtime_error)) {
        diag.error("bitcode: LLVM runtime not usable: " + runtime_error);
        return A2S_ERROR_INTERNAL;
    }

    // 1. Pre-scan (identification, size operands, record layouts LLVM overreads), before
    // LLVM's reader sees anything. Blocks it lists are neutralised in a private copy.
    PrescanResult prescan;
    if (const a2s_status s = prescan_bitstream(bitcode, prescan, diag); s != A2S_OK) {
        out.identity_ = std::move(prescan.identity);
        return s;
    }
    out.identity_ = prescan.identity;
    std::vector<std::byte> patched;
    if (!prescan.neutralize_block_id_bits.empty()) {
        patched.assign(bitcode.begin(), bitcode.end());
        neutralize_blocks(patched, prescan);
        bitcode = patched;
        diag.note("bitcode: " + std::to_string(prescan.neutralize_block_id_bits.size()) +
                  " use-list order block(s) ignored");
    }

    // 2. Parse. The reader materializes the whole module (no lazy loading: lazy metadata
    // loading has report_fatal_error paths in LLVM 18's MetadataLoader).
    auto impl = std::make_unique<ParsedModule::Impl>();
    impl->context = std::make_unique<llvm::LLVMContext>();
    auto llvm_diagnostics = std::make_shared<std::vector<CollectingDiagnosticHandler::Entry>>();
    impl->context->setDiagnosticHandler(
        std::make_unique<CollectingDiagnosticHandler>(llvm_diagnostics));

    const llvm::MemoryBufferRef buffer(
        llvm::StringRef(reinterpret_cast<const char*>(bitcode.data()), bitcode.size()),
        "<air2spirv input>");
    llvm::Expected<std::vector<llvm::BitcodeModule>> modules = llvm::getBitcodeModuleList(buffer);
    if (!modules) {
        return classify_reader_error(modules.takeError(), out.identity_, diag);
    }
    if (modules->size() != 1) {
        diag.error("bitcode: the stream contains " + std::to_string(modules->size()) +
                   " modules; exactly one is supported");
        return modules->empty() ? A2S_ERROR_MALFORMED_INPUT : A2S_ERROR_UNSUPPORTED;
    }
    llvm::Expected<std::unique_ptr<llvm::Module>> module =
        modules->front().parseModule(*impl->context);
    if (!module) {
        return classify_reader_error(module.takeError(), out.identity_, diag);
    }
    impl->module = std::move(*module);

    bool llvm_reported_error = false;
    for (const auto& entry : *llvm_diagnostics) {
        const std::string text = "llvm: " + escape_untrusted(entry.text, kMaxMessageLength);
        if (entry.severity == llvm::DS_Error) {
            diag.error(text);
            llvm_reported_error = true;
        } else {
            diag.warning(text);
        }
    }
    if (llvm_reported_error) {
        return A2S_ERROR_MALFORMED_INPUT;
    }

    // 3. Verify, always: the reader does not check everything, and the input is hostile.
    llvm::Module& m = *impl->module;
    std::string verifier_output;
    llvm::raw_string_ostream verifier_stream(verifier_output);
    bool broken_debug_info = false;
    const bool broken = llvm::verifyModule(m, &verifier_stream, &broken_debug_info);
    verifier_stream.flush();
    if (broken) {
        diag.error("bitcode: the module does not pass the LLVM verifier");
        report_verifier_output(verifier_output, diag, true);
        return A2S_ERROR_MALFORMED_INPUT;
    }

    // 4. Debug info: what llvm::UpgradeDebugInfo would do (it is disabled, llvm_runtime.hpp),
    // without its report_fatal_error. Debug info has no influence on the translation.
    bool dropped_debug_info = false;
    const unsigned debug_version = llvm::getDebugMetadataVersionFromModule(m);
    if (debug_version != llvm::DEBUG_METADATA_VERSION) {
        if (llvm::StripDebugInfo(m)) {
            diag.warning("bitcode: debug info version " + std::to_string(debug_version) +
                         " is not supported by LLVM " LLVM_VERSION_STRING "; debug info dropped");
            dropped_debug_info = true;
        }
    } else if (broken_debug_info) {
        diag.warning("bitcode: invalid debug info dropped");
        report_verifier_output(verifier_output, diag, false);
        llvm::StripDebugInfo(m);
        dropped_debug_info = true;
    }

    out.stats_ = collect_stats(m);
    out.stats_.debug_info_dropped = dropped_debug_info;
    out.impl_ = std::move(impl);
    return A2S_OK;
}

} // namespace air2spirv::bitcode
