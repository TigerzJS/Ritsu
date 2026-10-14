// Bitstream pre-scan (docs/subsystems/shader-compiler.md §2.3, §11.4).
//
// Before LLVM's BitcodeReader sees the input, one walk with llvm::BitstreamCursor over the
// block structure:
//
//   1. Identification: producer string and epoch (IDENTIFICATION_BLOCK) and the VERSION record
//      of the first MODULE_BLOCK. A newer epoch or module version is A2S_ERROR_UNSUPPORTED (the
//      stream is well-formed, just newer than the linked LLVM; LLVM itself reports the module
//      version only as "Invalid value", indistinguishable from corruption).
//   2. Resource limits: LLVM 18's reader sizes containers directly from a few record operands
//      without plausibility checks (type table, basic blocks, attribute indices, block
//      addresses), and vector and integer types of absurd size make it allocate in proportion
//      (shuffle masks, splats, APInt). A record of a few bits could make it allocate gigabytes,
//      and an allocation failure inside LLVM is fatal (llvm_runtime.hpp). Those operands are
//      bounded here, by what the stream could possibly encode (otherwise MALFORMED) and by
//      fixed limits (otherwise UNSUPPORTED, "limit exceeded").
//   3. Records LLVM 18 reads without checks (found by fuzz_air_bitcode): attribute group
//      entries must be complete, DATA constants need an array or vector type (the reader casts
//      unchecked; the pre-scan keeps a table of type kinds for that), metadata attachments must
//      name an existing instruction, inline assembly is refused (Metal has none, and old-style
//      records are cast unchecked as well).
//   4. Blocks that do not matter for the translation but crash LLVM 18 when malformed are
//      listed for neutralisation: USELIST_BLOCK (use-list order only; the reader indexes its
//      value and basic block tables with unchecked IDs from it). The caller rewrites their block
//      ID to an unknown one in a private copy of the input, so LLVM skips them.
//
// Decoding like LLVM: the checks only protect LLVM if the pre-scan reads the same records with
// the same abbreviations as LLVM 18's reader. So it follows the reader's structure: the role of
// a block depends on its parent (a CONSTANTS_BLOCK is checked as a child of the module or of a
// function, elsewhere the reader skips it and the pre-scan only walks it); BLOCKINFO counts only
// as the single direct child of the MODULE_BLOCK before the first function block (the reader
// skips it elsewhere; other placements are UNSUPPORTED); module-level abbreviation definitions
// after the first function block are UNSUPPORTED (the reader jumps over that region); every
// block must end exactly at its declared length (the reader skips some blocks by length and
// parses others to END_BLOCK); the word offsets the reader jumps to (VSTOFFSET, FNENTRY) must
// name a symbol table / function block of the module that the scan has read, and FNENTRY must
// name a function. Each module is scanned with fresh state, as the reader does.
//
// It is not the full bitstream audit (unknown blocks and record codes against a table of the
// pinned LLVM): that is WP-SC-07 and will extend this walk.
#pragma once

#include "air2spirv/air2spirv.h"

#include "bitcode/bitcode_module.hpp"
#include "diagnostics.hpp"

#include <cstddef>
#include <cstdint>
#include <span>
#include <vector>

namespace air2spirv::bitcode {

// Values of the pinned LLVM (llvm/Bitcode/LLVMBitCodes.h, BitcodeReader.cpp).
inline constexpr std::uint64_t kSupportedEpoch = 0;   // bitc::BITCODE_CURRENT_EPOCH
inline constexpr std::uint64_t kMaxModuleVersion = 2; // BitcodeReaderBase::parseVersionRecord
inline constexpr std::size_t kMaxProducerLength = 256;

// Fixed limits (far above anything a shader needs; exceeding them is UNSUPPORTED).
struct PrescanLimits {
    std::uint64_t max_types = 1U << 20;           // TYPE_BLOCK NUMENTRY
    std::uint64_t max_vector_elements = 1U << 16; // TYPE_BLOCK VECTOR
    std::uint64_t max_integer_bits = 4096;        // TYPE_BLOCK INTEGER
    std::uint64_t max_basic_blocks = 1U << 20;    // per function (DECLAREBLOCKS, blockaddress)
    std::uint64_t max_attribute_index = 1U << 16; // parameter index in attribute groups
    std::size_t max_block_depth = 16;             // nesting of bitstream blocks
};

// Block ID that LLVM 18 does not know and therefore skips; replaces neutralised block IDs.
// Fits into one VBR8 chunk like every block ID LLVM 18 uses.
inline constexpr std::uint8_t kNeutralBlockId = 120;

struct PrescanResult {
    BitcodeIdentity identity;
    // Bit offsets (from the start of the stream) of single-chunk VBR8 block-ID fields of blocks
    // to neutralise (see 4. above). Each currently holds a known block ID.
    std::vector<std::uint64_t> neutralize_block_id_bits;
};

//   A2S_OK                     - `out` filled (absent fields stay empty), limits respected
//   A2S_ERROR_UNSUPPORTED      - newer epoch / module version / identification record, or a
//                                fixed limit exceeded
//   A2S_ERROR_MALFORMED_INPUT  - broken block structure, a size operand the stream cannot
//                                possibly back, or an incomplete record LLVM would overread
[[nodiscard]] a2s_status prescan_bitstream(std::span<const std::byte> bitcode, PrescanResult& out,
                                           Diagnostics& diag,
                                           const PrescanLimits& limits = PrescanLimits{});

// Applies PrescanResult::neutralize_block_id_bits to `bytes` (a copy of the scanned stream).
void neutralize_blocks(std::span<std::byte> bytes, const PrescanResult& result);

} // namespace air2spirv::bitcode
