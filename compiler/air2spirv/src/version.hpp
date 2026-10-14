// Build identity of the library; part of every shader cache key.
#pragma once

#ifndef A2S_VERSION
#error "A2S_VERSION must be defined by the build system"
#endif

#if defined(A2S_HAVE_LLVM) && A2S_HAVE_LLVM
#include <llvm/Config/llvm-config.h>
#define A2S_LLVM_DESCRIPTION "llvm " LLVM_VERSION_STRING
#else
#define A2S_LLVM_DESCRIPTION "without llvm"
#endif

#define A2S_VERSION_STRING "air2spirv " A2S_VERSION " (" A2S_LLVM_DESCRIPTION ")"
