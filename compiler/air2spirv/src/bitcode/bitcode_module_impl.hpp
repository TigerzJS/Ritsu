// LLVM side of ParsedModule. Include only from src/bitcode, src/air and src/ir (§3 of
// docs/subsystems/shader-compiler.md); everything else uses bitcode_module.hpp.
#pragma once

#include "bitcode/bitcode_module.hpp"

#include <llvm/IR/LLVMContext.h>
#include <llvm/IR/Module.h>
#include <memory>

namespace air2spirv::bitcode {

struct ParsedModule::Impl {
    // One context per compilation (parallel compilations never share LLVM state). Declared
    // before `module` so that the module is destroyed first.
    std::unique_ptr<llvm::LLVMContext> context;
    std::unique_ptr<llvm::Module> module;
};

} // namespace air2spirv::bitcode
