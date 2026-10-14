#!/bin/sh
# Time-boxed run of the libFuzzer target fuzz_air_bitcode (stage 3: bitcode pre-scan, LLVM
# bitcode reader, verifier; WP-SC-02). Builds with clang, -DAIR2SPIRV_WITH_LLVM=ON and
# -DAIR2SPIRV_BUILD_FUZZERS=ON (ASan/UBSan forced on by CMake), then fuzzes for FUZZ_TIME seconds
# (default 60) on the checked-in seed corpus plus regression inputs. The checked-in corpus is
# never modified: new inputs go to a work directory.
#
# LLVM 18's bitcode reader has crash classes that hostile bitcode reaches within seconds and that
# air2spirv cannot rule out (docs/subsystems/shader-compiler.md §11.4; the answer is process
# isolation, WP-ARCH-04). The fuzzer therefore runs in fork mode and keeps going after crashes,
# leaks, timeouts and OOMs; afterwards triage.py replays every artifact and fails the run only
# for findings in air2spirv's own code (or findings it cannot attribute). Findings inside LLVM are
# listed and kept as artifacts.
#
# Needs: clang, cmake, ninja, llvm-18-dev, libzstd-dev, libclang-rt-18-dev, spirv-tools,
# spirv-headers.
#
# Environment:
#   FUZZ_TIME             seconds (default 60)
#   FUZZ_WORK             work directory (default: temporary, removed afterwards)
#   FUZZ_REQUIRED=1       fail instead of skipping when a tool is missing
#   CPP_FUZZ_LLVM_BUILD_DIR  build directory (default build/air2spirv-fuzz-llvm)
#   FUZZ_ARTIFACTS        where crashes go (default fuzz/artifacts/fuzz_air_bitcode)
#   FUZZ_JOBS             parallel fork-mode workers (default 2)
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
cd "$root"

time=${FUZZ_TIME:-60}
build=${CPP_FUZZ_LLVM_BUILD_DIR:-build/air2spirv-fuzz-llvm}
src=compiler/air2spirv
target=fuzz_air_bitcode
artifacts=${FUZZ_ARTIFACTS:-$root/fuzz/artifacts/$target}

for tool in clang clang++ cmake ninja; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        if [ "${FUZZ_REQUIRED:-0}" = 1 ]; then
            echo "error: $tool is missing" >&2
            exit 1
        fi
        echo "skip: $tool is not installed (needed for the libFuzzer build of $target)"
        exit 0
    fi
done

cmake -S "$src" -B "$build" -G Ninja -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
    -DAIR2SPIRV_BUILD_FUZZERS=ON -DAIR2SPIRV_WITH_LLVM=ON -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build "$build" --target "$target"

cleanup_work=0
if [ -z "${FUZZ_WORK:-}" ]; then
    FUZZ_WORK=$(mktemp -d "${TMPDIR:-/tmp}/ritsu-fuzz.XXXXXX")
    cleanup_work=1
fi
cleanup() {
    if [ "$cleanup_work" = 1 ]; then rm -rf "$FUZZ_WORK"; fi
}
trap cleanup EXIT

work="$FUZZ_WORK/$target"
mkdir -p "$work" "$artifacts"
dirs="$work"
[ -d "$src/fuzz/regressions/$target" ] && dirs="$dirs $src/fuzz/regressions/$target"
dirs="$dirs $src/fuzz/corpus/$target"

echo "=== $target (${time}s, fork=${FUZZ_JOBS:-2}) ==="
# In fork mode libFuzzer exits with 1 when it ignored findings; the triage decides.
status=0
# shellcheck disable=SC2086  # $dirs is a list of paths without spaces
"$build/fuzz/$target" "-max_total_time=$time" -timeout=10 -rss_limit_mb=2048 \
    "-fork=${FUZZ_JOBS:-2}" -ignore_crashes=1 -ignore_ooms=1 -ignore_timeouts=1 \
    "-artifact_prefix=$artifacts/" $dirs || status=$?
if [ "$status" -gt 1 ]; then
    echo "error: $target exited with status $status" >&2
    exit "$status"
fi

echo "=== triage of $artifacts ==="
python3 -B "$src/fuzz/triage.py" "$build/fuzz/$target" "$artifacts"
