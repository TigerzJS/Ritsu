#!/bin/sh
# Regenerates the LLVM bitcode test fixtures of air2spirv (WP-SC-02) from our own sources in
# src/. See README.md in this directory. Needs clang-18, llvm-as-18 and llvm-cat-18 (Ubuntu
# 24.04: packages clang-18 and llvm-18) and python3. The output is deterministic for a given
# LLVM build. Run it from any directory:
#
#   sh compiler/air2spirv/tests/fixtures/bitcode/generate.sh
#
# Every fixture is also copied into the fuzz seed corpus
# compiler/air2spirv/fuzz/corpus/fuzz_air_bitcode/.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
corpus="$here/../../../fuzz/corpus/fuzz_air_bitcode"
clang=${CLANG:-clang-18}
llvm_as=${LLVM_AS:-llvm-as-18}
llvm_cat=${LLVM_CAT:-llvm-cat-18}
python=${PYTHON:-python3}

for tool in "$clang" "$llvm_as" "$llvm_cat" "$python"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found (set CLANG/LLVM_AS/LLVM_CAT/PYTHON)" >&2
        exit 1
    fi
done
"$clang" --version | head -n 1

# Compile from inside src/ with relative paths so no absolute path ends up in the bitcode
# (source_filename / debug info compilation directory).
cd "$here/src"
common="-c -emit-llvm -fno-ident"
# OpenCL C kernels, SPIR target (address spaces, kernel calling convention, kernel metadata).
"$clang" $common --target=spir64 -cl-std=CL2.0 -Xclang -finclude-default-header -O1 \
    vector_add.cl -o ../vector_add.spir64.bc
# Plain C: several functions, loops, switch.
"$clang" $common --target=x86_64-unknown-linux-gnu -O1 control_flow.c -o ../control_flow.x86_64.bc
"$clang" $common --target=x86_64-unknown-linux-gnu -O0 control_flow.c \
    -o ../control_flow_O0.x86_64.bc
# Aggregates and vectors; with debug info (metadata-heavy) and relative compilation directory.
"$clang" $common --target=x86_64-unknown-linux-gnu -O1 -g -fdebug-compilation-dir=. structs.c \
    -o ../structs_debug.x86_64.bc
# Darwin targets make clang emit the public LLVM bitcode wrapper header (0x0B17C0DE).
"$clang" $common --target=arm64-apple-macos14 -O1 structs.c -o ../structs.arm64-apple.wrapped.bc
# Hand-written IR. -disable-verify keeps the deliberately invalid modules invalid.
for name in minimal invalid_dominance invalid_dominance_debug broken_debug_info \
    old_debug_info_version; do
    "$llvm_as" -disable-verify "$name.ll" -o "../$name.bc"
done
cd "$here"
# Two modules in one stream (binary concatenation, like `llvm-cat -b`).
"$llvm_cat" -b minimal.bc control_flow.x86_64.bc -o two_modules.bc

# "Too new" variants of our own bitcode (A2S_ERROR_UNSUPPORTED in the tests).
"$python" patch_bitcode.py --epoch 1 control_flow.x86_64.bc unsupported_epoch1.bc
"$python" patch_bitcode.py --module-version 3 control_flow.x86_64.bc unsupported_module_version3.bc
"$python" patch_bitcode.py --attribute-kind 100 control_flow.x86_64.bc \
    unsupported_attribute_kind100.bc
# Attribute group for parameter 2^31: LLVM 18 would allocate 16 GiB for it (pre-scan limit).
"$python" patch_bitcode.py --attribute-index 2147483648 control_flow.x86_64.bc \
    huge_attribute_index.bc

# Block-layout variants: the pre-scan must decode like LLVM 18 or reject the layout.
"$python" -B make_layout_fixtures.py control_flow.x86_64.bc .

# Every fixture is also a fuzz seed (valid, invalid and "too new" inputs alike).
mkdir -p "$corpus"
for f in *.bc; do
    cp "$f" "$corpus/$f"
done
echo "fixtures written to $here, seeds to $corpus"
