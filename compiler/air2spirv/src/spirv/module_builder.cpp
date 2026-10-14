#include "spirv/module_builder.hpp"

#include <bit>
#include <string>

namespace air2spirv::spirv {
namespace {

template <typename Enum>
constexpr std::uint32_t word(Enum value) noexcept {
    return static_cast<std::uint32_t>(value);
}

void append_instruction(Words& out, spv::Op op, std::span<const std::uint32_t> operands) {
    if (word(op) > kMaxOpcode) {
        throw BuilderError("SPIR-V opcode " + std::to_string(word(op)) +
                           " does not fit the 16-bit opcode field");
    }
    const std::size_t count = operands.size() + 1;
    if (count > kMaxInstructionWords) {
        throw BuilderError("SPIR-V instruction " + std::to_string(word(op)) + " would have " +
                           std::to_string(count) + " words (limit 65535)");
    }
    out.push_back((static_cast<std::uint32_t>(count) << 16U) | word(op));
    out.insert(out.end(), operands.begin(), operands.end());
}

bool is_block_terminator(spv::Op op) noexcept {
    switch (op) {
    case spv::Op::OpBranch:
    case spv::Op::OpBranchConditional:
    case spv::Op::OpSwitch:
    case spv::Op::OpReturn:
    case spv::Op::OpReturnValue:
    case spv::Op::OpKill:
    case spv::Op::OpUnreachable:
    case spv::Op::OpTerminateInvocation:
    case spv::Op::OpIgnoreIntersectionKHR:
    case spv::Op::OpTerminateRayKHR:
    case spv::Op::OpEmitMeshTasksEXT:
        return true;
    default:
        return false;
    }
}

// Instructions that must go through a dedicated ModuleBuilder method because the builder
// tracks their structure or places them in a module section.
bool has_dedicated_method(spv::Op op) noexcept {
    switch (op) {
    case spv::Op::OpFunction:
    case spv::Op::OpFunctionParameter:
    case spv::Op::OpFunctionEnd:
    case spv::Op::OpLabel:
    case spv::Op::OpCapability:
    case spv::Op::OpExtension:
    case spv::Op::OpExtInstImport:
    case spv::Op::OpMemoryModel:
    case spv::Op::OpEntryPoint:
    case spv::Op::OpExecutionMode:
    case spv::Op::OpExecutionModeId:
    case spv::Op::OpName:
    case spv::Op::OpMemberName:
    case spv::Op::OpDecorate:
    case spv::Op::OpMemberDecorate:
    case spv::Op::OpDecorateId:
    case spv::Op::OpDecorateString:
    case spv::Op::OpMemberDecorateString:
    case spv::Op::OpTypeForwardPointer:
    case spv::Op::OpConstant:
    case spv::Op::OpConstantTrue:
    case spv::Op::OpConstantFalse:
    case spv::Op::OpConstantComposite:
    case spv::Op::OpConstantNull:
    case spv::Op::OpConstantSampler:
    case spv::Op::OpSpecConstant:
    case spv::Op::OpSpecConstantTrue:
    case spv::Op::OpSpecConstantFalse:
    case spv::Op::OpSpecConstantComposite:
    case spv::Op::OpSpecConstantOp:
    case spv::Op::OpSource:
    case spv::Op::OpSourceContinued:
    case spv::Op::OpSourceExtension:
    case spv::Op::OpString:
    case spv::Op::OpModuleProcessed:
    case spv::Op::OpDecorationGroup:
    case spv::Op::OpGroupDecorate:
    case spv::Op::OpGroupMemberDecorate:
        return true;
    default:
        return false;
    }
}

void check_no_nul(std::string_view text, const char* what) {
    if (text.find('\0') != std::string_view::npos) {
        throw BuilderError(std::string(what) + " must not contain a NUL byte");
    }
}

} // namespace

void append_literal_string(Words& words, std::string_view text) {
    check_no_nul(text, "SPIR-V literal string");
    // text.size() + 1 bytes including the terminating NUL, rounded up to whole words.
    const std::size_t word_count = text.size() / 4 + 1;
    const std::size_t first = words.size();
    words.resize(first + word_count, 0U);
    for (std::size_t i = 0; i < text.size(); ++i) {
        const auto byte = static_cast<std::uint32_t>(static_cast<unsigned char>(text[i]));
        words[first + i / 4] |= byte << (8U * static_cast<std::uint32_t>(i % 4));
    }
}

ModuleBuilder::ModuleBuilder(BuilderOptions options)
    : options_(options),
      max_id_bound_(options.max_id_bound < kMaxIdBound ? options.max_id_bound : kMaxIdBound) {}

Id ModuleBuilder::fresh_id() {
    if (next_id_ >= max_id_bound_) {
        throw BuilderError("SPIR-V id bound would exceed the limit of " +
                           std::to_string(max_id_bound_));
    }
    return next_id_++;
}

Id ModuleBuilder::reserve_id() {
    const Id id = fresh_id();
    reserved_undefined_.insert(id);
    return id;
}

void ModuleBuilder::define_reserved(Id id, const char* what) {
    if (reserved_undefined_.erase(id) == 0) {
        throw BuilderError(std::string(what) + ": id " + std::to_string(id) +
                           " was not reserved with reserve_id() or is already defined");
    }
}

void ModuleBuilder::require_id(Id id, const char* what) {
    if (id == kNoId) {
        throw BuilderError(std::string(what) + ": id 0 is not a valid operand");
    }
}

// ---- module-level declarations -----------------------------------------------------------------

void ModuleBuilder::add_capability(spv::Capability capability) {
    capabilities_.insert(word(capability));
}

void ModuleBuilder::add_extension(std::string_view name) {
    check_no_nul(name, "extension name");
    extensions_.emplace(name);
}

Id ModuleBuilder::import_extended_instructions(std::string_view name) {
    check_no_nul(name, "extended instruction set name");
    if (const auto it = ext_imports_.find(name); it != ext_imports_.end()) {
        return it->second;
    }
    const Id id = fresh_id();
    Words operands{id};
    append_literal_string(operands, name);
    append_instruction(ext_import_words_, spv::Op::OpExtInstImport, operands);
    ext_imports_.emplace(std::string(name), id);
    return id;
}

void ModuleBuilder::set_memory_model(spv::AddressingModel addressing, spv::MemoryModel memory) {
    if (memory_model_.has_value() &&
        (memory_model_->first != addressing || memory_model_->second != memory)) {
        throw BuilderError("memory model set twice with different values");
    }
    memory_model_ = std::make_pair(addressing, memory);
}

void ModuleBuilder::add_entry_point(spv::ExecutionModel model, Id function, std::string_view name,
                                    std::span<const Id> interface) {
    require_id(function, "OpEntryPoint function");
    Words operands{word(model), function};
    append_literal_string(operands, name);
    for (const Id id : interface) {
        require_id(id, "OpEntryPoint interface");
        operands.push_back(id);
    }
    append_instruction(entry_points_, spv::Op::OpEntryPoint, operands);
}

void ModuleBuilder::add_execution_mode(Id entry_point, spv::ExecutionMode mode,
                                       std::initializer_list<std::uint32_t> literals) {
    require_id(entry_point, "OpExecutionMode entry point");
    Words operands{entry_point, word(mode)};
    operands.insert(operands.end(), literals.begin(), literals.end());
    append_instruction(execution_modes_, spv::Op::OpExecutionMode, operands);
}

void ModuleBuilder::set_name(Id target, std::string_view name) {
    require_id(target, "OpName target");
    Words operands{target};
    append_literal_string(operands, name); // validates even when names are stripped
    if (options_.emit_debug_names) {
        append_instruction(debug_names_, spv::Op::OpName, operands);
    }
}

void ModuleBuilder::set_member_name(Id struct_type, std::uint32_t member, std::string_view name) {
    require_id(struct_type, "OpMemberName type");
    Words operands{struct_type, member};
    append_literal_string(operands, name);
    if (options_.emit_debug_names) {
        append_instruction(debug_names_, spv::Op::OpMemberName, operands);
    }
}

void ModuleBuilder::decorate(Id target, spv::Decoration decoration,
                             std::initializer_list<std::uint32_t> literals) {
    require_id(target, "OpDecorate target");
    Words operands{target, word(decoration)};
    operands.insert(operands.end(), literals.begin(), literals.end());
    Key key{word(spv::Op::OpDecorate)};
    key.insert(key.end(), operands.begin(), operands.end());
    if (annotation_keys_.insert(std::move(key)).second) {
        append_instruction(annotations_, spv::Op::OpDecorate, operands);
    }
}

void ModuleBuilder::decorate_member(Id struct_type, std::uint32_t member,
                                    spv::Decoration decoration,
                                    std::initializer_list<std::uint32_t> literals) {
    require_id(struct_type, "OpMemberDecorate type");
    Words operands{struct_type, member, word(decoration)};
    operands.insert(operands.end(), literals.begin(), literals.end());
    Key key{word(spv::Op::OpMemberDecorate)};
    key.insert(key.end(), operands.begin(), operands.end());
    if (annotation_keys_.insert(std::move(key)).second) {
        append_instruction(annotations_, spv::Op::OpMemberDecorate, operands);
    }
}

// ---- types
// ---------------------------------------------------------------------------------------

Id ModuleBuilder::cached(Key key, spv::Op op, Id result_type,
                         std::span<const std::uint32_t> operands) {
    if (const auto it = type_and_constant_ids_.find(key); it != type_and_constant_ids_.end()) {
        return it->second;
    }
    const Id id = fresh_id();
    Words words;
    words.reserve(operands.size() + 2);
    if (result_type != kNoId) {
        words.push_back(result_type);
    }
    words.push_back(id);
    words.insert(words.end(), operands.begin(), operands.end());
    append_instruction(globals_, op, words);
    type_and_constant_ids_.emplace(std::move(key), id);
    return id;
}

Id ModuleBuilder::type_void() {
    return cached({word(spv::Op::OpTypeVoid)}, spv::Op::OpTypeVoid, kNoId, {});
}

Id ModuleBuilder::type_bool() {
    return cached({word(spv::Op::OpTypeBool)}, spv::Op::OpTypeBool, kNoId, {});
}

Id ModuleBuilder::type_int(std::uint32_t width, bool is_signed) {
    const Words operands{width, is_signed ? 1U : 0U};
    return cached({word(spv::Op::OpTypeInt), width, operands[1]}, spv::Op::OpTypeInt, kNoId,
                  operands);
}

Id ModuleBuilder::type_float(std::uint32_t width) {
    const Words operands{width};
    return cached({word(spv::Op::OpTypeFloat), width}, spv::Op::OpTypeFloat, kNoId, operands);
}

Id ModuleBuilder::type_vector(Id component, std::uint32_t count) {
    require_id(component, "OpTypeVector component");
    const Words operands{component, count};
    return cached({word(spv::Op::OpTypeVector), component, count}, spv::Op::OpTypeVector, kNoId,
                  operands);
}

Id ModuleBuilder::type_matrix(Id column, std::uint32_t columns) {
    require_id(column, "OpTypeMatrix column");
    const Words operands{column, columns};
    return cached({word(spv::Op::OpTypeMatrix), column, columns}, spv::Op::OpTypeMatrix, kNoId,
                  operands);
}

Id ModuleBuilder::type_array(Id element, Id length_constant, std::uint32_t array_stride) {
    require_id(element, "OpTypeArray element");
    require_id(length_constant, "OpTypeArray length");
    Key key{word(spv::Op::OpTypeArray), element, length_constant, array_stride};
    const bool is_new = !type_and_constant_ids_.contains(key);
    const Words operands{element, length_constant};
    const Id id = cached(std::move(key), spv::Op::OpTypeArray, kNoId, operands);
    if (is_new && array_stride != 0) {
        decorate(id, spv::Decoration::ArrayStride, {array_stride});
    }
    return id;
}

Id ModuleBuilder::type_runtime_array(Id element, std::uint32_t array_stride) {
    require_id(element, "OpTypeRuntimeArray element");
    Key key{word(spv::Op::OpTypeRuntimeArray), element, array_stride};
    const bool is_new = !type_and_constant_ids_.contains(key);
    const Words operands{element};
    const Id id = cached(std::move(key), spv::Op::OpTypeRuntimeArray, kNoId, operands);
    if (is_new && array_stride != 0) {
        decorate(id, spv::Decoration::ArrayStride, {array_stride});
    }
    return id;
}

Id ModuleBuilder::type_struct(std::span<const Id> members) {
    const Id id = fresh_id();
    Words operands{id};
    for (const Id member : members) {
        require_id(member, "OpTypeStruct member");
        operands.push_back(member);
    }
    append_instruction(globals_, spv::Op::OpTypeStruct, operands);
    return id;
}

Id ModuleBuilder::type_pointer(spv::StorageClass storage, Id pointee, std::uint32_t array_stride) {
    require_id(pointee, "OpTypePointer pointee");
    Key key{word(spv::Op::OpTypePointer), word(storage), pointee, array_stride};
    const bool is_new = !type_and_constant_ids_.contains(key);
    const Words operands{word(storage), pointee};
    const Id id = cached(std::move(key), spv::Op::OpTypePointer, kNoId, operands);
    if (is_new && array_stride != 0) {
        decorate(id, spv::Decoration::ArrayStride, {array_stride});
    }
    return id;
}

Id ModuleBuilder::type_forward_pointer(spv::StorageClass storage) {
    const Id id = fresh_id();
    const Words operands{id, word(storage)};
    append_instruction(globals_, spv::Op::OpTypeForwardPointer, operands);
    open_forward_pointers_.emplace(id, storage);
    return id;
}

void ModuleBuilder::complete_forward_pointer(Id pointer, Id pointee) {
    require_id(pointee, "OpTypePointer pointee");
    const auto it = open_forward_pointers_.find(pointer);
    if (it == open_forward_pointers_.end()) {
        throw BuilderError("complete_forward_pointer: id " + std::to_string(pointer) +
                           " is not an open forward pointer");
    }
    const spv::StorageClass storage = it->second;
    Key key{word(spv::Op::OpTypePointer), word(storage), pointee, 0U};
    if (type_and_constant_ids_.contains(key)) {
        throw BuilderError("complete_forward_pointer: a pointer type to id " +
                           std::to_string(pointee) +
                           " in this storage class already exists; it would be declared twice");
    }
    open_forward_pointers_.erase(it);
    const Words operands{pointer, word(storage), pointee};
    append_instruction(globals_, spv::Op::OpTypePointer, operands);
    // Later type_pointer(storage, pointee) requests reuse the forward-declared id.
    type_and_constant_ids_.emplace(std::move(key), pointer);
}

Id ModuleBuilder::type_function(Id return_type, std::span<const Id> parameters) {
    require_id(return_type, "OpTypeFunction return type");
    Words operands{return_type};
    for (const Id parameter : parameters) {
        require_id(parameter, "OpTypeFunction parameter");
        operands.push_back(parameter);
    }
    Key key{word(spv::Op::OpTypeFunction)};
    key.insert(key.end(), operands.begin(), operands.end());
    return cached(std::move(key), spv::Op::OpTypeFunction, kNoId, operands);
}

// ---- constants
// -----------------------------------------------------------------------------------

Id ModuleBuilder::constant_bool(bool value) {
    const Id type = type_bool();
    const spv::Op op = value ? spv::Op::OpConstantTrue : spv::Op::OpConstantFalse;
    return cached({word(op), type}, op, type, {});
}

Id ModuleBuilder::constant_u32(std::uint32_t value) {
    const std::uint32_t literal[] = {value};
    return constant(type_int(32, false), literal);
}

Id ModuleBuilder::constant_i32(std::int32_t value) {
    const std::uint32_t literal[] = {std::bit_cast<std::uint32_t>(value)};
    return constant(type_int(32, true), literal);
}

Id ModuleBuilder::constant_f32(float value) {
    // Keyed by bit pattern: +0.0 and -0.0 (and NaN payloads) stay distinct.
    const std::uint32_t literal[] = {std::bit_cast<std::uint32_t>(value)};
    return constant(type_float(32), literal);
}

Id ModuleBuilder::constant(Id type, std::span<const std::uint32_t> literal_words) {
    require_id(type, "OpConstant type");
    if (literal_words.empty()) {
        throw BuilderError("OpConstant needs at least one literal word");
    }
    Key key{word(spv::Op::OpConstant), type};
    key.insert(key.end(), literal_words.begin(), literal_words.end());
    return cached(std::move(key), spv::Op::OpConstant, type, literal_words);
}

Id ModuleBuilder::constant_composite(Id type, std::span<const Id> constituents) {
    require_id(type, "OpConstantComposite type");
    Key key{word(spv::Op::OpConstantComposite), type};
    for (const Id constituent : constituents) {
        require_id(constituent, "OpConstantComposite constituent");
        key.push_back(constituent);
    }
    return cached(std::move(key), spv::Op::OpConstantComposite, type, constituents);
}

Id ModuleBuilder::constant_null(Id type) {
    require_id(type, "OpConstantNull type");
    return cached({word(spv::Op::OpConstantNull), type}, spv::Op::OpConstantNull, type, {});
}

// ---- global variables
// ----------------------------------------------------------------------------

Id ModuleBuilder::global_variable(Id pointer_type, spv::StorageClass storage, Id initializer) {
    require_id(pointer_type, "OpVariable type");
    if (storage == spv::StorageClass::Function) {
        throw BuilderError("Function-storage variables belong in a function body (use emit)");
    }
    const Id id = fresh_id();
    Words operands{pointer_type, id, word(storage)};
    if (initializer != kNoId) {
        operands.push_back(initializer);
    }
    append_instruction(globals_, spv::Op::OpVariable, operands);
    return id;
}

// ---- functions
// -----------------------------------------------------------------------------------

Id ModuleBuilder::begin_function(Id result_type, spv::FunctionControlMask control, Id function_type,
                                 Id function) {
    if (in_function_) {
        throw BuilderError("begin_function: previous function not ended");
    }
    require_id(result_type, "OpFunction result type");
    require_id(function_type, "OpFunction function type");
    if (function == kNoId) {
        function = fresh_id();
    } else {
        define_reserved(function, "begin_function");
    }
    const Words operands{result_type, function, word(control), function_type};
    append_instruction(functions_, spv::Op::OpFunction, operands);
    in_function_ = true;
    in_block_ = false;
    function_has_block_ = false;
    return function;
}

Id ModuleBuilder::add_parameter(Id type) {
    if (!in_function_ || function_has_block_) {
        throw BuilderError("add_parameter: only allowed after begin_function, before any block");
    }
    require_id(type, "OpFunctionParameter type");
    const Id id = fresh_id();
    const Words operands{type, id};
    append_instruction(functions_, spv::Op::OpFunctionParameter, operands);
    return id;
}

Id ModuleBuilder::begin_block(Id label) {
    if (!in_function_) {
        throw BuilderError("begin_block: no open function");
    }
    if (in_block_) {
        throw BuilderError("begin_block: previous block has no terminator");
    }
    if (label == kNoId) {
        label = fresh_id();
    } else {
        define_reserved(label, "begin_block");
    }
    const Words operands{label};
    append_instruction(functions_, spv::Op::OpLabel, operands);
    in_block_ = true;
    function_has_block_ = true;
    return label;
}

Id ModuleBuilder::emit(spv::Op op, Id result_type, std::span<const std::uint32_t> operands) {
    if (!in_block_) {
        throw BuilderError("emit: opcode " + std::to_string(word(op)) + " outside a basic block");
    }
    if (has_dedicated_method(op)) {
        throw BuilderError("emit: opcode " + std::to_string(word(op)) +
                           " must use its dedicated ModuleBuilder method");
    }
    bool has_result = false;
    bool has_result_type = false;
    spv::HasResultAndType(op, &has_result, &has_result_type);
    if (has_result && !has_result_type) {
        // Types, labels, strings, imports: module-level declarations, never in a block.
        throw BuilderError("emit: opcode " + std::to_string(word(op)) +
                           " is a declaration and cannot appear in a block");
    }
    if (op == spv::Op::OpVariable &&
        (operands.empty() || operands[0] != word(spv::StorageClass::Function))) {
        throw BuilderError("emit: OpVariable in a function body must use the Function storage "
                           "class (use global_variable for module-scope variables)");
    }
    if (has_result_type != (result_type != kNoId)) {
        throw BuilderError("emit: opcode " + std::to_string(word(op)) +
                           (has_result_type ? " needs a result type" : " takes no result type"));
    }
    Words words;
    words.reserve(operands.size() + 2);
    Id result = kNoId;
    if (has_result_type) {
        words.push_back(result_type);
    }
    if (has_result) {
        result = fresh_id();
        words.push_back(result);
    }
    words.insert(words.end(), operands.begin(), operands.end());
    append_instruction(functions_, op, words);
    if (is_block_terminator(op)) {
        in_block_ = false;
    }
    return result;
}

Id ModuleBuilder::emit(spv::Op op, Id result_type, std::initializer_list<std::uint32_t> operands) {
    return emit(op, result_type, std::span<const std::uint32_t>(operands.begin(), operands.size()));
}

void ModuleBuilder::end_function() {
    if (!in_function_) {
        throw BuilderError("end_function: no open function");
    }
    if (in_block_) {
        throw BuilderError("end_function: last block has no terminator");
    }
    append_instruction(functions_, spv::Op::OpFunctionEnd, {});
    in_function_ = false;
    function_has_block_ = false;
}

// ---- output
// --------------------------------------------------------------------------------------

Words ModuleBuilder::finish() const {
    if (in_function_) {
        throw BuilderError("finish: a function is still open");
    }
    if (!memory_model_.has_value()) {
        throw BuilderError("finish: no memory model set");
    }
    if (!open_forward_pointers_.empty()) {
        throw BuilderError("finish: forward pointer " +
                           std::to_string(open_forward_pointers_.begin()->first) +
                           " was never completed");
    }

    Words out{spv::MagicNumber, kSpirvVersion16, kGeneratorMagic, next_id_, 0U};
    // Logical layout order, SPIR-V spec §2.4.
    for (const std::uint32_t capability : capabilities_) {
        const Words operands{capability};
        append_instruction(out, spv::Op::OpCapability, operands);
    }
    for (const std::string& extension : extensions_) {
        Words operands;
        append_literal_string(operands, extension);
        append_instruction(out, spv::Op::OpExtension, operands);
    }
    out.insert(out.end(), ext_import_words_.begin(), ext_import_words_.end());
    const Words memory_model{word(memory_model_->first), word(memory_model_->second)};
    append_instruction(out, spv::Op::OpMemoryModel, memory_model);
    for (const Words* section : {&entry_points_, &execution_modes_, &debug_names_, &annotations_,
                                 &globals_, &functions_}) {
        out.insert(out.end(), section->begin(), section->end());
    }
    return out;
}

} // namespace air2spirv::spirv
