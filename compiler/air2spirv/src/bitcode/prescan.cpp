#include "bitcode/prescan.hpp"

#include <algorithm>
#include <llvm/ADT/ArrayRef.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/Bitcode/LLVMBitCodes.h>
#include <llvm/Bitstream/BitCodeEnums.h>
#include <llvm/Bitstream/BitstreamReader.h>
#include <llvm/Support/Error.h>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace air2spirv::bitcode {
namespace {

static_assert(kSupportedEpoch == llvm::bitc::BITCODE_CURRENT_EPOCH,
              "the pre-scan must follow the linked LLVM");

// Same rule as llvm::getBitcodeFileContents: trailing bytes that cannot hold another block
// are padding (some tools append garbage), not an error.
constexpr std::uint64_t kMinBlockBytes = 8;
// llvm::AttributeList::FunctionIndex as stored in attribute records (unsigned, 32 bit).
constexpr std::uint32_t kFunctionAttributeIndex = 0xFFFFFFFFU;

std::string block_name(unsigned id) {
    switch (id) {
    case llvm::bitc::MODULE_BLOCK_ID:
        return "MODULE_BLOCK";
    case llvm::bitc::PARAMATTR_BLOCK_ID:
        return "PARAMATTR_BLOCK";
    case llvm::bitc::PARAMATTR_GROUP_BLOCK_ID:
        return "PARAMATTR_GROUP_BLOCK";
    case llvm::bitc::CONSTANTS_BLOCK_ID:
        return "CONSTANTS_BLOCK";
    case llvm::bitc::FUNCTION_BLOCK_ID:
        return "FUNCTION_BLOCK";
    case llvm::bitc::IDENTIFICATION_BLOCK_ID:
        return "IDENTIFICATION_BLOCK";
    case llvm::bitc::TYPE_BLOCK_ID_NEW:
        return "TYPE_BLOCK";
    case llvm::bitc::VALUE_SYMTAB_BLOCK_ID:
        return "VALUE_SYMTAB_BLOCK";
    case llvm::bitc::METADATA_ATTACHMENT_ID:
        return "METADATA_ATTACHMENT_BLOCK";
    default:
        return "block " + std::to_string(id);
    }
}

enum class TypeKind : std::uint8_t { Other, Array, Vector };

// How LLVM 18's reader treats a block at its position (see prescan.hpp, "Decoding like LLVM").
// Only blocks in a position where the reader parses them get the semantic checks; everything
// else is only walked for structure, because the reader skips it.
enum class Role : std::uint8_t {
    Generic,        // skipped by the reader (or walked without effect): structure only
    Identification, // top level, directly before a MODULE_BLOCK
    Module,         // top level
    TypeTable,      // child of MODULE_BLOCK
    ParamAttr,      // child of MODULE_BLOCK
    ParamAttrGroup, // child of MODULE_BLOCK
    Constants,      // child of MODULE_BLOCK or FUNCTION_BLOCK
    Function,       // child of MODULE_BLOCK
    Attachment,     // METADATA_ATTACHMENT, child of FUNCTION_BLOCK
    ModuleVst,      // VALUE_SYMTAB, child of MODULE_BLOCK
    FunctionVst,    // VALUE_SYMTAB, child of FUNCTION_BLOCK
};

Role role_of(unsigned id, std::optional<Role> parent) {
    namespace bitc = llvm::bitc;
    if (!parent) {
        if (id == bitc::IDENTIFICATION_BLOCK_ID) {
            return Role::Identification;
        }
        return id == bitc::MODULE_BLOCK_ID ? Role::Module : Role::Generic;
    }
    if (*parent == Role::Module) {
        switch (id) {
        case bitc::TYPE_BLOCK_ID_NEW:
            return Role::TypeTable;
        case bitc::PARAMATTR_BLOCK_ID:
            return Role::ParamAttr;
        case bitc::PARAMATTR_GROUP_BLOCK_ID:
            return Role::ParamAttrGroup;
        case bitc::CONSTANTS_BLOCK_ID:
            return Role::Constants;
        case bitc::FUNCTION_BLOCK_ID:
            return Role::Function;
        case bitc::VALUE_SYMTAB_BLOCK_ID:
            return Role::ModuleVst;
        default:
            return Role::Generic;
        }
    }
    if (*parent == Role::Function) {
        switch (id) {
        case bitc::CONSTANTS_BLOCK_ID:
            return Role::Constants;
        case bitc::METADATA_ATTACHMENT_ID:
            return Role::Attachment;
        case bitc::VALUE_SYMTAB_BLOCK_ID:
            return Role::FunctionVst;
        default:
            return Role::Generic;
        }
    }
    return Role::Generic;
}

class Prescan {
public:
    Prescan(std::span<const std::byte> bitcode, PrescanResult& out, Diagnostics& diag,
            const PrescanLimits& limits)
        : bytes_(reinterpret_cast<const std::uint8_t*>(bitcode.data()), bitcode.size()),
          cursor_(bytes_), result_(out), out_(out.identity), diag_(diag), limits_(limits),
          total_bits_(static_cast<std::uint64_t>(bitcode.size()) * 8U) {
        cursor_.setBlockInfo(&block_info_);
    }

    a2s_status run() {
        llvm::Expected<llvm::SimpleBitstreamCursor::word_t> magic = cursor_.Read(32);
        if (!magic) {
            return malformed("cannot read the bitcode magic", magic.takeError());
        }
        if (*magic != 0xDEC04342U) { // 'B' 'C' 0xC0 0xDE, read little-endian
            return malformed("missing bitcode magic");
        }
        while (!stack_.empty() || cursor_.getCurrentByteNo() + kMinBlockBytes < bytes_.size()) {
            // Abbreviation definitions are processed here (not inside advance()) so that the
            // position of a sub-block's header is known: entry_bit + abbreviation ID width.
            const std::uint64_t entry_bit = cursor_.GetCurrentBitNo();
            const unsigned id_width = cursor_.getAbbrevIDWidth();
            llvm::Expected<llvm::BitstreamEntry> entry =
                cursor_.advance(llvm::BitstreamCursor::AF_DontAutoprocessAbbrevs);
            if (!entry) {
                return malformed("broken block structure", entry.takeError());
            }
            a2s_status s = A2S_OK;
            switch (entry->Kind) {
            case llvm::BitstreamEntry::Error:
                return malformed("malformed block structure");
            case llvm::BitstreamEntry::EndBlock:
                s = on_end_block();
                break;
            case llvm::BitstreamEntry::SubBlock:
                s = on_block(entry->ID, entry_bit, entry_bit + id_width);
                break;
            case llvm::BitstreamEntry::Record:
                if (entry->ID == llvm::bitc::DEFINE_ABBREV) {
                    s = on_define_abbrev();
                } else {
                    s = on_record(entry->ID);
                }
                break;
            }
            if (s != A2S_OK) {
                return s;
            }
        }
        if (identification_start_) {
            return malformed("IDENTIFICATION_BLOCK without a MODULE_BLOCK after it");
        }
        return A2S_OK;
    }

private:
    struct Frame {
        unsigned id;
        Role role;
        std::uint64_t end_bit;                    // declared end (start of the body + length)
        std::uint64_t bits;                       // declared size of the body
        std::uint64_t instructions = 0;           // Function: instruction records so far
        TypeKind constant_type = TypeKind::Other; // Constants: the current type
    };

    // State of the MODULE_BLOCK being scanned. LLVM creates a fresh reader per module.
    struct ModuleState {
        std::uint64_t base_bit = 0; // start of the module's slice (identification or module)
        bool seen_blockinfo = false;
        bool seen_function_block = false;
        bool seen_type_table = false;
        std::vector<TypeKind> type_kinds;    // per type ID
        std::vector<bool> value_is_function; // per global value ID (globals, module constants)
        std::size_t values_at_first_function = 0;
        std::vector<std::uint64_t> function_starts;  // canonical module-child FUNCTION_BLOCKs
        std::vector<std::uint64_t> vst_starts;       // module-child VALUE_SYMTAB blocks
        std::vector<std::uint64_t> vst_offsets;      // VSTOFFSET record values
        std::vector<std::uint64_t> function_offsets; // FNENTRY offset values
    };

    std::optional<Role> parent_role() const {
        if (stack_.empty()) {
            return std::nullopt;
        }
        return stack_.back().role;
    }

    a2s_status on_define_abbrev() {
        // The reader resumes the module scan after the last function body (offsets from the
        // symbol table) and never sees module-level definitions in between. Writers put them
        // all before the first function block.
        if (!stack_.empty() && stack_.back().role == Role::Module && module_.seen_function_block) {
            diag_.error("bitcode: abbreviation definition after the first function block "
                        "(non-standard layout)");
            return A2S_ERROR_UNSUPPORTED;
        }
        // At the top level they may sit between an IDENTIFICATION_BLOCK and its MODULE_BLOCK
        // (llvm::getBitcodeFileContents processes them there).
        if (llvm::Error e = cursor_.ReadAbbrevRecord()) {
            return malformed("broken abbreviation definition", std::move(e));
        }
        return A2S_OK;
    }

    a2s_status on_block(unsigned id, std::uint64_t entry_bit, std::uint64_t id_bit) {
        const std::optional<Role> parent = parent_role();
        // Top level as llvm::getBitcodeFileContents sees it: an IDENTIFICATION_BLOCK must be
        // followed by a MODULE_BLOCK.
        if (!parent && identification_start_ && id != llvm::bitc::MODULE_BLOCK_ID) {
            return malformed("IDENTIFICATION_BLOCK not followed by a MODULE_BLOCK");
        }
        if (id == llvm::bitc::BLOCKINFO_BLOCK_ID) {
            return on_blockinfo(parent);
        }
        if (id == llvm::bitc::USELIST_BLOCK_ID &&
            (parent == Role::Module || parent == Role::Function)) {
            if (single_chunk_vbr8_at(id_bit) != static_cast<int>(id)) {
                return malformed("USELIST_BLOCK with a non-canonical block ID");
            }
            result_.neutralize_block_id_bits.push_back(id_bit);
        }
        const Role role = role_of(id, parent);
        if (stack_.size() >= limits_.max_block_depth) {
            return malformed("blocks nested more than " + std::to_string(limits_.max_block_depth) +
                             " levels deep");
        }
        switch (role) {
        case Role::Module:
            // A fresh reader per module: no BLOCKINFO, no types, no values yet.
            module_ = ModuleState{};
            module_.base_bit = identification_start_.value_or(entry_bit);
            identification_start_.reset();
            block_info_ = llvm::BitstreamBlockInfo{};
            break;
        case Role::Identification:
            identification_start_ = entry_bit;
            break;
        case Role::TypeTable:
            if (module_.seen_type_table) {
                return malformed("more than one TYPE_BLOCK in a module");
            }
            module_.seen_type_table = true;
            break;
        case Role::Function:
            if (!module_.seen_function_block) {
                module_.seen_function_block = true;
                module_.values_at_first_function = module_.value_is_function.size();
            }
            // Function bodies are also entered by jumping to entry_bit + abbreviation width +
            // 8 (the symbol table's offsets); that lands on the block header only if the
            // block ID uses a single VBR8 chunk.
            if (single_chunk_vbr8_at(id_bit) == static_cast<int>(id)) {
                module_.function_starts.push_back(entry_bit);
            }
            break;
        case Role::ModuleVst:
            module_.vst_starts.push_back(entry_bit);
            break;
        default:
            break;
        }
        unsigned num_words = 0;
        if (llvm::Error e = cursor_.EnterSubBlock(id, &num_words)) {
            return malformed("cannot enter " + block_name(id), std::move(e));
        }
        const std::uint64_t bits = static_cast<std::uint64_t>(num_words) * 32U;
        stack_.push_back(
            Frame{id, role, cursor_.GetCurrentBitNo() + bits, bits, 0, TypeKind::Other});
        return A2S_OK;
    }

    // LLVM 18 reads BLOCKINFO only as a direct child of the MODULE_BLOCK (and replaces the
    // previous one); everywhere else it skips it. One BLOCKINFO before the first function
    // block is the writer's layout and the only one accepted, so that every block of the
    // module is decoded with the same abbreviations here and in the reader, including function
    // bodies and the symbol table that the reader reaches by jumping.
    a2s_status on_blockinfo(std::optional<Role> parent) {
        if (parent != Role::Module) {
            if (llvm::Error e = cursor_.SkipBlock()) {
                return malformed("cannot skip BLOCKINFO block", std::move(e));
            }
            return A2S_OK;
        }
        if (module_.seen_blockinfo || module_.seen_function_block) {
            diag_.error("bitcode: BLOCKINFO block at a non-standard position (second one, or "
                        "after the first function block)");
            return A2S_ERROR_UNSUPPORTED;
        }
        module_.seen_blockinfo = true;
        llvm::Expected<std::optional<llvm::BitstreamBlockInfo>> info = cursor_.ReadBlockInfoBlock();
        if (!info) {
            return malformed("broken BLOCKINFO block", info.takeError());
        }
        if (!*info) {
            return malformed("incomplete BLOCKINFO block");
        }
        block_info_ = std::move(**info);
        return A2S_OK;
    }

    a2s_status on_end_block() {
        if (stack_.empty()) {
            return malformed("END_BLOCK outside of a block");
        }
        const Frame frame = stack_.back();
        stack_.pop_back();
        // Blocks the reader skips are skipped by their declared length, blocks it parses end
        // at END_BLOCK. Both must be the same place, or the two would continue differently.
        if (cursor_.GetCurrentBitNo() != frame.end_bit) {
            return malformed(block_name(frame.id) + " length does not match its content");
        }
        if (frame.role == Role::Module) {
            const a2s_status s = check_module_offsets();
            block_info_ = llvm::BitstreamBlockInfo{}; // top-level blocks see no BLOCKINFO
            return s;
        }
        return A2S_OK;
    }

    // Offsets in VSTOFFSET and FNENTRY are 32-bit words relative to one word before the
    // module's slice. The reader jumps there without further checks; they must name a
    // symbol table / function block of this module that the scan has inspected.
    a2s_status check_module_offsets() {
        auto absolute = [this](std::uint64_t value, std::uint64_t& bit) {
            if (value == 0 || value - 1 > (total_bits_ - module_.base_bit) / 32U) {
                return false;
            }
            bit = module_.base_bit + (value - 1) * 32U;
            return true;
        };
        std::sort(module_.vst_starts.begin(), module_.vst_starts.end());
        std::sort(module_.function_starts.begin(), module_.function_starts.end());
        for (const std::uint64_t value : module_.vst_offsets) {
            std::uint64_t bit = 0;
            if (!absolute(value, bit) ||
                !std::binary_search(module_.vst_starts.begin(), module_.vst_starts.end(), bit)) {
                return malformed("VSTOFFSET does not point at a symbol table of the module");
            }
        }
        for (const std::uint64_t value : module_.function_offsets) {
            std::uint64_t bit = 0;
            if (!absolute(value, bit) || !std::binary_search(module_.function_starts.begin(),
                                                             module_.function_starts.end(), bit)) {
                return malformed("function offset in the symbol table does not point at a "
                                 "function block of the module");
            }
        }
        return A2S_OK;
    }

    a2s_status on_record(unsigned abbrev_id) {
        if (stack_.empty()) { // records outside of blocks: LLVM skips them as well
            if (identification_start_) {
                return malformed("IDENTIFICATION_BLOCK not followed by a MODULE_BLOCK");
            }
            if (llvm::Expected<unsigned> code = cursor_.skipRecord(abbrev_id); !code) {
                return malformed("broken top-level record", code.takeError());
            }
            return A2S_OK;
        }
        Frame& frame = stack_.back();
        if (frame.role == Role::Generic) {
            if (llvm::Expected<unsigned> code = cursor_.skipRecord(abbrev_id); !code) {
                return malformed("broken record in " + block_name(frame.id), code.takeError());
            }
            return A2S_OK;
        }
        record_.clear();
        llvm::Expected<unsigned> code = cursor_.readRecord(abbrev_id, record_);
        if (!code) {
            return malformed("broken record in " + block_name(frame.id), code.takeError());
        }
        switch (frame.role) {
        case Role::Identification:
            return on_identification_record(*code);
        case Role::Module:
            return on_module_record(*code);
        case Role::TypeTable:
            return on_type_record(*code, frame);
        case Role::Function:
            switch (*code) {
            case llvm::bitc::FUNC_CODE_DECLAREBLOCKS:
                if (record_.empty()) {
                    return A2S_OK;
                }
                return check_count("basic block count (FUNCTION_BLOCK DECLAREBLOCKS)", record_[0],
                                   frame.bits, limits_.max_basic_blocks);
            case llvm::bitc::FUNC_CODE_DEBUG_LOC:
            case llvm::bitc::FUNC_CODE_DEBUG_LOC_AGAIN:
            case llvm::bitc::FUNC_CODE_OPERAND_BUNDLE:
            case llvm::bitc::FUNC_CODE_BLOCKADDR_USERS:
                return A2S_OK;
            default: // every other record creates (at least) one instruction or fails
                ++frame.instructions;
                return A2S_OK;
            }
        case Role::Attachment:
            return on_attachment_record(*code);
        case Role::Constants:
            return on_constant_record(*code, frame);
        case Role::ParamAttr:
            if (*code == llvm::bitc::PARAMATTR_CODE_ENTRY_OLD) {
                for (std::size_t i = 0; i < record_.size(); i += 2) {
                    if (const a2s_status s = check_attribute_index(record_[i]); s != A2S_OK) {
                        return s;
                    }
                }
            }
            return A2S_OK;
        case Role::ParamAttrGroup:
            if (*code == llvm::bitc::PARAMATTR_GRP_CODE_ENTRY && record_.size() >= 2) {
                if (const a2s_status s = check_attribute_index(record_[1]); s != A2S_OK) {
                    return s;
                }
                return check_attribute_group_layout();
            }
            return A2S_OK;
        case Role::ModuleVst:
            return on_module_vst_record(*code);
        case Role::FunctionVst:
            if (*code == llvm::bitc::VST_CODE_FNENTRY) {
                // The reader would take it, with an offset delta computed from the function
                // block's abbreviation width instead of the module's.
                return malformed("function offset in a function-level symbol table");
            }
            return A2S_OK;
        case Role::Generic:
            break;
        }
        return A2S_OK;
    }

    a2s_status on_type_record(unsigned code, const Frame& frame) {
        // Every record except NUMENTRY and STRUCT_NAME defines the next type ID.
        if (code != llvm::bitc::TYPE_CODE_NUMENTRY && code != llvm::bitc::TYPE_CODE_STRUCT_NAME) {
            module_.type_kinds.push_back(code == llvm::bitc::TYPE_CODE_ARRAY    ? TypeKind::Array
                                         : code == llvm::bitc::TYPE_CODE_VECTOR ? TypeKind::Vector
                                                                                : TypeKind::Other);
        }
        if (record_.empty()) {
            return A2S_OK; // LLVM reports "Invalid record"
        }
        switch (code) {
        case llvm::bitc::TYPE_CODE_NUMENTRY:
            return check_count("type table size (TYPE_BLOCK NUMENTRY)", record_[0], frame.bits,
                               limits_.max_types);
        case llvm::bitc::TYPE_CODE_VECTOR:
            return check_limit("vector element count", record_[0], limits_.max_vector_elements);
        case llvm::bitc::TYPE_CODE_INTEGER:
            return check_limit("integer width", record_[0], limits_.max_integer_bits);
        default:
            return A2S_OK;
        }
    }

    // FNENTRY: [valueid, offset, ...]. The reader casts the value to llvm::Function without a
    // check (string-table modules) and jumps to the offset; both are verified here. Values are
    // counted as the reader has them when it reads the table (at the first function block).
    a2s_status on_module_vst_record(unsigned code) {
        if (code != llvm::bitc::VST_CODE_FNENTRY || record_.size() < 2) {
            return A2S_OK;
        }
        const std::size_t known = module_.seen_function_block ? module_.values_at_first_function
                                                              : module_.value_is_function.size();
        if (record_[0] >= known ||
            !module_.value_is_function[static_cast<std::size_t>(record_[0])]) {
            return malformed("symbol table function entry for value " + std::to_string(record_[0]) +
                             ", which is not a function");
        }
        module_.function_offsets.push_back(record_[1]);
        return A2S_OK;
    }

    // METADATA_ATTACHMENT inside a FUNCTION_BLOCK: [instruction, (kind, node)...] (odd length).
    // LLVM 18 indexes its instruction list with the first operand unchecked. At that point the
    // list holds at least one entry per instruction record seen, so a smaller index is safe.
    // (Only ancient bitcode with upgraded address-space bitcasts has more entries; an
    // attachment beyond the record count there is rejected as well.)
    a2s_status on_attachment_record(unsigned code) {
        // Role::Attachment implies a FUNCTION_BLOCK parent.
        if (code != llvm::bitc::METADATA_ATTACHMENT || record_.size() % 2 == 0) {
            return A2S_OK;
        }
        const std::uint64_t instructions = stack_[stack_.size() - 2].instructions;
        if (record_[0] >= instructions) {
            return malformed("metadata attachment for instruction " + std::to_string(record_[0]) +
                             " of a function with " + std::to_string(instructions) +
                             " instructions so far");
        }
        return A2S_OK;
    }

    a2s_status on_constant_record(unsigned code, Frame& frame) {
        // Every record except SETTYPE defines one value; module-level ones are global values.
        if (code != llvm::bitc::CST_CODE_SETTYPE && stack_.size() >= 2 &&
            stack_[stack_.size() - 2].role == Role::Module) {
            module_.value_is_function.push_back(false);
        }
        switch (code) {
        case llvm::bitc::CST_CODE_SETTYPE:
            if (!record_.empty()) {
                if (record_[0] >= module_.type_kinds.size()) {
                    return malformed("constant type " + std::to_string(record_[0]) +
                                     " outside the type table");
                }
                frame.constant_type = module_.type_kinds[static_cast<std::size_t>(record_[0])];
            }
            return A2S_OK;
        case llvm::bitc::CST_CODE_DATA:
            // LLVM 18 casts the current type to a vector type without checking it.
            if (frame.constant_type != TypeKind::Array && frame.constant_type != TypeKind::Vector) {
                return malformed("DATA constant of a type that is neither array nor vector");
            }
            return A2S_OK;
        case llvm::bitc::CST_CODE_BLOCKADDRESS:
            if (record_.size() >= 3) {
                // The reader truncates the block number to 32 bits.
                return check_count("basic block number (CONSTANTS_BLOCK BLOCKADDRESS)",
                                   record_[2] & 0xFFFFFFFFU, total_bits_, limits_.max_basic_blocks);
            }
            return A2S_OK;
        case llvm::bitc::CST_CODE_INLINEASM_OLD:
        case llvm::bitc::CST_CODE_INLINEASM_OLD2:
        case llvm::bitc::CST_CODE_INLINEASM_OLD3:
        case llvm::bitc::CST_CODE_INLINEASM:
            // Metal has no inline assembly (and LLVM 18 casts old-style operands unchecked).
            diag_.error("bitcode: inline assembly is not supported");
            return A2S_ERROR_UNSUPPORTED;
        default:
            return A2S_OK;
        }
    }

    a2s_status on_identification_record(unsigned code) {
        switch (code) {
        case llvm::bitc::IDENTIFICATION_CODE_STRING:
            if (!out_.producer) {
                std::string text;
                for (const std::uint64_t c : record_) {
                    if (text.size() >= kMaxProducerLength) {
                        text += "...";
                        break;
                    }
                    text.push_back(static_cast<char>(static_cast<unsigned char>(c & 0xFFU)));
                }
                out_.producer = std::move(text);
            }
            return A2S_OK;
        case llvm::bitc::IDENTIFICATION_CODE_EPOCH:
            if (record_.empty()) {
                return malformed("EPOCH record without value");
            }
            if (!out_.epoch) {
                out_.epoch = record_[0];
            }
            if (record_[0] != kSupportedEpoch) {
                diag_.error("bitcode: epoch " + std::to_string(record_[0]) +
                            " is not supported (the linked LLVM reads epoch " +
                            std::to_string(kSupportedEpoch) + ")");
                return A2S_ERROR_UNSUPPORTED;
            }
            return A2S_OK;
        default:
            diag_.error("bitcode: unknown record code " + std::to_string(code) +
                        " in IDENTIFICATION_BLOCK (newer than the linked LLVM?)");
            return A2S_ERROR_UNSUPPORTED;
        }
    }

    a2s_status on_module_record(unsigned code) {
        switch (code) {
        case llvm::bitc::MODULE_CODE_GLOBALVAR:
        case llvm::bitc::MODULE_CODE_ALIAS_OLD:
        case llvm::bitc::MODULE_CODE_ALIAS:
        case llvm::bitc::MODULE_CODE_IFUNC:
            module_.value_is_function.push_back(false); // each defines one global value
            return A2S_OK;
        case llvm::bitc::MODULE_CODE_FUNCTION:
            module_.value_is_function.push_back(true);
            return A2S_OK;
        case llvm::bitc::MODULE_CODE_VSTOFFSET:
            if (record_.empty() || record_[0] == 0) {
                return malformed("invalid VSTOFFSET record");
            }
            module_.vst_offsets.push_back(record_[0]);
            return A2S_OK;
        case llvm::bitc::MODULE_CODE_VERSION:
            break;
        default:
            return A2S_OK;
        }
        if (record_.empty()) {
            return malformed("VERSION record without value");
        }
        if (!out_.module_version) {
            out_.module_version = record_[0];
        }
        if (record_[0] > kMaxModuleVersion) {
            diag_.error("bitcode: module version " + std::to_string(record_[0]) +
                        " is newer than the linked LLVM supports (at most " +
                        std::to_string(kMaxModuleVersion) + ")");
            return A2S_ERROR_UNSUPPORTED;
        }
        return A2S_OK;
    }

    // `value` sizes a container in the reader. More than `possible` (bits the stream has to
    // encode the elements) cannot be valid; more than `limit` is refused as a resource limit.
    a2s_status check_count(const char* what, std::uint64_t value, std::uint64_t possible,
                           std::uint64_t limit) {
        if (value > possible) {
            return malformed(std::string(what) + " " + std::to_string(value) +
                             " exceeds what the bitstream can encode");
        }
        if (value > limit) {
            diag_.error("bitcode: " + std::string(what) + " " + std::to_string(value) +
                        " exceeds the limit of " + std::to_string(limit));
            return A2S_ERROR_UNSUPPORTED;
        }
        return A2S_OK;
    }

    // ENTRY: [grpid, paramidx, (kind, payload)...]. LLVM 18 walks the payloads with unchecked
    // Record[++i] reads (and an unterminated string loop), so a record whose last attribute is
    // cut short makes it read past the record. Mirrors that loop and requires every payload to
    // be complete. Unknown kinds are left to LLVM, which rejects them.
    a2s_status check_attribute_group_layout() {
        const std::size_t n = record_.size();
        std::size_t i = 2;
        while (i < n) {
            std::size_t operands = 0; // operands after the kind
            switch (record_[i]) {
            case 0: // enum attribute: [0, attr]
            case 5: // type attribute without type: [5, attr]
                operands = 1;
                break;
            case 1: // integer attribute: [1, attr, value]
            case 6: // type attribute: [6, attr, type]
                operands = 2;
                break;
            case 3:   // string attribute: [3, key..., 0]
            case 4: { // string attribute with value: [4, key..., 0, value..., 0]
                std::size_t strings = record_[i] == 4 ? 2 : 1;
                std::size_t j = i + 1;
                while (strings > 0) {
                    while (j < n && record_[j] != 0) {
                        ++j;
                    }
                    if (j >= n) {
                        return malformed("unterminated string in attribute group record");
                    }
                    ++j; // the terminating 0
                    --strings;
                }
                i = j;
                continue;
            }
            default:
                return A2S_OK; // LLVM reports "Invalid attribute group entry"
            }
            if (n - i <= operands) {
                return malformed("truncated attribute in attribute group record");
            }
            i += operands + 1;
        }
        return A2S_OK;
    }

    a2s_status check_limit(const char* what, std::uint64_t value, std::uint64_t limit) {
        if (value > limit) {
            diag_.error("bitcode: " + std::string(what) + " " + std::to_string(value) +
                        " exceeds the limit of " + std::to_string(limit));
            return A2S_ERROR_UNSUPPORTED;
        }
        return A2S_OK;
    }

    // The 8-bit chunk at `bit`, or -1 if it is out of range or has its continuation bit set.
    int single_chunk_vbr8_at(std::uint64_t bit) const {
        if (bit + 8 > total_bits_) {
            return -1;
        }
        unsigned value = 0;
        for (unsigned i = 0; i < 8; ++i) {
            const std::uint64_t pos = bit + i;
            value |= ((static_cast<unsigned>(bytes_[pos / 8]) >> (pos % 8)) & 1U) << i;
        }
        return (value & 0x80U) != 0 ? -1 : static_cast<int>(value);
    }

    a2s_status check_attribute_index(std::uint64_t raw) {
        const auto index = static_cast<std::uint32_t>(raw & 0xFFFFFFFFU); // reader truncates
        if (index == kFunctionAttributeIndex) {
            return A2S_OK;
        }
        return check_count("attribute parameter index", index, total_bits_,
                           limits_.max_attribute_index);
    }

    a2s_status malformed(const std::string& what) {
        diag_.error("bitcode: " + what);
        return A2S_ERROR_MALFORMED_INPUT;
    }

    a2s_status malformed(const std::string& what, llvm::Error error) {
        diag_.error("bitcode: " + what + ": " +
                    escape_untrusted(llvm::toString(std::move(error)), 256));
        return A2S_ERROR_MALFORMED_INPUT;
    }

    llvm::ArrayRef<std::uint8_t> bytes_;
    llvm::BitstreamBlockInfo block_info_;
    llvm::BitstreamCursor cursor_;
    PrescanResult& result_;
    BitcodeIdentity& out_;
    Diagnostics& diag_;
    const PrescanLimits& limits_;
    const std::uint64_t total_bits_;
    std::vector<Frame> stack_;
    llvm::SmallVector<std::uint64_t, 64> record_;
    ModuleState module_;
    std::optional<std::uint64_t> identification_start_; // pending, until its MODULE_BLOCK
};

} // namespace

a2s_status prescan_bitstream(std::span<const std::byte> bitcode, PrescanResult& out,
                             Diagnostics& diag, const PrescanLimits& limits) {
    out = PrescanResult{};
    Prescan scan(bitcode, out, diag, limits);
    return scan.run();
}

void neutralize_blocks(std::span<std::byte> bytes, const PrescanResult& result) {
    for (const std::uint64_t bit : result.neutralize_block_id_bits) {
        for (unsigned i = 0; i < 7; ++i) { // payload bits; the continuation bit stays 0
            const std::uint64_t pos = bit + i;
            if (pos / 8 >= bytes.size()) {
                return; // cannot happen for offsets from prescan_bitstream on the same bytes
            }
            const auto mask = static_cast<std::byte>(1U << (pos % 8));
            if (((kNeutralBlockId >> i) & 1U) != 0) {
                bytes[pos / 8] |= mask;
            } else {
                bytes[pos / 8] &= ~mask;
            }
        }
    }
}

} // namespace air2spirv::bitcode
