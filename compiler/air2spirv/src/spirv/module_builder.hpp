// Deterministic SPIR-V module builder (docs/subsystems/shader-compiler.md §2.6, §7; WP-SC-03).
//
// Determinism rules (equal call sequence => bit-identical words):
//   - result ids are handed out in call order, starting at 1; nothing depends on addresses,
//     time, hashing of pointers or thread count,
//   - types (except structs) and constants are deduplicated; the first request defines the id
//     and the position in the module, later requests return the same id,
//   - capabilities and extensions are emitted sorted (by enum value / by name), independent of
//     the order in which lowering code requested them,
//   - everything else (entry points, execution modes, names, decorations, globals, functions)
//     is emitted in call order; identical decorations are emitted once.
//
// Misuse (instruction outside a block, unbalanced functions, missing memory model, literal
// strings with NUL, exceeding SPIR-V universal limits) throws BuilderError. Such errors are
// compiler bugs or limit violations; the C ABI turns them into A2S_ERROR_INTERNAL.
//
// The builder does not validate semantics. Every finished module must go through
// spirv::validate() (tools.hpp) before it leaves the library.
#pragma once

#include "spirv/spirv_headers.hpp"

#include <cstdint>
#include <initializer_list>
#include <map>
#include <optional>
#include <set>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace air2spirv::spirv {

using Id = std::uint32_t;
using Words = std::vector<std::uint32_t>;

inline constexpr Id kNoId = 0;
// SPIR-V 1.6 (ADR-0007: Vulkan SPIR-V 1.6 is the only output version).
inline constexpr std::uint32_t kSpirvVersion16 = 0x00010600U;
// Header word 2. 0 is explicitly allowed by the SPIR-V spec ("unknown generator"); a Khronos
// tool id can be registered later (see docs/subsystems/shader-compiler.md §13).
inline constexpr std::uint32_t kGeneratorMagic = 0;
// SPIR-V universal limit for the result <id> bound (spec §2.17), also spirv-val's default.
inline constexpr std::uint32_t kMaxIdBound = 0x3FFFFFU;
// Instruction word count and opcode are 16-bit fields.
inline constexpr std::size_t kMaxInstructionWords = 0xFFFFU;
inline constexpr std::uint32_t kMaxOpcode = 0xFFFFU;

class BuilderError : public std::logic_error {
public:
    using std::logic_error::logic_error;
};

struct BuilderOptions {
    // Emit OpName/OpMemberName. Names never influence ids, so toggling this only adds or
    // removes the debug-name section.
    bool emit_debug_names = true;
    // Upper limit for the id bound (resource limit against hostile input). Values above
    // kMaxIdBound are clamped to it.
    std::uint32_t max_id_bound = kMaxIdBound;
};

// Appends a SPIR-V literal string (UTF-8, NUL-terminated, zero-padded to a word boundary,
// first byte in the lowest-order bits). Throws BuilderError if `text` contains a NUL byte.
void append_literal_string(Words& words, std::string_view text);

class ModuleBuilder {
public:
    explicit ModuleBuilder(BuilderOptions options = {});

    // A fresh result id for a function or label that is referenced before it is defined (entry
    // points, forward branches). begin_function()/begin_block() accept only such reserved ids,
    // each exactly once.
    [[nodiscard]] Id reserve_id();
    // One past the largest id handed out so far (header word 3).
    [[nodiscard]] std::uint32_t id_bound() const noexcept { return next_id_; }
    // The effective id bound limit (BuilderOptions::max_id_bound clamped to kMaxIdBound).
    [[nodiscard]] std::uint32_t max_id_bound() const noexcept { return max_id_bound_; }

    // ---- module-level declarations -----------------------------------------------------------
    void add_capability(spv::Capability capability);
    void add_extension(std::string_view name);
    [[nodiscard]] Id import_extended_instructions(std::string_view name);
    void set_memory_model(spv::AddressingModel addressing, spv::MemoryModel memory);
    void add_entry_point(spv::ExecutionModel model, Id function, std::string_view name,
                         std::span<const Id> interface);
    void add_execution_mode(Id entry_point, spv::ExecutionMode mode,
                            std::initializer_list<std::uint32_t> literals = {});
    void set_name(Id target, std::string_view name);
    void set_member_name(Id struct_type, std::uint32_t member, std::string_view name);
    void decorate(Id target, spv::Decoration decoration,
                  std::initializer_list<std::uint32_t> literals = {});
    void decorate_member(Id struct_type, std::uint32_t member, spv::Decoration decoration,
                         std::initializer_list<std::uint32_t> literals = {});

    // ---- types (deduplicated, except structs) ------------------------------------------------
    [[nodiscard]] Id type_void();
    [[nodiscard]] Id type_bool();
    [[nodiscard]] Id type_int(std::uint32_t width, bool is_signed);
    [[nodiscard]] Id type_float(std::uint32_t width);
    [[nodiscard]] Id type_vector(Id component, std::uint32_t count);
    [[nodiscard]] Id type_matrix(Id column, std::uint32_t columns);
    // `array_stride` != 0 adds an ArrayStride decoration. The stride is part of the
    // deduplication key: the same element type with different strides gives different ids.
    [[nodiscard]] Id type_array(Id element, Id length_constant, std::uint32_t array_stride = 0);
    [[nodiscard]] Id type_runtime_array(Id element, std::uint32_t array_stride = 0);
    // Always a new id: struct identity carries Block/Offset decorations.
    [[nodiscard]] Id type_struct(std::span<const Id> members);
    // `array_stride` != 0 adds an ArrayStride decoration (PhysicalStorageBuffer pointers used
    // with OpPtrAccessChain) and is part of the deduplication key.
    [[nodiscard]] Id type_pointer(spv::StorageClass storage, Id pointee,
                                  std::uint32_t array_stride = 0);
    // OpTypeForwardPointer for recursive PhysicalStorageBuffer types. The returned pointer id
    // must be completed with complete_forward_pointer() before finish(). Completing throws if
    // type_pointer(storage, pointee) already exists (that would declare two pointer types).
    [[nodiscard]] Id type_forward_pointer(spv::StorageClass storage);
    void complete_forward_pointer(Id pointer, Id pointee);
    [[nodiscard]] Id type_function(Id return_type, std::span<const Id> parameters);

    // ---- constants (deduplicated by type and bit pattern) ------------------------------------
    [[nodiscard]] Id constant_bool(bool value);
    [[nodiscard]] Id constant_u32(std::uint32_t value);
    [[nodiscard]] Id constant_i32(std::int32_t value);
    [[nodiscard]] Id constant_f32(float value);
    // OpConstant with raw literal words (low-order word first for 64-bit types).
    [[nodiscard]] Id constant(Id type, std::span<const std::uint32_t> literal_words);
    [[nodiscard]] Id constant_composite(Id type, std::span<const Id> constituents);
    [[nodiscard]] Id constant_null(Id type);

    // Types, constants and global variables may be requested at any time, also while a
    // function body is being emitted; they always go to the global section.

    // ---- global variables (never deduplicated) -----------------------------------------------
    [[nodiscard]] Id global_variable(Id pointer_type, spv::StorageClass storage,
                                     Id initializer = kNoId);

    // ---- functions ---------------------------------------------------------------------------
    // `function` = kNoId allocates a new id; otherwise it must come from reserve_id() and not be
    // defined yet.
    Id begin_function(Id result_type, spv::FunctionControlMask control, Id function_type,
                      Id function = kNoId);
    [[nodiscard]] Id add_parameter(Id type);
    // Opens a basic block. `label` = kNoId allocates a new id; otherwise it must come from
    // reserve_id() and not be defined yet (forward branch targets).
    Id begin_block(Id label = kNoId);
    // Emits one instruction into the open block. Returns the result id, or kNoId for
    // instructions without a result. A block terminator closes the block. Module-level
    // instructions (types, constants, spec constants, debug source info, decorations, labels,
    // functions, OpVariable outside the Function storage class, ...) are rejected.
    Id emit(spv::Op op, Id result_type, std::span<const std::uint32_t> operands);
    Id emit(spv::Op op, Id result_type, std::initializer_list<std::uint32_t> operands);
    void end_function();

    [[nodiscard]] bool in_function() const noexcept { return in_function_; }
    [[nodiscard]] bool in_block() const noexcept { return in_block_; }

    // The complete module. Throws BuilderError if the module is structurally incomplete (open
    // function, no memory model, unresolved forward pointer). Const and repeatable.
    [[nodiscard]] Words finish() const;

private:
    using Key = std::vector<std::uint32_t>;

    Id fresh_id();
    void define_reserved(Id id, const char* what);
    Id cached(Key key, spv::Op op, Id result_type, std::span<const std::uint32_t> operands);
    static void require_id(Id id, const char* what);

    BuilderOptions options_;
    std::uint32_t next_id_ = 1;
    std::uint32_t max_id_bound_ = kMaxIdBound;
    std::set<Id> reserved_undefined_; // reserve_id() results not yet defined

    std::set<std::uint32_t> capabilities_;
    std::set<std::string, std::less<>> extensions_;
    std::map<std::string, Id, std::less<>> ext_imports_;
    Words ext_import_words_;
    std::optional<std::pair<spv::AddressingModel, spv::MemoryModel>> memory_model_;
    Words entry_points_;
    Words execution_modes_;
    Words debug_names_;
    Words annotations_;
    std::set<Key> annotation_keys_;
    Words globals_; // types, constants, global variables in creation order
    Words functions_;

    std::map<Key, Id> type_and_constant_ids_;
    std::map<Id, spv::StorageClass> open_forward_pointers_;

    bool in_function_ = false;
    bool in_block_ = false;
    bool function_has_block_ = false;
};

} // namespace air2spirv::spirv
